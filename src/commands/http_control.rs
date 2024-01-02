use crate::http_api::{DeviceParameters, EnumOption};
use uncased::Uncased;

#[derive(clap::Parser, Debug)]
pub struct HttpControlCommand {
    #[arg(long)]
    pub id: String,

    #[command(subcommand)]
    cmd: SubCommand,
}

#[derive(clap::Parser, Debug, PartialEq)]
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
    Music {
        /// List available modes
        #[arg(long)]
        list: bool,

        #[arg(long, default_value_t = 100)]
        sensitivity: u8,

        #[arg(long, default_value_t = false)]
        auto_color: bool,

        #[arg(long)]
        color: Option<csscolorparser::Color>,

        /// Name of a music mode to activate
        #[arg(required_unless_present = "list")]
        mode: Option<String>,
    },
}

impl HttpControlCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
        let client = args.api_args.api_client()?;
        let device = client.get_device_by_id(&self.id).await?;

        match &self.cmd {
            SubCommand::On | SubCommand::Off => {
                let result = client
                    .set_power_state(&device, self.cmd == SubCommand::On)
                    .await?;
                println!("{result:#?}");
            }

            SubCommand::Brightness { percent } => {
                let result = client.set_brightness(&device, *percent).await?;
                println!("{result:#?}");
            }

            SubCommand::Temperature { kelvin } => {
                let result = client.set_color_temperature(&device, *kelvin).await?;
                println!("{result:#?}");
            }

            SubCommand::Color { color } => {
                let cap = device
                    .capability_by_instance("colorRgb")
                    .ok_or_else(|| anyhow::anyhow!("device has no colorRgb"))?;
                let [r, g, b, _a] = color.to_rgba8();
                let value = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
                let result = client.control_device(&device, &cap, value).await?;
                println!("{result:#?}");
            }

            SubCommand::Scene { list, scene } => {
                if *list {
                    let mut scenes: Vec<_> = client
                        .list_scene_names(&device)
                        .await?
                        .into_iter()
                        .map(Uncased::new)
                        .collect();
                    scenes.sort();
                    for name in scenes {
                        println!("{name}");
                    }
                } else if let Some(scene) = scene {
                    client.set_scene_by_name(&device, scene).await?;
                }
            }
            SubCommand::Music {
                list,
                mode,
                sensitivity,
                auto_color,
                color,
            } => {
                let cap = device
                    .capability_by_instance("musicMode")
                    .ok_or_else(|| anyhow::anyhow!("device has no musicMode"))?;

                fn for_each_music_mode<F: FnMut(&EnumOption) -> anyhow::Result<bool>>(
                    mut apply: F,
                    parameters: &DeviceParameters,
                ) -> anyhow::Result<bool> {
                    match parameters {
                        DeviceParameters::Struct { fields } => {
                            for f in fields {
                                if f.field_name == "musicMode" {
                                    match &f.field_type {
                                        DeviceParameters::Enum { options } => {
                                            for opt in options {
                                                if !(apply)(opt)? {
                                                    return Ok(false);
                                                }
                                            }
                                            return Ok(true);
                                        }
                                        _ => anyhow::bail!("unexpected type {parameters:#?}"),
                                    }
                                }
                            }
                            anyhow::bail!("musicMode not found in {parameters:#?}");
                        }
                        _ => anyhow::bail!("unexpected type {parameters:#?}"),
                    }
                }

                if *list {
                    for_each_music_mode(
                        |opt| {
                            println!("{}", opt.name);
                            Ok(true)
                        },
                        &cap.parameters,
                    )?;
                } else if let Some(mode) = mode {
                    let mut music_mode = None;
                    for_each_music_mode(
                        |opt| {
                            if opt.name.eq_ignore_ascii_case(mode) {
                                music_mode.replace(opt.value.clone());
                                // Halt iteration
                                Ok(false)
                            } else {
                                // Continue
                                Ok(true)
                            }
                        },
                        &cap.parameters,
                    )?;
                    let Some(music_mode) = music_mode else {
                        anyhow::bail!("mode {mode} not found");
                    };

                    let value = serde_json::json!({
                        "musicMode": music_mode,
                        "sensitivity": sensitivity,
                        "autoColor": if *auto_color { 1 } else { 0 },
                        "rgb": color.as_ref().map(|color| {
                            let [r, g, b, _a] = color.to_rgba8();
                            ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
                        }),
                    });
                    let result = client.control_device(&device, &cap, value).await?;
                    println!("{result:#?}");
                }
            }
        }

        Ok(())
    }
}
