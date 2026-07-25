use anyhow::Context;
use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sqlite_cache::{Cache, CacheConfig, KeyUpdater};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub static CACHE: Lazy<ArcSwap<Cache>> =
    Lazy::new(|| open_cache().expect("failed to initialize cache").into());

/// Fallback tier for entries the on-disk cache refused to accept.
///
/// If the filesystem holding the cache becomes unwritable -- a failing SD card
/// remounting read-only, or a full disk -- every `write` fails. With SQLite as
/// the only tier there is nowhere else to keep the value, so it is recomputed
/// on every request. For `account-info` that means a fresh Govee account login
/// each cycle instead of the intended two per day, which is enough to trip
/// Govee's abuse detection and get the account rate-limited for 24 hours.
///
/// Keeping the value in memory preserves the intended TTL for the lifetime of
/// the process, so a storage fault degrades to "no persistence across restarts"
/// rather than "re-authenticate constantly".
static MEMORY_FALLBACK: Lazy<Mutex<HashMap<(String, String), MemoryEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

struct MemoryEntry {
    data: Vec<u8>,
    expires: DateTime<Utc>,
}

fn memory_get(topic: &str, key: &str) -> Option<Vec<u8>> {
    let mut map = MEMORY_FALLBACK.lock().ok()?;
    let map_key = (topic.to_string(), key.to_string());
    match map.get(&map_key) {
        Some(entry) if Utc::now() < entry.expires => Some(entry.data.clone()),
        Some(_) => {
            map.remove(&map_key);
            None
        }
        None => None,
    }
}

fn memory_put(topic: &str, key: &str, data: Vec<u8>, hard_ttl: Duration) {
    if let Ok(mut map) = MEMORY_FALLBACK.lock() {
        map.insert(
            (topic.to_string(), key.to_string()),
            MemoryEntry {
                data,
                expires: Utc::now() + hard_ttl,
            },
        );
    }
}

fn memory_forget(topic: &str, key: &str) {
    if let Ok(mut map) = MEMORY_FALLBACK.lock() {
        map.remove(&(topic.to_string(), key.to_string()));
    }
}

fn memory_clear() {
    if let Ok(mut map) = MEMORY_FALLBACK.lock() {
        map.clear();
    }
}

/// Persist a cache entry.
///
/// A failure to write the cache must never discard an otherwise successful
/// computation, so the error is logged and the value retained in memory
/// instead of being propagated to the caller.
fn persist(updater: KeyUpdater, data: &[u8], options: &CacheGetOptions<'_>) {
    match updater.write(data, options.hard_ttl) {
        Ok(()) => memory_forget(options.topic, options.key),
        Err(err) => {
            log::warn!(
                "cache write failed for {}/{}: {err:#}. Keeping the value in \
                 memory instead; the on-disk cache is not persisting \
                 (read-only filesystem, or out of space?)",
                options.topic,
                options.key
            );
            memory_put(options.topic, options.key, data.to_vec(), options.hard_ttl);
        }
    }
}

