use crate::device_database::{DeviceDatabase, DeviceDatabaseHandle, DiscoverySource, StartupMode};
use crate::lan_api::Client as LanClient;
use crate::platform_api::GoveeApiClient;
use crate::service::device::Device;
use crate::service::hass::spawn_hass_integration;
use crate::service::http::run_http_server;
use crate::service::iot::start_iot_client;
use crate::service::state::StateHandle;
use crate::undoc_api::GoveeUndocumentedApi;
use crate::version_info::govee_version;
use crate::UndocApiArguments;
use anyhow::Context;
use chrono::Utc;
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

pub static POLL_INTERVAL: Lazy<chrono::Duration> = Lazy::new(|| chrono::Duration::seconds(900));

#[derive(clap::Parser, Debug)]
pub struct ServeCommand {
    /// The port on which the HTTP API will listen
    #[arg(long, default_value_t = 8056)]
    http_port: u16,

    /// Path to the persistent device database file.
    /// May also be set with GOVEE_DEVICE_DB. Defaults to ~/.cache/govee2mqtt/devices.json.
    #[arg(long)]
    device_db: Option<PathBuf>,
}

async fn poll_single_device(state: &StateHandle, device: &Device) -> anyhow::Result<()> {
    let now = Utc::now();

    if device.is_ble_only_device() == Some(true) {
        // We can't poll this device, we have no ble support
        return Ok(());
    }

    // Collect the device status via the LAN API, if possible.
    // This is partially redundant with the LAN discovery task,
    // but the timing of that is not as regular and predictable
    // because it employs exponential backoff.
    // Some Govee devices have bad firmware that will cause the
    // lights to flicker about a minute after polling, so it
    // is desirable to keep polling on a regular basis.
    // <https://github.com/wez/govee2mqtt/issues/250>
    if let Some(lan_device) = &device.lan_device {
        if let Some(client) = state.get_lan_client().await {
            if let Ok(status) = client.query_status(lan_device).await {
                state
                    .device_mut(&lan_device.sku, &lan_device.device)
                    .await
                    .set_lan_device_status(status);
                state.notify_of_state_change(&lan_device.device).await.ok();
            }
        }
    }

    let poll_interval = device.preferred_poll_interval();

    let can_update = match &device.last_polled {
        None => true,
        Some(last) => now - last > poll_interval,
    };

    if !can_update {
        return Ok(());
    }

    let device_state = device.device_state();
    let needs_update = match &device_state {
        None => true,
        Some(state) => now - state.updated > poll_interval,
    };

    if !needs_update {
        return Ok(());
    }

    let needs_platform = device.needs_platform_poll();

    // Don't interrogate via HTTP if we can use the LAN.
    // If we have LAN and the device is stale, it is likely
    // offline and there is little sense in burning up request
    // quota to the platform API for it
    if device.lan_device.is_some() && !needs_platform {
        log::trace!("LAN-available device {device} needs a status update; it's likely offline.");
        return Ok(());
    }

    if !needs_platform && state.poll_iot_api(device).await? {
        return Ok(());
    }

    state.poll_platform_api(device).await?;

    Ok(())
}

async fn periodic_state_poll(state: StateHandle) -> anyhow::Result<()> {
    sleep(Duration::from_secs(20)).await;
    loop {
        for d in state.devices().await {
            if let Err(err) = poll_single_device(&state, &d).await {
                log::error!("while polling {d}: {err:#}");
            }
        }

        sleep(Duration::from_secs(30)).await;
    }
}

async fn enumerate_devices_via_platform_api(
    state: StateHandle,
    client: Option<GoveeApiClient>,
) -> anyhow::Result<()> {
    let initial_discovery = client.is_some();
    let client = match client {
        Some(client) => client,
        None => match state.get_platform_client().await {
            Some(client) => client,
            None => return Ok(()),
        },
    };

    log::info!("Querying platform API for device list");
    for info in client.get_devices().await? {
        // Update device database with API-discovered device
        if let Some(db) = state.get_device_database().await {
            db.update_from_api(
                &info.device,
                &info.sku,
                &info.device_name,
                None, // Platform API doesn't provide room info
                DiscoverySource::PlatformApi,
            );
        }

        let mut device = state.device_mut(&info.sku, &info.device).await;
        device.set_http_device_info(info);
    }

    if !initial_discovery {
        state.apply_device_database().await;
        if let Err(err) = state.save_device_database().await {
            log::warn!("Failed to save device database: {err:#}");
        }
    }

    Ok(())
}

