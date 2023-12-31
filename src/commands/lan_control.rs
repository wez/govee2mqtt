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
    ShowOneClick {},
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
            SubCommand::ShowOneClick {} => {
                use crate::undoc_api::GoveeUndocumentedApi;
                let client = GoveeUndocumentedApi::new(
                    std::env::var("GOVEE_EMAIL")?,
                    std::env::var("GOVEE_PASSWORD")?,
                );
                let token = client.login_community().await?;
                println!("{token:?}");
                client.get_saved_one_click_shortcuts(&token).await?;
            }
            SubCommand::Command { data } => {
                /*
                use crate::undoc_api::GoveeUndocumentedApi;
                let client = GoveeUndocumentedApi::new(
                    std::env::var("GOVEE_EMAIL")?,
                    std::env::var("GOVEE_PASSWORD")?,
                );
                */

                /*
                {
                let token = client.login_account().await?;
                client.get_device_list(&token).await?;
                return Ok(());
                }
                */

                /*
                 */

                /*
                let simple = vec!["MwUEMwgAAAAAAAAAAAAAAAAAAAk=".to_string()];

                let maybe = vec![
                    "owABCAIDGhQAAAEAAf//AAAAAKU=".to_string(),
                    "owEA/zIB//8AAAAAAAAAA0dQAHo=".to_string(),
                    "owIAEAAB//8AAAAAAvsUEP9/AM0=".to_string(),
                    "owP/fwD/AAD/AAD/FgD/FgD/AN8=".to_string(),
                    "owQA/38A/38A//8A//8A//8A/1g=".to_string(),
                    "owX/AP//AP//AP//AAAAAAAAAFk=".to_string(),
                    "owYCIGQAAAEAAf//AAD//wIAMtM=".to_string(),
                    "o/8DBoH+B7T/AAD/AAAAAAAAAZQ=".to_string(),
                ];
                device.send_real(maybe).await?;

                return Ok(());
                */

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
                device.send_real(vec![encoded]).await?;
            }
        }

        Ok(())
    }
}
