use crate::http_api::GoveeApiClient;
use anyhow::Context;

#[derive(clap::Parser, Debug)]
pub struct ListHttpCommand {}

impl ListHttpCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
        let key = std::env::var("GOVEE_API_KEY").context("obtaining GOVEE_API_KEY")?;
        let client = GoveeApiClient::new(key);
        let devices = client.get_devices().await?;
        Ok(())
    }
}