async fn enumerate_devices_via_undo_api(
    state: StateHandle,
    client: Option<GoveeUndocumentedApi>,
    args: &UndocApiArguments,
) -> anyhow::Result<()> {
    let initial_discovery = client.is_some();
    let client = match client {
        Some(client) => client,
        None => match state.get_undoc_client().await {
            Some(client) => client,
            None => return Ok(()),
        },
    };

    log::info!("Querying undocumented API for device + room list");
    let acct = client.login_account_cached().await?;
    let info = client.get_device_list(&acct.token).await?;
    let mut group_by_id = HashMap::new();
    for group in info.groups {
        group_by_id.insert(group.group_id, group.group_name);
    }
    for entry in &info.devices {
        let room_name = group_by_id.get(&entry.group_id).map(|name| name.as_str());

        // Update device database with undoc API-discovered device
        // This provides room information that platform API doesn't have
        if let Some(db) = state.get_device_database().await {
            db.update_from_api(
                &entry.device,
                &entry.sku,
                &entry.device_name,
                room_name,
                DiscoverySource::UndocApi,
            );
        }

        let mut device = state.device_mut(&entry.sku, &entry.device).await;
        device.set_undoc_device_info(entry.clone(), room_name);
    }

    if !initial_discovery {
        state.apply_device_database().await;
        if let Err(err) = state.save_device_database().await {
            log::warn!("Failed to save device database: {err:#}");
        }
    }

    if state.get_iot_client().await.is_none() {
        start_iot_client(args, state.clone(), Some(acct)).await?;
    }
    Ok(())
}

async fn initialize_device_database(
    state: &StateHandle,
    db: &DeviceDatabaseHandle,
    initial_device_ids: &HashSet<String>,
) -> anyhow::Result<()> {
    for device in state.devices().await {
        if !initial_device_ids.contains(&device.id) {
            db.initialize_identity(
                &device.id,
                device.name(),
                device.room_name().map(str::to_string),
            );
        }
    }
    state.apply_device_database().await;
    state.save_device_database().await
}

async fn register_recovered_devices(state: &StateHandle) -> bool {
    let Some(hass) = state.get_hass_client().await else {
        return false;
    };
    match hass.register_with_hass(state).await {
        Ok(()) => true,
        Err(err) => {
            log::warn!("Registering devices after API recovery: {err:#}");
            false
        }
    }
}

