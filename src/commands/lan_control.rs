use crate::ble::GoveeBlePacket;
use crate::lan_api::{Client, DiscoOptions};
use crate::undoc_api::GoveeUndocumentedApi;
use anyhow::Context;
use clap_num::maybe_hex;
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::Duration;
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
    Iot {},
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
            SubCommand::Iot {} => {
                let client = GoveeUndocumentedApi::new(
                    std::env::var("GOVEE_EMAIL")?,
                    std::env::var("GOVEE_PASSWORD")?,
                );
                let acct = client.login_account().await?;
                println!("{acct:#?}");
                let res = client.get_iot_key(&acct.token).await?;
                println!("{res:#?}");

                let key_bytes = data_encoding::BASE64.decode(res.p12.as_bytes())?;

                let container = p12::PFX::parse(&key_bytes).context("PFX::parse")?;
                for key in container.key_bags(&res.p12_pass).context("key_bags")? {
                    let priv_key =
                        openssl::pkey::PKey::private_key_from_der(&key).context("from_der")?;
                    let pem = priv_key
                        .private_key_to_pem_pkcs8()
                        .context("to_pem_pkcs8")?;
                    std::fs::write("/dev/shm/govee.iot.key", &pem)?;
                }
                for cert in container.cert_bags(&res.p12_pass).context("cert_bags")? {
                    let cert = openssl::x509::X509::from_der(&cert).context("x509 from der")?;
                    let pem = cert.to_pem().context("cert.to_pem")?;
                    std::fs::write("/dev/shm/govee.iot.cert", &pem)?;
                }

                let client = mosquitto_rs::Client::with_id(
                    &format!("AP/{account_id}/foo", account_id = acct.account_id),
                    true,
                )
                .context("new client")?;
                client
                    .configure_tls(
                        Some("AmazonRootCA1.pem"),
                        None::<&std::path::Path>,
                        Some("/dev/shm/govee.iot.cert"),
                        Some("/dev/shm/govee.iot.key"),
                        None,
                    )
                    .context("configure_tls")?;
                let status = client
                    .connect(&res.endpoint, 8883, Duration::from_secs(10), None)
                    .await
                    .context("connect")?;
                println!("Connection: {status:?}");

                let subscriptions = client.subscriber().expect("first and only");

                client
                    .subscribe(&acct.topic, mosquitto_rs::QoS::AtMostOnce)
                    .await?;

                while let Ok(msg) = subscriptions.recv().await {
                    let payload = String::from_utf8_lossy(&msg.payload);
                    println!("{} -> {payload}", msg.topic);
                }
            }
            SubCommand::ShowOneClick {} => {
                let client = GoveeUndocumentedApi::new(
                    std::env::var("GOVEE_EMAIL")?,
                    std::env::var("GOVEE_PASSWORD")?,
                );
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