fn cache_file_name() -> PathBuf {
    let cache_dir = std::env::var("GOVEE_CACHE_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(dirs_next::cache_dir)
        .expect("failed to resolve cache dir");

    cache_dir.join("govee2mqtt-cache.sqlite")
}

fn open_cache() -> anyhow::Result<Arc<Cache>> {
    let cache_file = cache_file_name();
    let conn = sqlite_cache::rusqlite::Connection::open(&cache_file)
        .unwrap_or_else(|_| panic!("failed to open {cache_file:?}"));
    Ok(Arc::new(Cache::new(
        // We have low cardinality and can be pretty relaxed
        CacheConfig {
            flush_gc_ratio: 1024,
            flush_interval: Duration::from_secs(900),
            max_ttl: None,
        },
        conn,
    )?))
}

pub fn purge_cache() -> anyhow::Result<()> {
    memory_clear();
    let cache_file = cache_file_name();
    std::fs::remove_file(&cache_file)
        .with_context(|| format!("removing cache file {cache_file:?}"))?;
    CACHE.store(open_cache()?);
    Ok(())
}

#[derive(Deserialize, Serialize, Debug)]
struct CacheEntry<T> {
    expires: DateTime<Utc>,
    result: CacheResult<T>,
}

#[derive(Deserialize, Serialize, Debug)]
enum CacheResult<T> {
    Ok(T),
    Err(String),
}

impl<T> CacheResult<T> {
    fn into_result(self) -> anyhow::Result<T> {
        match self {
            Self::Ok(v) => Ok(v),
            Self::Err(err) => anyhow::bail!("{err}"),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct CacheGetOptions<'a> {
    pub key: &'a str,
    pub topic: &'a str,
    pub soft_ttl: Duration,
    pub hard_ttl: Duration,
    pub negative_ttl: Duration,
    pub allow_stale: bool,
}

pub enum CacheComputeResult<T> {
    Value(T),
    WithTtl(T, Duration),
}

impl<T> CacheComputeResult<T> {
    #[allow(dead_code)]
    pub fn into_inner(self) -> T {
        match self {
            Self::Value(v) | Self::WithTtl(v, _) => v,
        }
    }
}

pub fn invalidate_key(topic: &str, key: &str) -> anyhow::Result<()> {
    memory_forget(topic, key);
    let topic = CACHE.load().topic(topic)?;
    Ok(topic.delete(key)?)
}

/// Cache an item with a soft TTL; we'll retry the operation
/// if the TTL has expired, but allow stale reads
pub async fn cache_get<T, Fut>(options: CacheGetOptions<'_>, future: Fut) -> anyhow::Result<T>
where
    T: Serialize + DeserializeOwned + std::fmt::Debug + Clone,
    Fut: Future<Output = anyhow::Result<CacheComputeResult<T>>>,
{
    let topic = CACHE.load().topic(options.topic)?;
    let (updater, current_value) = topic.get_for_update(options.key).await?;
    let now = Utc::now();

    let mut cache_entry: Option<CacheEntry<T>> = None;

    // Prefer the on-disk cache. The in-memory tier only holds entries that the
    // on-disk cache refused to accept, so it is consulted when SQLite has
    // nothing for this key.
    let cached_bytes: Option<Vec<u8>> = current_value
        .as_ref()
        .map(|current| current.data.clone())
        .or_else(|| memory_get(options.topic, options.key));

    if let Some(bytes) = &cached_bytes {
        match serde_json::from_slice::<CacheEntry<T>>(bytes) {
            Ok(entry) => {
                if now < entry.expires {
                    log::trace!("cache hit for {}", options.key);
                    return entry.result.into_result();
                }

                cache_entry.replace(entry);
            }
            Err(err) => {
                log::warn!(
                    "Error parsing CacheEntry: {err:#} {:?}",
                    String::from_utf8_lossy(bytes)
                );
            }
        }
    }

    log::trace!("cache miss for {}", options.key);
    let value: anyhow::Result<CacheComputeResult<T>> = future.await;
    match value {
        Ok(CacheComputeResult::WithTtl(value, ttl)) => {
            let entry = CacheEntry {
                expires: Utc::now() + ttl,
                result: CacheResult::Ok(value.clone()),
            };

            let data = serde_json::to_string_pretty(&entry)?;
            persist(updater, data.as_bytes(), &options);
            Ok(value)
        }
        Ok(CacheComputeResult::Value(value)) => {
            let entry = CacheEntry {
                expires: Utc::now() + options.soft_ttl,
                result: CacheResult::Ok(value.clone()),
            };

            let data = serde_json::to_string_pretty(&entry)?;
            persist(updater, data.as_bytes(), &options);
            Ok(value)
        }
        Err(err) => match cache_entry.take() {
            Some(mut entry) if options.allow_stale => {
                entry.expires = Utc::now() + options.negative_ttl;

                log::warn!("{err:#}, will use prior results");
                if matches!(&entry.result, CacheResult::Err(_)) {
                    entry.result = CacheResult::Err(format!("{err:#}"));
                }

                let data = serde_json::to_string_pretty(&entry)?;
                persist(updater, data.as_bytes(), &options);

                entry.result.into_result()
            }
            _ => {
                let entry = CacheEntry {
                    expires: Utc::now() + options.negative_ttl,
                    result: CacheResult::Err(format!("{err:#}")),
                };

                let data = serde_json::to_string_pretty(&entry)?;
                persist(updater, data.as_bytes(), &options);
                entry.result.into_result()
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_fallback_round_trip() {
        memory_put(
            "test-topic",
            "round-trip",
            b"hello".to_vec(),
            Duration::from_secs(60),
        );
        assert_eq!(
            memory_get("test-topic", "round-trip").as_deref(),
            Some(&b"hello"[..])
        );

        memory_forget("test-topic", "round-trip");
        assert_eq!(memory_get("test-topic", "round-trip"), None);
    }

    #[test]
    fn memory_fallback_honours_ttl() {
        memory_put(
            "test-topic",
            "expiring",
            b"stale".to_vec(),
            Duration::from_secs(0),
        );
        assert_eq!(
            memory_get("test-topic", "expiring"),
            None,
            "an entry whose hard TTL has elapsed must not be served"
        );
    }

    #[test]
    fn memory_fallback_is_keyed_by_topic_and_key() {
        memory_put("topic-a", "shared", b"a".to_vec(), Duration::from_secs(60));
        memory_put("topic-b", "shared", b"b".to_vec(), Duration::from_secs(60));

        assert_eq!(memory_get("topic-a", "shared").as_deref(), Some(&b"a"[..]));
        assert_eq!(memory_get("topic-b", "shared").as_deref(), Some(&b"b"[..]));

        memory_forget("topic-a", "shared");
        assert_eq!(memory_get("topic-a", "shared"), None);
        assert_eq!(
            memory_get("topic-b", "shared").as_deref(),
            Some(&b"b"[..]),
            "forgetting one topic must not affect another"
        );

        memory_forget("topic-b", "shared");
    }
}