impl ServeCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
        log::info!("Starting service. version {}", govee_version());
        let state = Arc::new(crate::service::state::State::new());

        // Initialize the device database for persistent device storage
        let db_path = match &self.device_db {
            Some(path) => path.clone(),
            None => {
                crate::opt_env_var("GOVEE_DEVICE_DB")?.unwrap_or_else(DeviceDatabase::default_path)
            }
        };
        let cache_path = crate::cache::cache_file_name();
        let startup_mode = StartupMode::detect(&db_path, &cache_path);

        log::info!("Device database path: {:?}", db_path);
        match &startup_mode {
            StartupMode::FreshInstall => {
                log::info!("Fresh install detected - no existing device data");
            }
            StartupMode::Upgrade => {
                log::info!(
                    "Upgrade detected - device database will be populated on next API success"
                );
            }
            StartupMode::Normal => {
                log::info!("Normal startup - loading existing device data");
            }
        }

        let device_db = DeviceDatabaseHandle::open(db_path.clone())
            .context("Failed to open device database")?;

        state.set_device_database(device_db.clone()).await;

        // Record discovery start time to detect stale devices later
        let discovery_start = chrono::Utc::now();

        // Step 1: Always load devices from persistent database first
        // The database is the source of truth for device naming
        let initial_device_ids: HashSet<_> =
            device_db.list_devices().into_iter().map(|d| d.id).collect();
        let initial_device_count = device_db.len();
        if initial_device_count > 0 {
            log::info!(
                "Loading {} devices from persistent database",
                initial_device_count
            );
            for persisted in device_db.list_devices() {
                let mut device = state.device_mut(&persisted.sku, &persisted.id).await;
                device.name = Some(persisted.name.clone());
                device.room = persisted.room.clone();
            }
        }

        // Step 2: Attempt API discovery to refresh/update device metadata
        // API updates will set last_api_sync timestamps

        if let Ok(client) = args.api_args.api_client() {
            state.set_platform_client(client.clone()).await;
            let result = enumerate_devices_via_platform_api(state.clone(), Some(client)).await;
            let mut needs_registration = result.is_err();
            if let Err(err) = result {
                log::warn!(
                    "Platform API discovery failed: {err:#}; will retry during periodic discovery"
                );
            }
            let state = state.clone();
            tokio::spawn(async move {
                loop {
                    sleep(Duration::from_secs(600)).await;
                    if let Err(err) = enumerate_devices_via_platform_api(state.clone(), None).await
                    {
                        needs_registration = true;
                        log::error!("Error during periodic platform API discovery: {err:#}");
                    } else if needs_registration {
                        needs_registration = !register_recovered_devices(&state).await;
                    }
                }
            });
        }

        if let Ok(client) = args.undoc_args.api_client() {
            state.set_undoc_client(client.clone()).await;
            let result =
                enumerate_devices_via_undo_api(state.clone(), Some(client), &args.undoc_args).await;
            let mut needs_registration = result.is_err();
            if let Err(err) = result {
                log::warn!(
                    "Undoc API discovery failed: {err:#}; will retry during periodic discovery"
                );
            }
            let state = state.clone();
            let args = args.undoc_args.clone();
            tokio::spawn(async move {
                loop {
                    sleep(Duration::from_secs(600)).await;
                    if let Err(err) =
                        enumerate_devices_via_undo_api(state.clone(), None, &args).await
                    {
                        needs_registration = true;
                        log::error!("Error during periodic undoc API discovery: {err:#}");
                    } else if needs_registration {
                        needs_registration = !register_recovered_devices(&state).await;
                    }
                }
            });
        }

        // Preserve the pre-database naming rules on upgrade: Platform names and
        // Undoc rooms, or computed names without Platform metadata. Once stored,
        // these fields no longer change when a different API becomes available.
        if let Err(err) = initialize_device_database(&state, &device_db, &initial_device_ids).await
        {
            log::warn!("Failed to save device database: {err:#}");
        }

        // Step 3: Check which devices didn't get API updates during discovery
        // These are using cached data from the persistent database
        let stale_devices: Vec<_> = device_db
            .list_devices()
            .into_iter()
            .filter(|d| {
                d.last_api_sync()
                    .map(|ts| ts < discovery_start)
                    .unwrap_or(true)
            })
            .collect();

        if !stale_devices.is_empty() {
            log::warn!(
                "{} device(s) using cached data (no recent API sync):",
                stale_devices.len()
            );
            for device in &stale_devices {
                log::warn!(
                    "  {} ({}) - last sync: {:?}",
                    device.name,
                    device.sku,
                    device.last_api_sync()
                );
            }
            log::warn!("LAN-capable devices should still be controllable");
        }

        // Now start LAN discovery

        let options = args.lan_disco_args.to_disco_options()?;
        if !options.is_empty() {
            log::info!("Starting LAN discovery");
            let state = state.clone();
            let db_for_lan = device_db.clone();
            let (client, mut scan) = LanClient::new(options).await?;

            state.set_lan_client(client.clone()).await;

            tokio::spawn(async move {
                while let Some(lan_device) = scan.recv().await {
                    log::trace!("LAN disco: {lan_device:?}");

                    // Update device database with LAN-discovered device
                    // This ensures LAN-only devices get persisted even without API
                    let persisted =
                        db_for_lan.handle_lan_discovery(&lan_device.device, &lan_device.sku);
                    {
                        let mut device =
                            state.device_mut(&lan_device.sku, &lan_device.device).await;
                        device.name = Some(persisted.name);
                        device.room = persisted.room;
                        device.set_lan_device(lan_device.clone());
                    }

                    let state = state.clone();
                    let client = client.clone();
                    tokio::spawn(async move {
                        if let Ok(status) = client.query_status(&lan_device).await {
                            state
                                .device_mut(&lan_device.sku, &lan_device.device)
                                .await
                                .set_lan_device_status(status);

                            log::trace!("LAN disco: update and notify {}", lan_device.device);
                            state.notify_of_state_change(&lan_device.device).await.ok();
                        }
                    });
                }
            });

            // I don't love that this is 10 seconds but since our timeout
            // for query_status is 10 seconds, and we show a warning for
            // devices that didn't respond in the section below, in the
            // interest of reducing false positives we need to wait long
            // enough to provide high-signal warnings.
            log::info!("Waiting 10 seconds for LAN API discovery");
            sleep(Duration::from_secs(10)).await;
        }

        log::info!("Devices returned from Govee's APIs");
        for device in state.devices().await {
            log::info!("{device}");
            if let Some(lan) = &device.lan_device {
                log::info!("  LAN API: ip={:?}", lan.ip);
            }
            if let Some(http_info) = &device.http_device_info {
                let kind = &http_info.device_type;
                let rgb = http_info.supports_rgb();
                let bright = http_info.supports_brightness();
                let color_temp = http_info.get_color_temperature_range();
                let segment_rgb = http_info.supports_segmented_rgb();
                log::info!(
                    "  Platform API: {kind}. supports_rgb={rgb} supports_brightness={bright}"
                );
                log::info!("                color_temp={color_temp:?} segment_rgb={segment_rgb:?}");
                log::trace!("{http_info:#?}");
            }
            if let Some(undoc) = &device.undoc_device_info {
                let room = &undoc.room_name;
                let supports_iot = undoc.entry.device_ext.device_settings.topic.is_some();
                let ble_only = undoc.entry.device_ext.device_settings.wifi_name.is_none();
                log::info!(
                    "  Undoc: room={room:?} supports_iot={supports_iot} ble_only={ble_only}"
                );
                log::trace!("{undoc:#?}");
            }
            if let Some(quirk) = device.resolve_quirk() {
                log::info!("  {quirk:?}");

                // Sanity check for LAN devices: if we don't see an API for it,
                // it may indicate a networking issue
                if quirk.lan_api_capable && device.lan_device.is_none() {
                    log::warn!(
                        "  This device should be available via the LAN API, \
                        but didn't respond to probing yet. Possible causes:"
                    );
                    log::warn!("  1) LAN API needs to be enabled in the Govee Home App.");
                    log::warn!("  2) The device is offline.");
                    log::warn!("  3) A network configuration issue is preventing communication.");
                    log::warn!(
                        "  4) The device needs a firmware update before it can enable LAN API."
                    );
                    log::warn!(
                        "  5) The hardware version of the device is too old to enable the LAN API."
                    );
                }
            } else if device.http_device_info.is_none() {
                log::warn!("  Unknown device type. Cannot map to Home Assistant.");
                if state.get_platform_client().await.is_none() {
                    log::warn!(
                        "  Recommendation: configure your Govee API Key so that \
                                  metadata can be fetched from Govee"
                    );
                }
            }

            log::info!("");
        }

        // Start periodic status polling
        {
            let state = state.clone();
            tokio::spawn(async move {
                if let Err(err) = periodic_state_poll(state).await {
                    log::error!("periodic_state_poll: {err:#}");
                }
            });
        }

        // start advertising on local mqtt
        spawn_hass_integration(state.clone(), &args.hass_args).await?;

        run_http_server(state.clone(), self.http_port)
            .await
            .with_context(|| format!("Starting HTTP service on port {}", self.http_port))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::platform_api::HttpDeviceInfo;
    use crate::undoc_api::DeviceEntry;

    fn api_metadata() -> (HttpDeviceInfo, DeviceEntry) {
        let platform: serde_json::Value =
            serde_json::from_str(include_str!("../../test-data/list_devices.json")).unwrap();
        let undoc: serde_json::Value =
            serde_json::from_str(include_str!("../../test-data/undoc-device-list.json")).unwrap();
        let mut info: HttpDeviceInfo = serde_json::from_value(platform["data"][0].clone()).unwrap();
        info.device_name = "Kitchen Light".into();
        let mut entry: DeviceEntry = serde_json::from_value(undoc["devices"][0].clone()).unwrap();
        entry.device = info.device.clone();
        entry.sku = info.sku.clone();
        (info, entry)
    }

    #[tokio::test]
    async fn first_discovery_and_offline_restart_keep_the_same_mqtt_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let db = DeviceDatabaseHandle::open(path.clone()).unwrap();
        let state = Arc::new(crate::service::state::State::new());
        state.set_device_database(db.clone()).await;
        let (info, entry) = api_metadata();
        let id = info.device.clone();
        db.update_from_api(
            &id,
            &info.sku,
            &info.device_name,
            None,
            DiscoverySource::PlatformApi,
        );
        db.update_from_api(
            &id,
            &entry.sku,
            &entry.device_name,
            Some("Kitchen"),
            DiscoverySource::UndocApi,
        );
        {
            let mut device = state.device_mut(&info.sku, &id).await;
            device.set_http_device_info(info);
            device.set_undoc_device_info(entry, Some("Kitchen"));
        }

        initialize_device_database(&state, &db, &HashSet::new())
            .await
            .unwrap();
        let original = state.device_by_id(&id).await.unwrap();
        let mqtt = crate::hass_mqtt::base::Device::for_device(&original);
        let restored = Arc::new(crate::service::state::State::new());
        restored
            .set_device_database(DeviceDatabaseHandle::open(path).unwrap())
            .await;
        restored.apply_device_database().await;
        let device = restored.device_by_id(&id).await.unwrap();
        let offline_mqtt = crate::hass_mqtt::base::Device::for_device(&device);

        assert_eq!(device.name(), "Kitchen Light");
        assert_eq!(device.room_name(), Some("Kitchen"));
        assert!(device.http_device_info.is_none());
        assert_eq!(offline_mqtt.identifiers, mqtt.identifiers);
        assert_eq!(offline_mqtt.name, mqtt.name);
        assert_eq!(offline_mqtt.suggested_area, mqtt.suggested_area);
    }

    #[tokio::test]
    async fn undoc_only_upgrade_preserves_the_previous_computed_name() {
        let dir = tempfile::tempdir().unwrap();
        let db = DeviceDatabaseHandle::open(dir.path().join("devices.json")).unwrap();
        let state = Arc::new(crate::service::state::State::new());
        state.set_device_database(db.clone()).await;
        let (_, entry) = api_metadata();
        let id = entry.device.clone();
        db.update_from_api(
            &id,
            &entry.sku,
            &entry.device_name,
            Some("Kitchen"),
            DiscoverySource::UndocApi,
        );
        let expected;
        {
            let mut device = state.device_mut(&entry.sku, &id).await;
            device.set_undoc_device_info(entry, Some("Kitchen"));
            expected = device.name();
        }

        initialize_device_database(&state, &db, &HashSet::new())
            .await
            .unwrap();

        assert_eq!(state.device_by_id(&id).await.unwrap().name(), expected);
        assert_eq!(db.get_device(&id).unwrap().name, expected);
        assert_eq!(db.get_device(&id).unwrap().room.as_deref(), Some("Kitchen"));
    }

    #[tokio::test]
    async fn api_recovery_preserves_a_lan_name_and_an_explicitly_cleared_room() {
        let dir = tempfile::tempdir().unwrap();
        let db = DeviceDatabaseHandle::open(dir.path().join("devices.json")).unwrap();
        let state = Arc::new(crate::service::state::State::new());
        state.set_device_database(db.clone()).await;
        let (info, entry) = api_metadata();
        let id = info.device.clone();
        db.handle_lan_discovery(&id, &info.sku);
        db.initialize_identity(&id, "Chosen name".into(), None);
        {
            let mut device = state.device_mut(&info.sku, &id).await;
            device.set_http_device_info(info.clone());
            device.set_undoc_device_info(entry, Some("Cloud room"));
        }
        db.update_from_api(
            &id,
            &info.sku,
            "New cloud name",
            Some("Cloud room"),
            DiscoverySource::PlatformApi,
        );

        state.apply_device_database().await;

        let device = state.device_by_id(&id).await.unwrap();
        assert_eq!(device.name(), "Chosen name");
        assert_eq!(device.room_name(), None);
        assert!(device.http_device_info.is_some());
    }
}
