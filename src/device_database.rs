//! Persistent Device Database
//!
//! Provides stable device identity and naming across API outages by storing
//! device metadata in a local JSON file.
//!
//! ## Design Principles
//!
//! 1. **Database is source of truth for naming** - Entity IDs come from database,
//!    not from the API that discovered the device
//! 2. **API enriches, doesn't replace** - API metadata updates database but
//!    doesn't overwrite user customizations
//! 3. **LAN devices match to database** - LAN-discovered devices are matched
//!    to existing database entries by device_id
//! 4. **Separate from cache** - Database persists when cache is cleared
//!
//! ## File Format
//!
//! The database is stored as a pretty-printed JSON file. The path is configured
//! via the `--device-db` CLI argument.
//!
//! Default locations:
//! - `/data/devices.json` (Home Assistant add-on, explicitly set via CLI in run.sh)
//! - `~/.cache/govee2mqtt/devices.json` (standalone, when --device-db is not specified)
//!
//! Writes are atomic (write to temp file, then rename) to prevent corruption.

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::NamedTempFile;

/// The persistent device database
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeviceDatabase {
    /// Schema version for future migrations
    #[serde(default = "default_version")]
    pub version: u32,

    /// Map of device_id -> device state
    pub devices: BTreeMap<String, PersistedDevice>,
}

fn default_version() -> u32 {
    2 // Bumped for new data structure
}

/// Per-API-source metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedApiInfo {
    /// Device name from this API source
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Room name from this API source
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,

    /// Last successful sync from this API source
    pub last_sync: DateTime<Utc>,
}

/// Persisted state for a single device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedDevice {
    /// Unique device identifier (e.g., "AA:BB:CC:DD:EE:FF:00:11")
    pub id: String,

    /// Device SKU (e.g., "H6072")
    pub sku: String,

    /// Display name - the definitive name for Home Assistant entity naming.
    /// Updated from API sources, or can be edited directly in the JSON.
    pub name: String,

    /// Room/area - the definitive room for Home Assistant suggested_area.
    /// Updated from API sources, or can be edited directly in the JSON.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,

    /// When this device was first seen
    pub first_seen: DateTime<Utc>,

    /// Per-API-source metadata tracking
    #[serde(default)]
    pub api_info: BTreeMap<DiscoverySource, PersistedApiInfo>,
}

/// Source of device discovery
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySource {
    PlatformApi,
    UndocApi,
    Lan,
    Ble,
}

impl std::fmt::Display for DiscoverySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscoverySource::PlatformApi => write!(f, "platform_api"),
            DiscoverySource::UndocApi => write!(f, "undoc_api"),
            DiscoverySource::Lan => write!(f, "lan"),
            DiscoverySource::Ble => write!(f, "ble"),
        }
    }
}

impl PersistedDevice {
    /// Create a new device with minimal information (LAN discovery)
    pub fn new_minimal(id: &str, sku: &str, source: DiscoverySource) -> Self {
        let now = Utc::now();
        let computed_name = compute_device_name(sku, id);

        let mut api_info = BTreeMap::new();
        api_info.insert(
            source,
            PersistedApiInfo {
                name: None,
                room: None,
                last_sync: now,
            },
        );

        Self {
            id: id.to_string(),
            sku: sku.to_string(),
            name: computed_name,
            room: None,
            first_seen: now,
            api_info,
        }
    }

    /// Create a new device with full API information
    pub fn new_from_api(
        id: &str,
        sku: &str,
        name: &str,
        room: Option<&str>,
        source: DiscoverySource,
    ) -> Self {
        let now = Utc::now();

        let mut api_info = BTreeMap::new();
        api_info.insert(
            source,
            PersistedApiInfo {
                name: Some(name.to_string()),
                room: room.map(|s| s.to_string()),
                last_sync: now,
            },
        );

        Self {
            id: id.to_string(),
            sku: sku.to_string(),
            name: name.to_string(),
            room: room.map(|s| s.to_string()),
            first_seen: now,
            api_info,
        }
    }

