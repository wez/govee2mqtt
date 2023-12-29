use crate::lan_api::Client;

#[derive(clap::Parser, Debug)]
pub struct LanDiscoCommand {}

impl LanDiscoCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
        let (client, mut scan) = Client::new().await?;

        while let Some(device) = scan.recv().await {
            log::info!("{device:?}");

            if let Ok(resp) = client.query_status(&device).await {
                log::info!("Got status: {resp:?}");
            }
        }
        Ok(())
    }
}
