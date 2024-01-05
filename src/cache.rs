use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sqlite_cache::{Cache, CacheConfig};
use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

pub static CACHE: Lazy<Cache> = Lazy::new(|| {
    let cache_dir = std::env::var("GOVEE_CACHE_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(|| dirs_next::cache_dir())
        .expect("failed to resolve cache dir");

    let cache_file = cache_dir.join("govee2mqtt-cache.sqlite");
    let conn = sqlite_cache::rusqlite::Connection::open(&cache_file)
        .expect(&format!("failed to open {cache_file:?}"));
    Cache::new(
        // We have low cardinality and can be pretty relaxed
        CacheConfig {
            flush_gc_ratio: 1024,
            flush_interval: Duration::from_secs(900),
            max_ttl: None,
        },
        conn,
    )
    .expect("failed to initialize cache")
});

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

/// Cache an item with a soft TTL; we'll retry the operation
/// if the TTL has expired, but allow stale reads
pub async fn cache_get<T, Fut>(options: CacheGetOptions<'_>, future: Fut) -> anyhow::Result<T>
where
    T: Serialize + DeserializeOwned + std::fmt::Debug + Clone,
    Fut: Future<Output = anyhow::Result<CacheComputeResult<T>>>,
{
    let topic = CACHE.topic(options.topic)?;
    let (updater, current_value) = topic.get_for_update(options.key).await?;
    let now = Utc::now();

    let mut cache_entry: Option<CacheEntry<T>> = None;

    if let Some(current) = &current_value {
        match serde_json::from_slice::<CacheEntry<T>>(&current.data) {
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
                    String::from_utf8_lossy(&current.data)
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
            updater.write(data.as_bytes(), options.hard_ttl)?;
            Ok(value)
        }
        Ok(CacheComputeResult::Value(value)) => {
            let entry = CacheEntry {
                expires: Utc::now() + options.soft_ttl,
                result: CacheResult::Ok(value.clone()),
            };

            let data = serde_json::to_string_pretty(&entry)?;
            updater.write(data.as_bytes(), options.hard_ttl)?;
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
                updater.write(data.as_bytes(), options.hard_ttl)?;

                entry.result.into_result()
            }
            _ => {
                let entry = CacheEntry {
                    expires: Utc::now() + options.negative_ttl,
                    result: CacheResult::Err(format!("{err:#}")),
                };

                let data = serde_json::to_string_pretty(&entry)?;
                updater.write(data.as_bytes(), options.hard_ttl)?;
                entry.result.into_result()
            }
        },
    }
}
