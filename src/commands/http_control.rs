use crate::http_api::{DeviceParameters, IntegerRange};

#[derive(clap::Parser, Debug)]
pub struct HttpControlCommand {
    #[arg(long)]
    pub id: String,

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
    Scene {
        /// List available scenes
        #[arg(long)]
        list: bool,

        /// Name of a scene to activate
        #[arg(required_unless_present = "list")]
        scene: Option<String>,
    },
}

impl HttpControlCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
        let client = args.api_args.api_client()?;
        let device = client.get_device_by_id(&self.id).await?;

        match &self.cmd {
            SubCommand::On | SubCommand::Off => {
                let cap = device
                    .capability_by_instance("powerSwitch")
                    .ok_or_else(|| anyhow::anyhow!("device has no powerSwitch"))?;

                let value = cap
                    .enum_parameter_by_name(match &self.cmd {
                        SubCommand::On => "on",
                        SubCommand::Off => "off",
                        _ => unreachable!(),
                    })
                    .ok_or_else(|| anyhow::anyhow!("powerSwitch has no on/off!?"))?;

                println!("value: {value}");

                let result = client.control_device(&device, &cap, value).await?;
                println!("{result:#?}");
            }

            SubCommand::Brightness { percent } => {
                let cap = device
                    .capability_by_instance("brightness")
                    .ok_or_else(|| anyhow::anyhow!("device has no powerSwitch"))?;
                let value = match &cap.parameters {
                    DeviceParameters::Integer {
                        range: IntegerRange { min, max, .. },
                        ..
                    } => (*percent as u32).max(*min).min(*max),
                    _ => anyhow::bail!("unexpected parameter type for brightness"),
                };
                let result = client.control_device(&device, &cap, value).await?;
                println!("{result:#?}");
            }

            SubCommand::Temperature { kelvin } => {
                let cap = device
                    .capability_by_instance("colorTemperatureK")
                    .ok_or_else(|| anyhow::anyhow!("device has no powerSwitch"))?;
                let value = match &cap.parameters {
                    DeviceParameters::Integer {
                        range: IntegerRange { min, max, .. },
                        ..
                    } => (*kelvin).max(*min).min(*max),
                    _ => anyhow::bail!("unexpected parameter type for colorTemperatureK"),
                };
                let result = client.control_device(&device, &cap, value).await?;
                println!("{result:#?}");
            }

            SubCommand::Color { color } => {
                let cap = device
                    .capability_by_instance("colorRgb")
                    .ok_or_else(|| anyhow::anyhow!("device has no powerSwitch"))?;
                let [r, g, b, _a] = color.to_rgba8();
                let value = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
                let result = client.control_device(&device, &cap, value).await?;
                println!("{result:#?}");
            }

            SubCommand::Scene { list: _, scene } => {
                let scene_caps = client.get_device_scenes(&device).await?;

                for cap in scene_caps {
                    match &cap.parameters {
                        DeviceParameters::Enum { options } => {
                            for opt in options {
                                if let Some(scene) = scene.as_deref() {
                                    if scene.eq_ignore_ascii_case(&opt.name) {
                                        let result = client
                                            .control_device(&device, &cap, opt.value.clone())
                                            .await?;
                                        println!("{result:#?}");
                                        return Ok(());
                                    }
                                    continue;
                                }

                                println!("{}", opt.name);
                            }
                        }
                        _ => anyhow::bail!("unexpected type {cap:#?}"),
                    }
                }
            }
        }

        Ok(())
    }
}
