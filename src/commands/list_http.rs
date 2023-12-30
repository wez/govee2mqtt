#[derive(clap::Parser, Debug)]
pub struct ListHttpCommand {}

impl ListHttpCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
        let client = args.api_args.api_client()?;
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
