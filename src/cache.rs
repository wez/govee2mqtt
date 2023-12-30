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

    let cache_file = cache_dir.join("govee-rs-cache.sqlite");
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
}

/// Cache an item with a soft TTL; we'll retry the operation
/// if the TTL has expired, but allow stale reads
pub async fn cache_get<T, Fut>(options: CacheGetOptions<'_>, future: Fut) -> anyhow::Result<T>
where
    T: Serialize + DeserializeOwned + std::fmt::Debug + Clone,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let topic = CACHE.topic(options.topic)?;
    let (updater, current_value) = topic.get_for_update(options.key).await?;
    let now = Utc::now();

    let mut cache_entry: Option<CacheEntry<T>> = None;

    if let Some(current) = &current_value {
        if let Ok(entry) = serde_json::from_slice::<CacheEntry<T>>(&current.data) {
            if now < entry.expires {
                log::trace!("cache hit for {}", options.key);
                return entry.result.into_result();
            }

            cache_entry.replace(entry);
        }
    }

    log::trace!("cache miss for {}", options.key);
    let value: anyhow::Result<T> = future.await;
    match value {
        Ok(value) => {
            let entry = CacheEntry {
                expires: Utc::now() + options.soft_ttl,
                result: CacheResult::Ok(value.clone()),
            };

            let data = serde_json::to_string_pretty(&entry)?;
            updater.write(data.as_bytes(), options.hard_ttl)?;
            Ok(value)
        }
        Err(err) => match cache_entry.take() {
            Some(mut entry) => {
                entry.expires = Utc::now() + options.negative_ttl;

                log::warn!("{err:#}, will use prior results");
                if matches!(&entry.result, CacheResult::Err(_)) {
                    entry.result = CacheResult::Err(format!("{err:#}"));
                }

                let data = serde_json::to_string_pretty(&entry)?;
                updater.write(data.as_bytes(), options.hard_ttl)?;

                entry.result.into_result()
            }
            None => {
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
