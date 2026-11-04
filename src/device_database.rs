//! Stable device names and rooms, kept separately from the disposable API cache.
//! API refreshes update per-source metadata without replacing the stored identity.

use anyhow::Context;
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceDatabase {
    /// Schema version for future migrations
    #[serde(default = "default_version")]
    pub version: u32,

    /// Map of device_id -> device state
    pub devices: BTreeMap<String, PersistedDevice>,
}

impl Default for DeviceDatabase {
    fn default() -> Self {
        Self {
            version: default_version(),
            devices: BTreeMap::new(),
        }
    }
}

fn default_version() -> u32 {
    2
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
    /// Initialized during discovery; subsequent API updates leave this unchanged.
    pub name: String,

    /// Room/area - the definitive room for Home Assistant suggested_area.
    /// Initialized during discovery; subsequent API updates leave this unchanged.
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
                let mut data: serde_json::Value = serde_json::from_str(&contents)
                    .with_context(|| format!("Parsing device database {path:?}; preserve this file before repairing it"))?;
                let version = data.get("version").and_then(|v| v.as_u64()).unwrap_or(2);
                anyhow::ensure!(
                    version <= 2,
                    "Unsupported device database version {version} in {path:?}"
                );
                if version <= 1 {
                    // Early versions kept explicit user overrides alongside API names.
                    if let Some(devices) = data.get_mut("devices").and_then(|v| v.as_object_mut()) {
                        for device in devices.values_mut() {
                            for (old, current) in [("user_name", "name"), ("user_room", "room")] {
                                if let Some(value) =
                                    device.get(old).filter(|v| !v.is_null()).cloned()
                                {
                                    device[current] = value;
                                }
                            }
                        }
                    }
                }
                let mut db: DeviceDatabase = serde_json::from_value(data)
                    .with_context(|| format!("Loading device database {path:?}"))?;
                db.version = default_version();
                for (id, device) in &db.devices {
                    anyhow::ensure!(
                        id == &device.id,
                        "Device database key {id:?} does not match device ID {:?}",
                        device.id
                    );
                }
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
            Err(err) => Err(err).with_context(|| format!("Reading device database {path:?}")),
        }
    }

    /// Save database atomically (write temp file, then rename)
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .with_context(|| format!("Creating device database directory {parent:?}"))?;

        let contents = serde_json::to_string_pretty(self)
            .with_context(|| format!("Serializing device database for {path:?}"))?;

        // Keep the temporary file on the same filesystem so replacement is atomic.
        let mut file = NamedTempFile::new_in(parent)
            .with_context(|| format!("Creating temporary device database in {parent:?}"))?;
        file.write_all(contents.as_bytes())
            .with_context(|| format!("Writing temporary device database for {path:?}"))?;
        file.as_file()
            .sync_all()
            .with_context(|| format!("Syncing temporary device database for {path:?}"))?;
        file.persist(path)
            .with_context(|| format!("Replacing device database {path:?}"))?;

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

    #[cfg(test)]
    pub fn get_display_name(&self, device_id: &str) -> Option<&str> {
        self.devices.get(device_id).map(|d| d.name.as_str())
    }

    #[cfg(test)]
    pub fn get_room(&self, device_id: &str) -> Option<&str> {
        self.devices.get(device_id).and_then(|d| d.room.as_deref())
    }

    /// Check if a device exists in the database
    pub fn contains(&self, device_id: &str) -> bool {
        self.devices.contains_key(device_id)
    }

    #[cfg(test)]
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
            existing.sku = sku.to_string();

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

    /// Record LAN contact without changing a known device's identity.
    pub fn handle_lan_discovery(&mut self, device_id: &str, sku: &str) -> &PersistedDevice {
        let now = Utc::now();

        self.devices
            .entry(device_id.to_string())
            .and_modify(|d| {
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

    #[cfg(test)]
    pub fn get(&self, device_id: &str) -> Option<&PersistedDevice> {
        self.devices.get(device_id)
    }
}

type SaveReply = tokio::sync::oneshot::Sender<anyhow::Result<()>>;

/// Thread-safe database access with one filesystem writer.
#[derive(Clone)]
pub struct DeviceDatabaseHandle {
    inner: Arc<RwLock<DeviceDatabase>>,
    writer: async_channel::Sender<Option<SaveReply>>,
}

impl DeviceDatabaseHandle {
    pub fn open(path: PathBuf) -> anyhow::Result<Self> {
        anyhow::ensure!(
            path.file_name().is_some(),
            "Device database path must include a file name: {path:?}"
        );
        let inner = Arc::new(RwLock::new(DeviceDatabase::load(&path)?));
        let (writer, requests) = async_channel::bounded::<Option<SaveReply>>(1);
        let db = inner.clone();
        std::thread::Builder::new()
            .name("device-database".into())
            .spawn(move || {
                while let Ok(reply) = requests.recv_blocking() {
                    // Clone in a separate statement so no data lock spans filesystem I/O.
                    // Snapshot here, rather than at enqueue time, to preserve write ordering.
                    let snapshot = db.read().clone();
                    let result = snapshot.save(&path);
                    if let Some(reply) = reply {
                        reply.send(result).ok();
                    } else if let Err(err) = result {
                        log::warn!("Failed to save device database {path:?}: {err:#}");
                    }
                }
            })
            .context("Starting device database writer")?;
        Ok(Self { inner, writer })
    }

    /// Wait for an ordered save, including all changes made before this call.
    pub async fn save(&self) -> anyhow::Result<()> {
        let (reply, saved) = tokio::sync::oneshot::channel();
        self.writer
            .send(Some(reply))
            .await
            .context("Requesting device database save")?;
        saved.await.context("Waiting for device database save")?
    }

    fn request_save(&self) {
        match self.writer.try_send(None) {
            Ok(()) | Err(async_channel::TrySendError::Full(_)) => (),
            Err(err) => log::error!("Device database writer is unavailable: {err}"),
        }
    }

    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

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

    /// Set the initial identity after all startup API sources have been consulted.
    pub fn initialize_identity(&self, device_id: &str, name: String, room: Option<String>) {
        if let Some(device) = self.inner.write().devices.get_mut(device_id) {
            device.name = name;
            device.room = room;
        }
    }

    /// Persist newly discovered devices, coalescing pending writes. Status responses
    /// update the LAN timestamp in memory without triggering writes on every packet.
    pub fn handle_lan_discovery(&self, device_id: &str, sku: &str) -> PersistedDevice {
        let (device, is_new) = {
            let mut db = self.inner.write();
            let is_new = !db.contains(device_id);
            (db.handle_lan_discovery(device_id, sku).clone(), is_new)
        };
        if is_new {
            self.request_save();
        }
        device
    }

    pub fn list_devices(&self) -> Vec<PersistedDevice> {
        self.inner.read().devices.values().cloned().collect()
    }

    #[cfg(test)]
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
    fn database_roundtrip_preserves_the_json_contract() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test_devices.json");

        let mut db = DeviceDatabase::default();
        db.handle_lan_discovery("0022", "H6072");
        db.update_from_api(
            "0011",
            "H6072",
            "Test Light",
            Some("Living Room"),
            DiscoverySource::PlatformApi,
        );
        db.update_from_api("0011", "H6072", "App name", None, DiscoverySource::UndocApi);
        let fixed_time = "2025-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        for device in db.devices.values_mut() {
            device.first_seen = fixed_time;
            for info in device.api_info.values_mut() {
                info.last_sync = fixed_time;
            }
        }

        db.save(&path).unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        k9::snapshot!(
            k9::MultilineString(contents.clone()),
            r#"
{
  "version": 2,
  "devices": {
    "0011": {
      "id": "0011",
      "sku": "H6072",
      "name": "Test Light",
      "room": "Living Room",
      "first_seen": "2025-01-01T00:00:00Z",
      "api_info": {
        "platform_api": {
          "name": "Test Light",
          "room": "Living Room",
          "last_sync": "2025-01-01T00:00:00Z"
        },
        "undoc_api": {
          "name": "App name",
          "last_sync": "2025-01-01T00:00:00Z"
        }
      }
    },
    "0022": {
      "id": "0022",
      "sku": "H6072",
      "name": "H6072_0022",
      "first_seen": "2025-01-01T00:00:00Z",
      "api_info": {
        "lan": {
          "last_sync": "2025-01-01T00:00:00Z"
        }
      }
    }
  }
}
"#
        );
        let loaded = DeviceDatabase::load(&path).unwrap();
        k9::assert_equal!(serde_json::to_string_pretty(&loaded).unwrap(), contents);
    }

    #[test]
    fn api_enriches_a_lan_device_without_changing_its_identity() {
        let mut db = DeviceDatabase::default();

        db.handle_lan_discovery("AA:BB:CC:DD:EE:FF:00:11", "H6072");

        db.update_from_api(
            "AA:BB:CC:DD:EE:FF:00:11",
            "H6072",
            "Kitchen Light",
            Some("Kitchen"),
            DiscoverySource::PlatformApi,
        );

        let device = db.get("AA:BB:CC:DD:EE:FF:00:11").unwrap();
        assert_eq!(device.name, "H6072_0011");
        assert_eq!(device.room, None);
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

        assert_eq!(
            StartupMode::detect(&db_path, &cache_path),
            StartupMode::FreshInstall
        );

        std::fs::write(&cache_path, "").unwrap();
        assert_eq!(
            StartupMode::detect(&db_path, &cache_path),
            StartupMode::Upgrade
        );

        std::fs::write(&db_path, "{}").unwrap();
        assert_eq!(
            StartupMode::detect(&db_path, &cache_path),
            StartupMode::Normal
        );
    }

    #[test]
    fn last_api_sync_uses_the_newest_source_timestamp() {
        let older = "2025-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let newer = "2025-01-02T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let mut device = PersistedDevice::new_minimal("0011", "H6072", DiscoverySource::Lan);
        device
            .api_info
            .get_mut(&DiscoverySource::Lan)
            .unwrap()
            .last_sync = newer;
        device.api_info.insert(
            DiscoverySource::PlatformApi,
            PersistedApiInfo {
                name: None,
                room: None,
                last_sync: older,
            },
        );

        assert_eq!(device.last_api_sync(), Some(newer));
    }

    #[test]
    fn api_refresh_preserves_user_edits_and_tracks_each_source() {
        let mut db = DeviceDatabase::default();
        db.update_from_api(
            "0011",
            "H6072",
            "Cloud",
            Some("Old room"),
            DiscoverySource::PlatformApi,
        );
        let device = db.devices.get_mut("0011").unwrap();
        device.name = "Chosen name".into();
        device.room = None;

        db.update_from_api(
            "0011",
            "H6072",
            "Different cloud",
            Some("New room"),
            DiscoverySource::UndocApi,
        );

        assert_eq!(db.get_display_name("0011"), Some("Chosen name"));
        assert_eq!(db.get_room("0011"), None);
        let info = &db.get("0011").unwrap().api_info[&DiscoverySource::UndocApi];
        assert_eq!(info.name.as_deref(), Some("Different cloud"));
        assert_eq!(info.room.as_deref(), Some("New room"));
    }

    #[test]
    fn empty_database_path_is_rejected() {
        assert!(DeviceDatabaseHandle::open(PathBuf::new()).is_err());
    }

    #[test]
    fn future_and_malformed_databases_remain_untouched() {
        let dir = TempDir::new().unwrap();
        for (index, contents) in [
            r#"{"version":999,"devices":{},"future":"preserve"}"#,
            "{broken",
        ]
        .iter()
        .enumerate()
        {
            let path = dir.path().join(format!("{index}.json"));
            fs::write(&path, contents).unwrap();
            assert!(DeviceDatabaseHandle::open(path.clone()).is_err());
            assert_eq!(fs::read_to_string(path).unwrap(), *contents);
        }
    }

    #[test]
    fn older_databases_preserve_explicit_user_overrides() {
        let dir = TempDir::new().unwrap();
        for version in [0, 1] {
            let path = dir.path().join(format!("{version}.json"));
            let legacy = serde_json::json!({
                "version": version,
                "devices": {
                    "0011": {
                        "id": "0011",
                        "sku": "H6072",
                        "name": "Cloud",
                        "room": "Cloud room",
                        "user_name": "Chosen",
                        "user_room": "Kitchen",
                        "first_seen": "2025-01-01T00:00:00Z",
                        "api_info": {
                            "platform_api": {
                                "name": "Cloud",
                                "last_sync": "2025-01-02T00:00:00Z"
                            }
                        }
                    }
                }
            });
            fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();

            let loaded = DeviceDatabase::load(&path).unwrap();

            k9::snapshot!(
                k9::MultilineString(serde_json::to_string_pretty(&loaded).unwrap()),
                r#"
{
  "version": 2,
  "devices": {
    "0011": {
      "id": "0011",
      "sku": "H6072",
      "name": "Chosen",
      "room": "Kitchen",
      "first_seen": "2025-01-01T00:00:00Z",
      "api_info": {
        "platform_api": {
          "name": "Cloud",
          "last_sync": "2025-01-02T00:00:00Z"
        }
      }
    }
  }
}
"#
            );
        }
    }

    #[test]
    fn database_read_failure_identifies_the_path_and_preserves_the_cause() {
        let dir = TempDir::new().unwrap();

        let err = DeviceDatabase::load(dir.path()).unwrap_err();

        assert_eq!(
            err.to_string(),
            format!("Reading device database {:?}", dir.path())
        );
        assert!(err.downcast_ref::<std::io::Error>().is_some());
    }

    #[tokio::test]
    async fn lan_discovery_is_saved_without_an_api_or_explicit_save() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("devices.json");
        let db = DeviceDatabaseHandle::open(path.clone()).unwrap();

        db.handle_lan_discovery("0011", "H6072");

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if DeviceDatabase::load(&path).unwrap().contains("0011") {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            DeviceDatabase::load(&path)
                .unwrap()
                .get_display_name("0011"),
            Some("H6072_0011")
        );
    }

    #[tokio::test]
    async fn queued_writes_preserve_the_latest_snapshot() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("devices.json");
        let db = DeviceDatabaseHandle::open(path.clone()).unwrap();
        let other = db.clone();
        for id in 0..100 {
            other.handle_lan_discovery(&format!("{id:04X}"), "H6072");
        }
        db.initialize_identity("0000", "Edited".into(), None);

        db.save().await.unwrap();

        let loaded = DeviceDatabase::load(&path).unwrap();
        assert_eq!(loaded.len(), 100);
        assert_eq!(loaded.get_display_name("0000"), Some("Edited"));
        assert_eq!(db.get_device("0000").unwrap().name, "Edited");
    }

    #[tokio::test]
    async fn save_failure_is_reported_and_does_not_hold_the_data_lock() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("devices.json");
        let db = DeviceDatabaseHandle::open(path.clone()).unwrap();
        fs::create_dir(&path).unwrap(); // A directory cannot be replaced by the JSON file.
        db.handle_lan_discovery("0011", "H6072");

        let err = db.save().await.unwrap_err();

        assert_eq!(
            err.to_string(),
            format!("Replacing device database {path:?}")
        );
        assert!(err.downcast_ref::<tempfile::PersistError>().is_some());
        drop(err);
        assert_eq!(db.get_device("0011").unwrap().name, "H6072_0011");
        assert!(path.is_dir());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
