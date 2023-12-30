use crate::lan_api::{Client, DiscoOptions};
use std::net::IpAddr;

#[derive(clap::Parser, Debug)]
pub struct LanControlCommand {
    #[arg(long)]
    pub ip: IpAddr,

    #[command(subcommand)]
    cmd: SubCommand,
}

#[derive(clap::Parser, Debug)]
enum SubCommand {
    On,
    Off,
    Brightness { percent: u8 },
    Temperature { kelvin: u32 },
    Color { color: csscolorparser::Color },
}

impl LanControlCommand {
    pub async fn run(&self, _args: &crate::Args) -> anyhow::Result<()> {
        let (client, _scan) = Client::new(DiscoOptions::default()).await?;

        let device = client.scan_ip(self.ip).await?;

        match &self.cmd {
            SubCommand::On => {
                device.send_turn(true).await?;
            }
            SubCommand::Off => {
                device.send_turn(false).await?;
            }
            SubCommand::Brightness { percent } => {
                device.send_brightness(*percent).await?;
            }
            SubCommand::Temperature { kelvin } => {
                device.send_color_temperature_kelvin(*kelvin).await?;
            }
            SubCommand::Color { color } => {
                let [r, g, b, _a] = color.to_rgba8();
                device
                    .send_color_rgb(crate::lan_api::DeviceColor { r, g, b })
                    .await?;
            }
        }

        Ok(())
    }
}
