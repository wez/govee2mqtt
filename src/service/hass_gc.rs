use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone, Hash, Eq, PartialEq)]
pub struct PublishedEntity {
    pub unique_id: String,
    pub integration: String,
}

fn persistence_path() -> anyhow::Result<PathBuf> {
    let mut path = dirs_next::config_dir()
        .ok_or_else(|| anyhow!("No config dir found"))?
        .join("govee2mqtt");
    std::fs::create_dir_all(&path)?;
    path.push("hass-entities.json");
    Ok(path)
}

pub fn load_published_entities() -> anyhow::Result<HashSet<PublishedEntity>> {
    let path = persistence_path()?;
    if !path.exists() {
        return Ok(HashSet::new());
    }
    let data = std::fs::read_to_string(path)?;
    if data.is_empty() {
        return Ok(HashSet::new());
    }
    let entities: HashSet<PublishedEntity> = serde_json::from_str(&data)?;
    Ok(entities)
}

pub fn save_published_entities(entities: &HashSet<PublishedEntity>) -> anyhow::Result<()> {
    let path = persistence_path()?;
    let data = serde_json::to_string_pretty(entities)?;
    std::fs::write(path, data)?;
    Ok(())
}