    /// Get the most recent API sync timestamp across all sources
    pub fn last_api_sync(&self) -> Option<DateTime<Utc>> {
        self.api_info.values().map(|info| info.last_sync).max()
    }
}

/// Compute a device name from SKU and ID (matches Device::computed_name logic)
fn compute_device_name(sku: &str, id: &str) -> String {
    // The id is usually "XX:XX:XX:XX:XX:XX:XX:XX" but some devices
    // report it without colons, and in lowercase. Normalize it.
    let mut normalized_id = String::new();
    for c in id.chars() {
        if c == ':' {
            continue;
        }
        normalized_id.push(c.to_ascii_uppercase());
    }

    format!(
        "{}_{}",
        sku,
        normalized_id
            .get(normalized_id.len().saturating_sub(4)..)
            .unwrap_or(&normalized_id)
    )
}

impl DeviceDatabase {
    /// Load database from disk, or create empty if not exists
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match fs::read_to_string(path) {
            Ok(contents) => {
                let db: DeviceDatabase = serde_json::from_str(&contents)?;
                log::info!(
                    "Loaded device database with {} devices from {:?}",
                    db.devices.len(),
                    path
                );
                Ok(db)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                log::info!("No device database found at {:?}, starting fresh", path);
                Ok(DeviceDatabase::default())
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Save database atomically (write temp file, then rename)
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        // Create parent directory if needed
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Invalid database path: no parent directory"))?;
        fs::create_dir_all(parent)?;

        let contents = serde_json::to_string_pretty(self)?;

        // Use NamedTempFile for robust atomic writes:
        // - Creates file with unique name (avoids collisions)
        // - Cleans up on drop if persist() fails
        // - Handles symlink race conditions
        let mut file = NamedTempFile::new_in(parent)?;
        file.write_all(contents.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(path)?;

        log::debug!(
            "Saved device database with {} devices to {:?}",
            self.devices.len(),
            path
        );

        Ok(())
    }

    /// Get the default database path (cache directory for standalone use)
    pub fn default_path() -> PathBuf {
        dirs_next::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("govee2mqtt")
            .join("devices.json")
    }

    /// Get the effective display name for a device
    pub fn get_display_name(&self, device_id: &str) -> Option<&str> {
        self.devices.get(device_id).map(|d| d.name.as_str())
    }

    /// Get the effective room for a device
    pub fn get_room(&self, device_id: &str) -> Option<&str> {
        self.devices.get(device_id).and_then(|d| d.room.as_deref())
    }

    /// Check if a device exists in the database
    pub fn contains(&self, device_id: &str) -> bool {
        self.devices.contains_key(device_id)
    }

    /// Get all device IDs
    pub fn device_ids(&self) -> impl Iterator<Item = &str> {
        self.devices.keys().map(|s| s.as_str())
    }

    /// Check if database is empty (for first-run detection)
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    /// Get device count
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    /// Update database from API-discovered device
    pub fn update_from_api(
        &mut self,
        device_id: &str,
        sku: &str,
        api_name: &str,
        api_room: Option<&str>,
        source: DiscoverySource,
    ) {
        let now = Utc::now();

        if let Some(existing) = self.devices.get_mut(device_id) {
            // Update existing device
            existing.sku = sku.to_string();
            existing.name = api_name.to_string();
            if api_room.is_some() {
                existing.room = api_room.map(|s| s.to_string());
            }

            // Update per-API tracking
            existing.api_info.insert(
                source.clone(),
                PersistedApiInfo {
                    name: Some(api_name.to_string()),
                    room: api_room.map(|s| s.to_string()),
                    last_sync: now,
                },
            );

            log::trace!(
                "Updated existing device {} in database from {:?}",
                device_id,
                source
            );
        } else {
            // New device
            self.devices.insert(
                device_id.to_string(),
                PersistedDevice::new_from_api(device_id, sku, api_name, api_room, source.clone()),
            );
            log::info!(
                "Added new device {} ({}) to database from {:?}",
                device_id,
                sku,
                source
            );
        }
    }

    /// Handle LAN-discovered device
    /// If known: update last_seen
    /// If unknown: create minimal entry with SKU-based name
    pub fn handle_lan_discovery(&mut self, device_id: &str, sku: &str) -> &PersistedDevice {
        let now = Utc::now();

        self.devices
            .entry(device_id.to_string())
            .and_modify(|d| {
                // Update LAN API tracking
                d.api_info.insert(
                    DiscoverySource::Lan,
                    PersistedApiInfo {
                        name: None,
                        room: None,
                        last_sync: now,
                    },
                );
            })
            .or_insert_with(|| {
                log::info!(
                    "LAN-discovered new device {} ({}), adding to database with computed name",
                    device_id,
                    sku
                );
                PersistedDevice::new_minimal(device_id, sku, DiscoverySource::Lan)
            })
    }

    /// Get a device by ID
    pub fn get(&self, device_id: &str) -> Option<&PersistedDevice> {
        self.devices.get(device_id)
    }
}

/// Thread-safe handle to the device database
#[derive(Clone)]
pub struct DeviceDatabaseHandle {
    inner: Arc<RwLock<DeviceDatabase>>,
    path: PathBuf,
}

impl DeviceDatabaseHandle {
    /// Create a new handle by loading or creating the database
    pub fn open(path: PathBuf) -> anyhow::Result<Self> {
        let db = DeviceDatabase::load(&path)?;

        Ok(Self {
            inner: Arc::new(RwLock::new(db)),
            path,
        })
    }

    /// Get the path where the database is stored
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Clone the database for saving outside the lock
    fn clone_database(&self) -> DeviceDatabase {
        self.inner.read().clone()
    }

    /// Save the database to disk (lock-free IO)
    /// Clones the data first, releases lock, then writes to disk
    pub fn save(&self) -> anyhow::Result<()> {
        let db = self.clone_database();
        db.save(&self.path)
    }

    /// Get the number of devices in the database
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    /// Check if database is empty
    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }

    /// Get the effective display name for a device
    pub fn get_display_name(&self, device_id: &str) -> Option<String> {
        self.inner
            .read()
            .get_display_name(device_id)
            .map(|s| s.to_string())
    }

    /// Get the effective room for a device
    pub fn get_room(&self, device_id: &str) -> Option<String> {
        self.inner
            .read()
            .get_room(device_id)
            .map(|s| s.to_string())
    }

    /// Check if a device exists in the database
    pub fn contains(&self, device_id: &str) -> bool {
        self.inner.read().contains(device_id)
    }

    /// Update from API discovery
    pub fn update_from_api(
        &self,
        device_id: &str,
        sku: &str,
        api_name: &str,
        api_room: Option<&str>,
        source: DiscoverySource,
    ) {
        self.inner
            .write()
            .update_from_api(device_id, sku, api_name, api_room, source);
    }

    /// Handle LAN discovery - returns the display name to use
    pub fn handle_lan_discovery(&self, device_id: &str, sku: &str) -> String {
        let mut db = self.inner.write();
        let device = db.handle_lan_discovery(device_id, sku);
        device.name.clone()
    }

    /// Get all devices from the database
    pub fn list_devices(&self) -> Vec<PersistedDevice> {
        self.inner.read().devices.values().cloned().collect()
    }

    /// Get a single device by ID
    pub fn get_device(&self, device_id: &str) -> Option<PersistedDevice> {
        self.inner.read().get(device_id).cloned()
    }
}

/// Determine the startup mode based on existing files
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupMode {
    /// Fresh install: no device DB, no cache
    FreshInstall,
    /// Upgrading from old version: cache exists, no device DB
    Upgrade,
    /// Normal startup: device DB exists
    Normal,
}

impl StartupMode {
    /// Detect the startup mode based on file existence
    pub fn detect(device_db_path: &Path, cache_path: &Path) -> Self {
        let has_device_db = device_db_path.exists();
        let has_cache = cache_path.exists();

        match (has_device_db, has_cache) {
            (true, _) => StartupMode::Normal,
            (false, true) => StartupMode::Upgrade,
            (false, false) => StartupMode::FreshInstall,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_compute_device_name() {
        assert_eq!(
            compute_device_name("H6072", "AA:BB:CC:DD:EE:FF:00:11"),
            "H6072_0011"
        );
        assert_eq!(
            compute_device_name("H6072", "aabbccddeeff0011"),
            "H6072_0011"
        );
        assert_eq!(compute_device_name("H6072", "1234"), "H6072_1234");
        assert_eq!(compute_device_name("H6072", "12"), "H6072_12");
    }

    #[test]
    fn test_database_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test_devices.json");

        let mut db = DeviceDatabase::default();
        db.update_from_api(
            "AA:BB:CC:DD:EE:FF:00:11",
            "H6072",
            "Test Light",
            Some("Living Room"),
            DiscoverySource::PlatformApi,
        );

        db.save(&path).unwrap();

        let loaded = DeviceDatabase::load(&path).unwrap();
        assert_eq!(loaded.devices.len(), 1);

        let device = loaded.devices.get("AA:BB:CC:DD:EE:FF:00:11").unwrap();
        assert_eq!(device.name, "Test Light");
        assert_eq!(device.room, Some("Living Room".to_string()));
        assert!(device.api_info.contains_key(&DiscoverySource::PlatformApi));
    }

    #[test]
    fn test_lan_discovery_new_device() {
        let mut db = DeviceDatabase::default();
        let device = db.handle_lan_discovery("AA:BB:CC:DD:EE:FF:00:11", "H6072");

        assert_eq!(device.name, "H6072_0011");
        assert_eq!(device.sku, "H6072");
        assert!(device.api_info.contains_key(&DiscoverySource::Lan));
    }

    #[test]
    fn test_api_updates_existing_device() {
        let mut db = DeviceDatabase::default();

        // First, LAN discovery
        db.handle_lan_discovery("AA:BB:CC:DD:EE:FF:00:11", "H6072");

        // Then API enriches it
        db.update_from_api(
            "AA:BB:CC:DD:EE:FF:00:11",
            "H6072",
            "Kitchen Light",
            Some("Kitchen"),
            DiscoverySource::PlatformApi,
        );

        let device = db.get("AA:BB:CC:DD:EE:FF:00:11").unwrap();
        assert_eq!(device.name, "Kitchen Light");
        assert_eq!(device.room, Some("Kitchen".to_string()));
        // Should have both LAN and Platform API entries
        assert!(device.api_info.contains_key(&DiscoverySource::Lan));
        assert!(device.api_info.contains_key(&DiscoverySource::PlatformApi));
    }

    #[test]
    fn test_handle_missing_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.json");

        let db = DeviceDatabase::load(&path).unwrap();
        assert!(db.is_empty());
    }

