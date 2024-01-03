use crate::lan_api::Client as LanClient;
use crate::service::device::Device;
use crate::service::hass::spawn_hass_integration;
use crate::service::http::run_http_server;
use crate::service::iot::start_iot_client;
use crate::service::state::StateHandle;
use crate::version_info::govee_version;
use anyhow::Context;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::time::Duration;

#[derive(clap::Parser, Debug)]
pub struct ServeCommand {
    /// The port on which the HTTP API will listen
    #[arg(long, default_value_t = 8056)]
    http_port: u16,
}

async fn poll_single_device(state: &StateHandle, device: &Device) -> anyhow::Result<()> {
    let now = Utc::now();

    let can_update = match &device.last_polled {
        None => true,
        Some(last) => now - last > chrono::Duration::seconds(900),
    };

    if !can_update {
        return Ok(());
    }

    let device_state = device.device_state();
    let needs_update = match &device_state {
        None => true,
        Some(state) => now - state.updated > chrono::Duration::seconds(900),
    };

    if !needs_update {
        return Ok(());
    }

    // Don't interrogate via HTTP if we can use the LAN.
    // If we have LAN and the device is stale, it is likely
    // offline and there is little sense in burning up request
    // quota to the platform API for it
    if device.lan_device.is_some() {
        log::trace!("LAN-available device {device} needs a status update; it's likely offline.");
        return Ok(());
    }

    if let Some(iot) = state.get_iot_client().await {
        if let Some(info) = device.undoc_device_info.clone() {
            log::info!("requesting update via IoT MQTT {device} {device_state:?}");
            match iot
                .request_status_update(&info.entry)
                .await
            {
                Err(err) => {
                    log::error!("Failed: {err:#}");
                }
                Ok(()) => {
                    // The response will come in async via the mqtt loop in iot.rs
                    // However, if the device is offline, nothing will change our state.
                    // Let's explicitly mark the device as having been polled so that
                    // we don't keep sending a request every minute.
                    state
                        .device_mut(&device.sku, &device.id)
                        .await
                        .set_last_polled();

                    return Ok(());
                }
            }
        }
    }

    if let Some(client) = state.get_platform_client().await {
        log::info!("requesting update via Platform API {device} {device_state:?}");
        if let Some(info) = &device.http_device_info {
            let http_state = client
                .get_device_state(info)
                .await
                .context("get_device_state")?;
            log::trace!("updated state for {device}");

            {
                let mut device = state.device_mut(&device.sku, &device.id).await;
                device.set_http_device_state(http_state);
                device.set_last_polled();
            }
            state.notify_of_state_change(&device.id).await?;
        }
    } else {
        log::trace!(
            "device {device} needs a status update, but there is no platform client available"
        );
    }

    Ok(())
}

async fn periodic_state_poll(state: StateHandle) -> anyhow::Result<()> {
    tokio::time::sleep(Duration::from_secs(20)).await;
    loop {
        for d in state.devices().await {
            if let Err(err) = poll_single_device(&state, &d).await {
                log::error!("while polling {d}: {err:#}");
            }
        }

        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

impl ServeCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
        log::info!("Starting service. version {}", govee_version());
        let state = Arc::new(crate::service::state::State::new());

        // First, use the HTTP APIs to determine the list of devices and
        // their names.

        if let Ok(client) = args.api_args.api_client() {
            log::info!("Querying platform API for device list");
            for info in client.get_devices().await? {
                let mut device = state.device_mut(&info.sku, &info.device).await;
                device.set_http_device_info(info);
            }

            state.set_platform_client(client).await;
        }
        if let Ok(client) = args.undoc_args.api_client() {
            log::info!("Querying undocumented API for device + room list");
            let acct = client.login_account().await?;
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

            start_iot_client(args, state.clone()).await?;

            state.set_undoc_client(client).await;
        }

        // Now start discovery

        let options = args.lan_disco_args.to_disco_options()?;
        if !options.is_empty() {
            log::info!("Starting LAN discovery");
            let state = state.clone();
            let (client, mut scan) = LanClient::new(options).await?;

            state.set_lan_client(client.clone()).await;

            tokio::spawn(async move {
                while let Some(lan_device) = scan.recv().await {
                    state
                        .device_mut(&lan_device.sku, &lan_device.device)
                        .await
                        .set_lan_device(lan_device.clone());

                    if let Ok(status) = client.query_status(&lan_device).await {
                        state
                            .device_mut(&lan_device.sku, &lan_device.device)
                            .await
                            .set_lan_device_status(status);

                        log::trace!("LAN disco: update and notify {}", lan_device.device);
                        state.notify_of_state_change(&lan_device.device).await.ok();
                    }
                }
            });
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

        run_http_server(state.clone(), self.http_port).await
    }
}
