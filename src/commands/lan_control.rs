use crate::ble::GoveeBlePacket;
use crate::lan_api::{Client, DiscoOptions};
use crate::undoc_api::GoveeUndocumentedApi;
use clap_num::maybe_hex;
use std::collections::BTreeMap;
use std::net::IpAddr;
use uncased::Uncased;

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
    Scene {
        /// List available scenes
        #[arg(long)]
        list: bool,

        /// Name of a scene to activate
        #[arg(required_unless_present = "list")]
        scene: Option<String>,
    },
}

impl LanControlCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
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
                let client = args.undoc_args.api_client()?;
                let token = client.login_community().await?;
                let res = client.get_saved_one_click_shortcuts(&token).await?;
                println!("{res:#?}");
            }
            SubCommand::Scene { list, scene } => {
                let mut scene_code_by_name = BTreeMap::new();
                for category in GoveeUndocumentedApi::get_scenes_for_device(&device.sku).await? {
                    for scene in category.scenes {
                        for effect in scene.light_effects {
                            if effect.scene_code != 0 {
                                scene_code_by_name
                                    .insert(Uncased::new(scene.scene_name), effect.scene_code);
                                break;
                            }
                        }
                    }
                }
                if *list {
                    for name in scene_code_by_name.keys() {
                        println!("{name}");
                    }
                } else {
                    let scene = Uncased::new(scene.clone().expect("scene if not list"));
                    if let Some(code) = scene_code_by_name.get(&scene) {
                        let encoded = GoveeBlePacket::scene_code(*code).base64();
                        println!("Computed {encoded}");
                        device.send_real(vec![encoded]).await?;
                    } else {
                        anyhow::bail!("scene {scene} not found");
                    }
                }
            }
            SubCommand::Command { data } => {
                let encoded = GoveeBlePacket::with_bytes(data.to_vec()).finish().base64();
                println!("encoded: {encoded}");
                device.send_real(vec![encoded]).await?;
            }
        }

        Ok(())
    }
}
