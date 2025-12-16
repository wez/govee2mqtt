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
//! The database is stored as a pretty-printed JSON file at:
//! - `/data/devices.json` (Home Assistant add-on)
//! - `$GOVEE_DEVICE_DB` (if set)
//! - `~/.cache/govee2mqtt/devices.json` (default)
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
    1
}

/// Persisted state for a single device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedDevice {
    /// Unique device identifier (e.g., "AA:BB:CC:DD:EE:FF:00:11")
    pub id: String,

    /// Device SKU (e.g., "H6072")
    pub sku: String,

    /// Display name - populated from API or user override
    /// This is used for Home Assistant entity naming
    pub name: String,

    /// Room/area - populated from API or user override
    /// Used for Home Assistant suggested_area
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,

    /// Which API first discovered this device
    pub discovered_via: DiscoverySource,

    /// When this device was first seen
    pub first_seen: DateTime<Utc>,

    /// When this device was last seen (any source)
    pub last_seen: DateTime<Utc>,

    /// Last successful API metadata sync
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_api_sync: Option<DateTime<Utc>>,

    // === User Overrides (editable via JSON or future web UI) ===
    /// User-specified name override (takes precedence over API name)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,

    /// User-specified room override
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_room: Option<String>,
}

/// Source of initial device discovery
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    /// Get the effective display name (user override > API name)
    pub fn display_name(&self) -> &str {
        self.user_name.as_deref().unwrap_or(&self.name)
    }

    /// Get the effective room (user override > API room)
    pub fn effective_room(&self) -> Option<&str> {
        self.user_room.as_deref().or(self.room.as_deref())
    }

    /// Create a new device with minimal information (LAN discovery)
    pub fn new_minimal(id: &str, sku: &str, source: DiscoverySource) -> Self {
        let now = Utc::now();
        let computed_name = compute_device_name(sku, id);

        Self {
            id: id.to_string(),
            sku: sku.to_string(),
            name: computed_name,
            room: None,
            discovered_via: source,
            first_seen: now,
            last_seen: now,
            last_api_sync: None,
            user_name: None,
            user_room: None,
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

        Self {
            id: id.to_string(),
            sku: sku.to_string(),
            name: name.to_string(),
            room: room.map(|s| s.to_string()),
            discovered_via: source,
            first_seen: now,
            last_seen: now,
            last_api_sync: Some(now),
            user_name: None,
            user_room: None,
        }
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
        &normalized_id[normalized_id.len().saturating_sub(4)..]
    )
}

