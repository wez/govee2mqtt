use crate::lan_api::{Client, DiscoOptions};
use clap_num::maybe_hex;
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
    Brightness {
        percent: u8,
    },
    Temperature {
        kelvin: u32,
    },
    Color {
        color: csscolorparser::Color,
    },
    /// Send a BLE-encoded govee packet
    /// eg: `0x33 1 0` is power off, `0x33 1 1` is power on.
    /// More usefully: you can send scene or music mode commands
    /// this way.
    Command {
        #[arg(value_parser=maybe_hex::<u8>)]
        data: Vec<u8>,
    },
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
            SubCommand::Command { data } => {
                println!("data: {data:x?}");
                let mut data = data.to_vec();
                let mut checksum = 0u8;
                data.resize(19, 0);
                for &b in &data {
                    checksum = checksum ^ b;
                }
                data.push(checksum);
                println!("packet: {data:x?}");
                let encoded = data_encoding::BASE64.encode(&data);
                println!("encoded: {encoded}");
                device.send_real(&encoded).await?;
            }
        }

        Ok(())
    }
}
