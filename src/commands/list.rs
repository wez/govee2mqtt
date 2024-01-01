use crate::lan_api::Client as LanClient;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

#[derive(clap::Parser, Debug)]
pub struct ListCommand {
    #[arg(long)]
    skip_lan: bool,
}

impl ListCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
        let state = Arc::new(crate::service::state::State::new());

        let options = args.lan_disco_args.to_disco_options();
        if options.is_empty() {
            anyhow::bail!("Discovery options are empty");
        }

        let disco = if self.skip_lan {
            None
        } else {
            eprintln!(
                "Waiting {} seconds for LAN discovery, use --skip-lan to skip...",
                args.lan_disco_args.disco_timeout
            );
            let deadline = Instant::now() + Duration::from_secs(args.lan_disco_args.disco_timeout);
            let state = state.clone();
            let (client, mut scan) = LanClient::new(options).await?;
            Some(tokio::spawn(async move {
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
            }))
        };

        if let Ok(client) = args.api_args.api_client() {
            for info in client.get_devices().await? {
                let mut device = state.device_mut(&info.sku, &info.device).await;
                device.set_http_device_info(info);
            }
        }

        if let Some(disco) = disco {
            disco.await?;
        }

        let mut devices = state.devices().await;
        devices.sort_by_key(|d| d.name());

        for d in devices {
            println!(
                "{sku:<7} {id} {ip:<15} {name}",
                sku = d.sku,
                id = d.id,
                ip = d
                    .ip_addr()
                    .map(|ip| ip.to_string())
                    .unwrap_or(String::new()),
                name = d.name()
            );
        }

        Ok(())
    }
}
