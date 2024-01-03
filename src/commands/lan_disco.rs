use crate::lan_api::Client;
use tokio::time::{Duration, Instant};

#[derive(clap::Parser, Debug)]
pub struct LanDiscoCommand {}

impl LanDiscoCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
        let options = args.lan_disco_args.to_disco_options()?;
        if options.is_empty() {
            anyhow::bail!("Discovery options are empty");
        }

        let (client, mut scan) = Client::new(options).await?;

        let deadline = Instant::now() + Duration::from_secs(args.lan_disco_args.disco_timeout()?);

        let state = crate::service::state::State::new();

        while let Ok(Some(lan_device)) = tokio::time::timeout_at(deadline, scan.recv()).await {
            if !state.device_by_id(&lan_device.device).await.is_some() {
                let mut device = state.device_mut(&lan_device.sku, &lan_device.device).await;

                device.set_lan_device(lan_device.clone());

                let status = match client.query_status(&lan_device).await {
                    Ok(status) => {
                        device.set_lan_device_status(status.clone());
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
                    "{ip:<15} {name:<10} {id} {status}",
                    ip = lan_device.ip,
                    name = device.computed_name(),
                    id = device.id,
                );
            }
        }
        Ok(())
    }
}
