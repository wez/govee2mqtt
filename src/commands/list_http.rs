#[derive(clap::Parser, Debug)]
pub struct ListHttpCommand {}

impl ListHttpCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
        let client = args.api_args.api_client()?;
        let state = crate::service::state::State::new();

        for info in client.get_devices().await? {
            let mut device = state.device_mut(&info.sku, &info.device).await;
            device.set_http_device_info(info);
        }

        for d in state.devices().await {
            println!(
                "{sku:<7} {id} {name}",
                sku = d.sku,
                id = d.id,
                name = d.name()
            );
        }

        Ok(())
    }
}
