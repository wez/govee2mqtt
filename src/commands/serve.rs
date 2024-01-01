use crate::service::http::spawn_http_server;
use crate::lan_api::Client as LanClient;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

#[derive(clap::Parser, Debug)]
pub struct ServeCommand {
    /// The port on which the HTTP API will listen
    #[arg(long, default_value_t = 8056)]
    http_port: u16,
}

impl ServeCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
        let state = Arc::new(crate::service::state::State::new());

        // First, use the HTTP APIs to determine the list of devices and
        // their names.

        if let Ok(client) = args.api_args.api_client() {
            log::info!("Querying platform API for device list");
            for info in client.get_devices().await? {
                let mut device = state.device_mut(&info.sku, &info.device).await;
                device.set_http_device_info(info);
            }
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
        }

        let options = args.lan_disco_args.to_disco_options();
        if !options.is_empty() {
            log::info!("Starting LAN discovery");
            let deadline = Instant::now() + Duration::from_secs(args.lan_disco_args.disco_timeout);
            let state = state.clone();
            let (client, mut scan) = LanClient::new(options).await?;
            tokio::spawn(async move {
                while let Ok(Some(lan_device)) =
                    tokio::time::timeout_at(deadline, scan.recv()).await
                {
                    state
                        .device_mut(&lan_device.sku, &lan_device.device)
                        .await
                        .set_lan_device(lan_device.clone());

                    if let Ok(status) = client.query_status(&lan_device).await {
                        state
                            .device_mut(&lan_device.sku, &lan_device.device)
                            .await
                            .set_lan_device_status(status);
                    }
                }
            });
        }

        spawn_http_server(state.clone(), self.http_port).await?;

        tokio::time::sleep(Duration::from_secs(86400)).await; // FIXME: wait for other stuff

        Ok(())
    }
}