    #[test]
    fn test_startup_mode_detection() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("devices.json");
        let cache_path = dir.path().join("cache.sqlite");

        // Fresh install
        assert_eq!(
            StartupMode::detect(&db_path, &cache_path),
            StartupMode::FreshInstall
        );

        // Upgrade (cache exists, no DB)
        std::fs::write(&cache_path, "").unwrap();
        assert_eq!(
            StartupMode::detect(&db_path, &cache_path),
            StartupMode::Upgrade
        );

        // Normal (DB exists)
        std::fs::write(&db_path, "{}").unwrap();
        assert_eq!(
            StartupMode::detect(&db_path, &cache_path),
            StartupMode::Normal
        );
    }

    #[test]
    fn test_last_api_sync() {
        let mut db = DeviceDatabase::default();

        // Add via LAN first
        db.handle_lan_discovery("AA:BB:CC:DD:EE:FF:00:11", "H6072");
        let device = db.get("AA:BB:CC:DD:EE:FF:00:11").unwrap();
        let lan_sync = device.last_api_sync();

        // Update via Platform API
        std::thread::sleep(std::time::Duration::from_millis(10));
        db.update_from_api(
            "AA:BB:CC:DD:EE:FF:00:11",
            "H6072",
            "Test",
            None,
            DiscoverySource::PlatformApi,
        );
        let device = db.get("AA:BB:CC:DD:EE:FF:00:11").unwrap();
        let platform_sync = device.last_api_sync();

        // Platform sync should be newer
        assert!(platform_sync > lan_sync);
    }
}
