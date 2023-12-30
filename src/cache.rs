use once_cell::sync::Lazy;
use serde::de::DeserializeOwned;
use serde::Serialize;
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
    Cache::new(CacheConfig::default(), conn).expect("failed to initialize cache")
});

pub async fn cache_get<T, Fut>(
    topic: &str,
    key: &str,
    ttl: Duration,
    future: Fut,
) -> anyhow::Result<T>
where
    T: Serialize + DeserializeOwned + std::fmt::Debug,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let topic = CACHE.topic(topic)?;
    let (updater, current_value) = topic.get_for_update(key).await?;
    if let Some(current) = current_value {
        let result: T = serde_json::from_slice(&current.data)?;
        return Ok(result);
    }

    let value: T = future.await?;
    let data = serde_json::to_string_pretty(&value)?;
    updater.write(data.as_bytes(), ttl)?;

    Ok(value)
}
