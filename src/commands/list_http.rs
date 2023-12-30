use crate::http_api::GoveeApiClient;
use anyhow::Context;

#[derive(clap::Parser, Debug)]
pub struct ListHttpCommand {}

impl ListHttpCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
        let key = std::env::var("GOVEE_API_KEY").context("obtaining GOVEE_API_KEY")?;
        let client = GoveeApiClient::new(key);
        let devices = client.get_devices().await?;
        for d in devices {
            println!(
                "{sku:<7} {id} {name}",
                sku = d.sku,
                id = d.device,
                name = d.device_name
            );

            /*
            let state = client.get_device_state(&d).await?;
            log::info!("state: {state:#?}");
            break;
            */
        }
        Ok(())
    }
}
