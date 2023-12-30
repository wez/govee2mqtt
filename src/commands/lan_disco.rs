use crate::lan_api::Client;
use std::collections::HashSet;
use tokio::time::{Duration, Instant};

#[derive(clap::Parser, Debug)]
pub struct LanDiscoCommand {}

impl LanDiscoCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
        let options = args.lan_disco_args.to_disco_options();
        if options.is_empty() {
            anyhow::bail!("Discovery options are empty");
        }

        let (client, mut scan) = Client::new(options).await?;

        let deadline = Instant::now() + Duration::from_secs(args.lan_disco_args.disco_timeout);

        let mut devices = HashSet::new();

        while let Ok(Some(device)) = tokio::time::timeout_at(deadline, scan.recv()).await {
            if !devices.contains(&device) {
                let status = match client.query_status(&device).await {
                    Ok(status) => {
                        if status.on {
                            format!(
                                "{pct}% #{r:02x}{g:02x}{b:02x} {k}k",
                                pct = status.brightness,
                                r = status.color.r,
                                g = status.color.g,
                                b = status.color.b,
                                k = status.color_temperature_kelvin
                            )
                        } else {
                            "off".to_string()
                        }
                    }
                    Err(err) => format!("{err:#}"),
                };

                println!(
                    "{ip:<15} {sku:<7} {id} {status}",
                    ip = device.ip,
                    sku = device.sku,
                    id = device.device
                );

                devices.insert(device);
            }
        }
        Ok(())
    }
}
