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
use std::collections::HashMap;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

pub const POLL_INTERVAL: Lazy<chrono::Duration> = Lazy::new(|| chrono::Duration::seconds(900));

/// Path to the SQLite cache file (for upgrade detection)
fn cache_file_path() -> std::path::PathBuf {
    let cache_dir = std::env::var("GOVEE_CACHE_DIR")
        .ok()
        .map(std::path::PathBuf::from)
        .or_else(|| dirs_next::cache_dir())
        .expect("failed to resolve cache dir");

    cache_dir.join("govee2mqtt-cache.sqlite")
}

#[derive(clap::Parser, Debug)]
pub struct ServeCommand {
    /// The port on which the HTTP API will listen
    #[arg(long, default_value_t = 8056)]
    http_port: u16,
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

    if !needs_platform {
        if state.poll_iot_api(&device).await? {
            return Ok(());
        }
    }

    state.poll_platform_api(&device).await?;

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

    // Save database after enumeration
    if let Err(err) = state.save_device_database().await {
        log::warn!("Failed to save device database: {err:#}");
    }

    Ok(())
}

async fn enumerate_devices_via_undo_api(
    state: StateHandle,
    client: Option<GoveeUndocumentedApi>,
    args: &UndocApiArguments,
) -> anyhow::Result<()> {
    let (client, needs_start) = match client {
        Some(client) => (client, true),
        None => match state.get_undoc_client().await {
            Some(client) => (client, false),
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
    for entry in info.devices.clone() {
        let room_name = group_by_id.get(&entry.group_id).map(|name| name.as_str());

        // Update device database with undoc API-discovered device
        // This provides room information that platform API doesn't have
        if let Some(db) = state.get_device_database().await {
            // Use device name from undoc API if available
            let device_name = entry.device_name.clone();
            db.update_from_api(
                &entry.device,
                &entry.sku,
                &device_name,
                room_name,
                DiscoverySource::UndocApi,
            );
        }

        let mut device = state.device_mut(&entry.sku, &entry.device).await;
        device.set_undoc_device_info(entry, room_name);
    }

    // Save database after enumeration
    if let Err(err) = state.save_device_database().await {
        log::warn!("Failed to save device database: {err:#}");
    }

    if needs_start {
        start_iot_client(args, state.clone(), Some(acct)).await?;
    }
    Ok(())
}

const ISSUE_76_EXPLANATION: &str = "Startup cannot automatically continue because entity names\n\
    could become inconsistent especially across frequent similar\n\
    intermittent issues if/as they occur on an ongoing basis.\n\
    Please see https://github.com/wez/govee2mqtt/issues/76\n\
    A workaround is to remove the Govee API credentials from your\n\
    configuration, which will cause this govee2mqtt to use only\n\
    the LAN API. Two consequences of that will be loss of control\n\
    over devices that do not support the LAN API, and also devices\n\
    changing entity ID to less descriptive names due to lack of\n\
    metadata availability via the LAN API.";

impl ServeCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
        log::info!("Starting service. version {}", govee_version());
        let state = Arc::new(crate::service::state::State::new());

        // Initialize the device database for persistent device storage
        let db_path = DeviceDatabase::default_path();
        let cache_path = cache_file_path();
        let startup_mode = StartupMode::detect(&db_path, &cache_path);

        log::info!("Device database path: {:?}", db_path);
        match &startup_mode {
            StartupMode::FreshInstall => {
                log::info!("Fresh install detected - no existing device data");
            }
            StartupMode::Upgrade => {
                log::info!("Upgrade detected - device database will be populated on next API success");
            }
            StartupMode::Normal => {
                log::info!("Normal startup - loading existing device data");
            }
        }

        let device_db = DeviceDatabaseHandle::open(Some(db_path.clone()))
            .context("Failed to open device database")?;

        let cached_device_count = device_db.len();
        if cached_device_count > 0 {
            log::info!(
                "Loaded {} devices from persistent database",
                cached_device_count
            );
        }

        state.set_device_database(device_db.clone()).await;

        // Track whether API discovery succeeded
        let mut api_discovery_ok = false;

        // First, use the HTTP APIs to determine the list of devices and
        // their names.

        if let Ok(client) = args.api_args.api_client() {
            match enumerate_devices_via_platform_api(state.clone(), Some(client.clone())).await {
                Ok(()) => {
                    api_discovery_ok = true;
                    // only record the client after we've completed the
                    // initial platform disco attempt
                    state.set_platform_client(client).await;

                    // spawn periodic discovery task
                    let state = state.clone();
                    tokio::spawn(async move {
                        loop {
                            sleep(Duration::from_secs(600)).await;
                            if let Err(err) =
                                enumerate_devices_via_platform_api(state.clone(), None).await
                            {
                                log::error!(
                                    "Error during periodic platform API discovery: {err:#}"
                                );
                            }
                        }
                    });
                }
                Err(err) => {
                    // Check if we can fall back to cached data
                    if cached_device_count > 0 {
                        log::warn!("Platform API discovery failed: {err:#}");
                        log::warn!(
                            "Continuing with {} cached devices from persistent database",
                            cached_device_count
                        );
                        // Don't set the platform client since it failed
                    } else {
                        // Fresh install with no cached data - we need API access
                        anyhow::bail!(
                            "Error during initial platform API discovery: {err:#}\n\
                            No cached device data available to fall back to.\n\
                            {ISSUE_76_EXPLANATION}"
                        );
                    }
                }
            }
        }

        if let Ok(client) = args.undoc_args.api_client() {
            match enumerate_devices_via_undo_api(
                state.clone(),
                Some(client.clone()),
                &args.undoc_args,
            )
            .await
            {
                Ok(()) => {
                    api_discovery_ok = true;
                    // only record the client after we've completed the
                    // initial undoc disco attempt
                    state.set_undoc_client(client).await;

                    // spawn periodic discovery task
                    let state = state.clone();
                    let args = args.undoc_args.clone();
                    tokio::spawn(async move {
                        loop {
                            sleep(Duration::from_secs(600)).await;
                            if let Err(err) =
                                enumerate_devices_via_undo_api(state.clone(), None, &args).await
                            {
                                log::error!("Error during periodic undoc API discovery: {err:#}");
                            }
                        }
                    });
                }
                Err(err) => {
                    // Check if we can fall back to cached data
                    if cached_device_count > 0 || api_discovery_ok {
                        log::warn!("Undoc API discovery failed: {err:#}");
                        if cached_device_count > 0 && !api_discovery_ok {
                            log::warn!(
                                "Continuing with {} cached devices from persistent database",
                                cached_device_count
                            );
                        }
                        // Continue with whatever data we have
                    } else {
                        // Fresh install with no cached data - we need API access
                        anyhow::bail!(
                            "Error during initial undoc API discovery: {err:#}\n\
                            No cached device data available to fall back to.\n\
                            {ISSUE_76_EXPLANATION}"
                        );
                    }
                }
            }
        }

        // If we're running with cached data only, populate the in-memory state
        // from the persistent database
        if !api_discovery_ok && cached_device_count > 0 {
            log::info!("Populating device state from persistent database...");
            for persisted in device_db.list_devices() {
                // Create a minimal Device entry for each cached device
                // This allows LAN discovery to work and match to known devices
                let mut device = state.device_mut(&persisted.sku, &persisted.id).await;

                // Populate cached name and room from persistent database
                // These will be used by HASS MQTT discovery when http_device_info is unavailable
                device.cached_name = Some(persisted.name.clone());
                device.cached_room = persisted.room.clone();
            }
            log::info!(
                "Loaded {} devices into memory from persistent database",
                cached_device_count
            );
            log::warn!("Running in degraded mode - device metadata may be stale");
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
                    let _display_name =
                        db_for_lan.handle_lan_discovery(&lan_device.device, &lan_device.sku);

                    state
                        .device_mut(&lan_device.sku, &lan_device.device)
                        .await
                        .set_lan_device(lan_device.clone());

                    let state = state.clone();
                    let client = client.clone();
                    let db_for_status = db_for_lan.clone();
                    tokio::spawn(async move {
                        if let Ok(status) = client.query_status(&lan_device).await {
                            state
                                .device_mut(&lan_device.sku, &lan_device.device)
                                .await
                                .set_lan_device_status(status);

                            log::trace!("LAN disco: update and notify {}", lan_device.device);
                            state.notify_of_state_change(&lan_device.device).await.ok();

                            // Save database periodically on LAN updates
                            if let Err(err) = db_for_status.save() {
                                log::trace!("Failed to save device database: {err:#}");
                            }
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
