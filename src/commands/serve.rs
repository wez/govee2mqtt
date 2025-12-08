use crate::api_lockout::{
    clear_api_lockout, get_api_lockout, is_recoverable_error, set_api_lockout, ApiLockout,
};
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::time::{sleep, Duration};

pub const POLL_INTERVAL: Lazy<chrono::Duration> = Lazy::new(|| chrono::Duration::seconds(900));

/// Flag to track if we're running in degraded (LAN-only) mode
static DEGRADED_MODE: AtomicBool = AtomicBool::new(false);

/// Check if we're currently in degraded mode
pub fn is_degraded_mode() -> bool {
    DEGRADED_MODE.load(Ordering::Relaxed)
}

#[derive(clap::Parser, Debug)]
pub struct ServeCommand {
    /// The port on which the HTTP API will listen
    #[arg(long, default_value_t = 8056)]
    http_port: u16,

    /// Behavior when API is unavailable: 'lan' to continue with LAN-only,
    /// 'fail' to exit (original behavior)
    #[arg(long, default_value = "lan")]
    api_fallback_mode: String,
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

    // Skip cloud API polling if in degraded mode
    if is_degraded_mode() {
        log::trace!("Skipping cloud API poll for {device} - running in degraded mode");
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
        let mut device = state.device_mut(&info.sku, &info.device).await;
        device.set_http_device_info(info);
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
    for entry in info.devices {
        let mut device = state.device_mut(&entry.sku, &entry.device).await;
        let room_name = group_by_id.get(&entry.group_id).map(|name| name.as_str());
        device.set_undoc_device_info(entry, room_name);
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

const DEGRADED_MODE_NOTICE: &str = "\n\
    ========================================================================\n\
    RUNNING IN DEGRADED MODE (LAN-ONLY)\n\
    ========================================================================\n\
    The Govee cloud API is currently unavailable. This service will continue\n\
    operating with LAN-capable devices only. Cloud-only devices will not be\n\
    controllable until API access is restored.\n\
    \n\
    Possible causes:\n\
    - Account locked due to too many login attempts (24h cooldown)\n\
    - Rate limit exceeded (daily limit)\n\
    - Network connectivity issues\n\
    - Govee API service outage\n\
    \n\
    The service will automatically attempt to restore API access when the\n\
    lockout period expires.\n\
    ========================================================================";

/// Spawn a background task to periodically check if API access can be restored
async fn spawn_recovery_checker(
    state: StateHandle,
    platform_client: Option<GoveeApiClient>,
    undoc_client: Option<GoveeUndocumentedApi>,
    undoc_args: UndocApiArguments,
) {
    tokio::spawn(async move {
        // Check every 30 minutes
        let check_interval = Duration::from_secs(30 * 60);

        loop {
            sleep(check_interval).await;

            // Check if we're still locked out
            if let Some(lockout) = get_api_lockout().await {
                if lockout.is_active() {
                    if let Some(remaining) = lockout.time_remaining() {
                        log::info!(
                            "API recovery check: still locked out for {} more minutes ({})",
                            remaining.num_minutes(),
                            lockout.lockout_type
                        );
                    }
                    continue;
                }
            }

            log::info!("API lockout expired, attempting to restore API access...");

            // Try platform API first
            let mut api_restored = false;
            if let Some(ref client) = platform_client {
                match client.get_devices().await {
                    Ok(devices) => {
                        log::info!(
                            "Platform API access restored! Found {} devices",
                            devices.len()
                        );

                        // Update device state with fresh data
                        for info in devices {
                            let mut device = state.device_mut(&info.sku, &info.device).await;
                            device.set_http_device_info(info);
                        }

                        state.set_platform_client(client.clone()).await;
                        api_restored = true;
                    }
                    Err(err) => {
                        log::warn!("Platform API still unavailable: {err:#}");
                        if is_recoverable_error(&err) {
                            let lockout = ApiLockout::from_error(&err);
                            if let Err(e) = set_api_lockout(&lockout).await {
                                log::error!("Failed to set lockout state: {e:#}");
                            }
                        }
                    }
                }
            }

            // Try undoc API if platform succeeded or wasn't configured
            if let Some(ref client) = undoc_client {
                match client.login_account_cached().await {
                    Ok(acct) => {
                        match client.get_device_list(&acct.token).await {
                            Ok(info) => {
                                log::info!(
                                    "Undoc API access restored! Found {} devices in {} rooms",
                                    info.devices.len(),
                                    info.groups.len()
                                );

                                let mut group_by_id = HashMap::new();
                                for group in info.groups {
                                    group_by_id.insert(group.group_id, group.group_name);
                                }
                                for entry in info.devices {
                                    let room_name =
                                        group_by_id.get(&entry.group_id).map(|n| n.as_str());
                                    let mut device =
                                        state.device_mut(&entry.sku, &entry.device).await;
                                    device.set_undoc_device_info(entry, room_name);
                                }

                                state.set_undoc_client(client.clone()).await;

                                // Restart IoT client
                                if let Err(e) =
                                    start_iot_client(&undoc_args, state.clone(), Some(acct)).await
                                {
                                    log::error!("Failed to restart IoT client: {e:#}");
                                }

                                api_restored = true;
                            }
                            Err(err) => {
                                log::warn!("Undoc API device list failed: {err:#}");
                            }
                        }
                    }
                    Err(err) => {
                        log::warn!("Undoc API login still unavailable: {err:#}");
                        if is_recoverable_error(&err) {
                            let lockout = ApiLockout::from_error(&err);
                            if let Err(e) = set_api_lockout(&lockout).await {
                                log::error!("Failed to set lockout state: {e:#}");
                            }
                        }
                    }
                }
            }

            if api_restored {
                // Clear lockout and exit degraded mode
                if let Err(e) = clear_api_lockout() {
                    log::error!("Failed to clear lockout state: {e:#}");
                }
                DEGRADED_MODE.store(false, Ordering::Relaxed);
                log::info!("Exited degraded mode - full API access restored");

                // Don't break - keep monitoring in case we get locked out again
            }
        }
    });
}

impl ServeCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
        log::info!("Starting service. version {}", govee_version());
        let state = Arc::new(crate::service::state::State::new());

        let fallback_enabled = std::env::var("GOVEE_API_FALLBACK_MODE")
            .unwrap_or_else(|_| self.api_fallback_mode.clone())
            .to_lowercase() != "fail";

        // Check for existing lockout state from previous run
        if let Some(lockout) = get_api_lockout().await {
            if lockout.is_active() {
                if let Some(remaining) = lockout.time_remaining() {
                    log::warn!(
                        "Previous API lockout still active: {} ({} minutes remaining)",
                        lockout.lockout_type,
                        remaining.num_minutes()
                    );
                    log::warn!("Last error: {}", lockout.last_error);

                    if fallback_enabled {
                        log::warn!("Will start in degraded (LAN-only) mode");
                        DEGRADED_MODE.store(true, Ordering::Relaxed);
                    }
                }
            } else {
                log::info!("Previous API lockout has expired, will attempt normal startup");
                if let Err(e) = clear_api_lockout() {
                    log::warn!("Failed to clear expired lockout: {e:#}");
                }
            }
        }

        // Track API clients for recovery checker
        let mut platform_client_for_recovery: Option<GoveeApiClient> = None;
        let mut undoc_client_for_recovery: Option<GoveeUndocumentedApi> = None;
        let mut entered_degraded_mode = false;

        // First, use the HTTP APIs to determine the list of devices and
        // their names.

        if let Ok(client) = args.api_args.api_client() {
            // Skip API call if already in degraded mode from previous lockout
            if !is_degraded_mode() {
                match enumerate_devices_via_platform_api(state.clone(), Some(client.clone())).await
                {
                    Ok(()) => {
                        // Success - record the client
                        state.set_platform_client(client.clone()).await;
                        platform_client_for_recovery = Some(client.clone());

                        // spawn periodic discovery task
                        let state_clone = state.clone();
                        tokio::spawn(async move {
                            loop {
                                sleep(Duration::from_secs(600)).await;
                                if is_degraded_mode() {
                                    log::trace!(
                                        "Skipping periodic platform API discovery - degraded mode"
                                    );
                                    continue;
                                }
                                if let Err(err) =
                                    enumerate_devices_via_platform_api(state_clone.clone(), None)
                                        .await
                                {
                                    log::error!(
                                        "Error during periodic platform API discovery: {err:#}"
                                    );
                                }
                            }
                        });
                    }
                    Err(err) => {
                        if fallback_enabled && is_recoverable_error(&err) {
                            log::error!("Platform API error: {err:#}");
                            log::warn!("Recoverable error detected, entering degraded mode");

                            let lockout = ApiLockout::from_error(&err);
                            if let Err(e) = set_api_lockout(&lockout).await {
                                log::error!("Failed to set lockout state: {e:#}");
                            }

                            DEGRADED_MODE.store(true, Ordering::Relaxed);
                            entered_degraded_mode = true;
                            platform_client_for_recovery = Some(client.clone());
                        } else {
                            anyhow::bail!(
                                "Error during initial platform API discovery: {err:#}\n{ISSUE_76_EXPLANATION}"
                            );
                        }
                    }
                }
            } else {
                // Already in degraded mode, save client for recovery
                platform_client_for_recovery = Some(client.clone());
            }
        }

        if let Ok(client) = args.undoc_args.api_client() {
            // Skip API call if already in degraded mode
            if !is_degraded_mode() {
                match enumerate_devices_via_undo_api(
                    state.clone(),
                    Some(client.clone()),
                    &args.undoc_args,
                )
                .await
                {
                    Ok(()) => {
                        // Success - record the client
                        state.set_undoc_client(client.clone()).await;
                        undoc_client_for_recovery = Some(client.clone());

                        // spawn periodic discovery task
                        let state_clone = state.clone();
                        let args_clone = args.undoc_args.clone();
                        tokio::spawn(async move {
                            loop {
                                sleep(Duration::from_secs(600)).await;
                                if is_degraded_mode() {
                                    log::trace!(
                                        "Skipping periodic undoc API discovery - degraded mode"
                                    );
                                    continue;
                                }
                                if let Err(err) =
                                    enumerate_devices_via_undo_api(state_clone.clone(), None, &args_clone)
                                        .await
                                {
                                    log::error!("Error during periodic undoc API discovery: {err:#}");
                                }
                            }
                        });
                    }
                    Err(err) => {
                        if fallback_enabled && is_recoverable_error(&err) {
                            log::error!("Undoc API error: {err:#}");
                            log::warn!("Recoverable error detected, entering degraded mode");

                            let lockout = ApiLockout::from_error(&err);
                            if let Err(e) = set_api_lockout(&lockout).await {
                                log::error!("Failed to set lockout state: {e:#}");
                            }

                            DEGRADED_MODE.store(true, Ordering::Relaxed);
                            entered_degraded_mode = true;
                            undoc_client_for_recovery = Some(client.clone());
                        } else {
                            anyhow::bail!(
                                "Error during initial undoc API discovery: {err:#}\n{ISSUE_76_EXPLANATION}"
                            );
                        }
                    }
                }
            } else {
                // Already in degraded mode, save client for recovery
                undoc_client_for_recovery = Some(client.clone());
            }
        }

        // Log degraded mode notice if we just entered it
        if entered_degraded_mode || is_degraded_mode() {
            log::warn!("{DEGRADED_MODE_NOTICE}");
        }

        // Now start LAN discovery - this is critical and runs regardless of API status

        let options = args.lan_disco_args.to_disco_options()?;
        if !options.is_empty() {
            log::info!("Starting LAN discovery");
            let state = state.clone();
            let (client, mut scan) = LanClient::new(options).await?;

            state.set_lan_client(client.clone()).await;

            tokio::spawn(async move {
                while let Some(lan_device) = scan.recv().await {
                    log::trace!("LAN disco: {lan_device:?}");
                    state
                        .device_mut(&lan_device.sku, &lan_device.device)
                        .await
                        .set_lan_device(lan_device.clone());

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
        } else if is_degraded_mode() {
            log::error!(
                "CRITICAL: Running in degraded mode but LAN discovery is not configured!"
            );
            log::error!("No devices will be controllable until API access is restored.");
            log::error!("Consider configuring LAN discovery with --lan-disco-addr");
        }

        log::info!("Devices returned from Govee's APIs");
        let devices = state.devices().await;

        if devices.is_empty() && is_degraded_mode() {
            log::warn!("No devices found. In degraded mode, only LAN-discoverable devices will appear.");
            log::warn!("Devices will be added as they respond to LAN discovery probes.");
        }

        for device in devices {
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
                    if is_degraded_mode() {
                        log::warn!(
                            "  Note: Running in degraded mode. Device metadata unavailable."
                        );
                    } else {
                        log::warn!(
                            "  Recommendation: configure your Govee API Key so that \
                                      metadata can be fetched from Govee"
                        );
                    }
                }
            }

            log::info!("");
        }

        // Spawn recovery checker if in degraded mode
        if is_degraded_mode() {
            log::info!("Starting API recovery checker (will check every 30 minutes)");
            spawn_recovery_checker(
                state.clone(),
                platform_client_for_recovery,
                undoc_client_for_recovery,
                args.undoc_args.clone(),
            )
            .await;
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