impl DeviceDatabase {
    /// Load database from disk, or create empty if not exists
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            let contents = fs::read_to_string(path)?;
            let db: DeviceDatabase = serde_json::from_str(&contents)?;
            log::info!(
                "Loaded device database with {} devices from {:?}",
                db.devices.len(),
                path
            );
            Ok(db)
        } else {
            log::info!(
                "No device database found at {:?}, starting fresh",
                path
            );
            Ok(DeviceDatabase::default())
        }
    }

    /// Save database atomically (write temp file, then rename)
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Write to temporary file in same directory
        let temp_path = path.with_extension("json.tmp");
        let contents = serde_json::to_string_pretty(self)?;

        {
            let mut file = fs::File::create(&temp_path)?;
            file.write_all(contents.as_bytes())?;
            file.sync_all()?; // Ensure data is on disk
        }

        // Atomic rename
        fs::rename(&temp_path, path)?;

        log::debug!(
            "Saved device database with {} devices to {:?}",
            self.devices.len(),
            path
        );

        Ok(())
    }

    /// Get the default database path based on environment
    pub fn default_path() -> PathBuf {
        // Check for explicit configuration first
        if let Ok(path) = std::env::var("GOVEE_DEVICE_DB") {
            return PathBuf::from(path);
        }

        // Home Assistant add-on uses /data
        let addon_path = PathBuf::from("/data/devices.json");
        if addon_path
            .parent()
            .map(|p| p.exists())
            .unwrap_or(false)
        {
            return addon_path;
        }

        // Fall back to cache directory
        dirs_next::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("govee2mqtt")
            .join("devices.json")
    }

    /// Get the effective display name for a device
    pub fn get_display_name(&self, device_id: &str) -> Option<&str> {
        self.devices.get(device_id).map(|d| d.display_name())
    }

    /// Get the effective room for a device
    pub fn get_room(&self, device_id: &str) -> Option<&str> {
        self.devices
            .get(device_id)
            .and_then(|d| d.effective_room())
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
    /// Preserves user overrides, updates API-sourced fields
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
            // Update existing device - preserve user overrides
            existing.sku = sku.to_string();
            existing.name = api_name.to_string();
            existing.room = api_room.map(|s| s.to_string());
            existing.last_seen = now;
            existing.last_api_sync = Some(now);
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
                d.last_seen = now;
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
    pub fn open(path: Option<PathBuf>) -> anyhow::Result<Self> {
        let path = path.unwrap_or_else(DeviceDatabase::default_path);
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

    /// Save the database to disk
    pub fn save(&self) -> anyhow::Result<()> {
        let db = self.inner.read();
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
        device.display_name().to_string()
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
    fn test_persisted_device_display_name() {
        let mut device = PersistedDevice::new_from_api(
            "test-id",
            "H6072",
            "Living Room Lamp",
            Some("Living Room"),
            DiscoverySource::PlatformApi,
        );

        // Default: use API name
        assert_eq!(device.display_name(), "Living Room Lamp");
        assert_eq!(device.effective_room(), Some("Living Room"));

        // User override takes precedence
        device.user_name = Some("My Custom Name".to_string());
        device.user_room = Some("Kitchen".to_string());

        assert_eq!(device.display_name(), "My Custom Name");
        assert_eq!(device.effective_room(), Some("Kitchen"));
    }

    #[test]
    fn test_device_database_roundtrip() -> anyhow::Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("devices.json");

        let mut db = DeviceDatabase::default();
        db.update_from_api(
            "device-1",
            "H6072",
            "Test Light",
            Some("Living Room"),
            DiscoverySource::PlatformApi,
        );

        db.save(&db_path)?;

        let loaded = DeviceDatabase::load(&db_path)?;
        assert_eq!(loaded.devices.len(), 1);
        assert_eq!(loaded.get_display_name("device-1"), Some("Test Light"));
        assert_eq!(loaded.get_room("device-1"), Some("Living Room"));

        Ok(())
    }

    #[test]
    fn test_startup_mode_detection() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("devices.json");
        let cache_path = temp_dir.path().join("cache.sqlite");

        // Fresh install
        assert_eq!(
            StartupMode::detect(&db_path, &cache_path),
            StartupMode::FreshInstall
        );

        // Create cache file - upgrade scenario
        std::fs::write(&cache_path, "test").unwrap();
        assert_eq!(
            StartupMode::detect(&db_path, &cache_path),
            StartupMode::Upgrade
        );

        // Create device DB - normal startup
        std::fs::write(&db_path, "{}").unwrap();
        assert_eq!(
            StartupMode::detect(&db_path, &cache_path),
            StartupMode::Normal
        );
    }

    #[test]
    fn test_handle_lan_discovery() {
        let mut db = DeviceDatabase::default();

        // First discovery - creates new entry
        let device = db.handle_lan_discovery("AA:BB:CC:DD:EE:FF:00:11", "H6072");
        assert_eq!(device.name, "H6072_0011");
        assert_eq!(device.discovered_via, DiscoverySource::Lan);

        // Second discovery - updates last_seen
        let first_seen = device.first_seen;
        let last_seen = device.last_seen;

        std::thread::sleep(std::time::Duration::from_millis(10));
        let device2 = db.handle_lan_discovery("AA:BB:CC:DD:EE:FF:00:11", "H6072");

        assert_eq!(device2.first_seen, first_seen);
        assert!(device2.last_seen >= last_seen);
    }

    #[test]
    fn test_update_from_api_preserves_user_overrides() {
        let mut db = DeviceDatabase::default();

        // Initial API discovery
        db.update_from_api(
            "device-1",
            "H6072",
            "Original Name",
            Some("Original Room"),
            DiscoverySource::PlatformApi,
        );

        // Set user overrides
        if let Some(device) = db.devices.get_mut("device-1") {
            device.user_name = Some("User Name".to_string());
            device.user_room = Some("User Room".to_string());
        }

        // API update
        db.update_from_api(
            "device-1",
            "H6072",
            "New API Name",
            Some("New API Room"),
            DiscoverySource::PlatformApi,
        );

        // Verify user overrides are preserved
        let device = db.devices.get("device-1").unwrap();
        assert_eq!(device.name, "New API Name"); // API name updated
        assert_eq!(device.room, Some("New API Room".to_string())); // API room updated
        assert_eq!(device.user_name, Some("User Name".to_string())); // User override preserved
        assert_eq!(device.user_room, Some("User Room".to_string())); // User override preserved

        // display_name should use user override
        assert_eq!(device.display_name(), "User Name");
        assert_eq!(device.effective_room(), Some("User Room"));
    }
}
