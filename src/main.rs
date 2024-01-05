use crate::lan_api::LanDiscoArguments;
use crate::platform_api::GoveeApiArguments;
use crate::service::hass::HassArguments;
use crate::undoc_api::UndocApiArguments;
use clap::Parser;
use std::str::FromStr;

mod ble;
mod cache;
mod commands;
mod lan_api;
mod platform_api;
mod service;
mod undoc_api;
mod version_info;

#[derive(clap::Parser, Debug)]
#[command(version = version_info::govee_version(),  propagate_version=true)]
pub struct Args {
    #[command(flatten)]
    api_args: GoveeApiArguments,
    #[command(flatten)]
    lan_disco_args: LanDiscoArguments,
    #[command(flatten)]
    undoc_args: UndocApiArguments,
    #[command(flatten)]
    hass_args: HassArguments,

    #[command(subcommand)]
    cmd: SubCommand,
}

#[derive(clap::Parser, Debug)]
pub enum SubCommand {
    LanControl(commands::lan_control::LanControlCommand),
    LanDisco(commands::lan_disco::LanDiscoCommand),
    ListHttp(commands::list_http::ListHttpCommand),
    List(commands::list::ListCommand),
    HttpControl(commands::http_control::HttpControlCommand),
    Serve(commands::serve::ServeCommand),
    Undoc(commands::undoc::UndocCommand),
}

impl Args {
    pub async fn run(&self) -> anyhow::Result<()> {
        match &self.cmd {
            SubCommand::LanControl(cmd) => cmd.run(self).await,
            SubCommand::LanDisco(cmd) => cmd.run(self).await,
            SubCommand::ListHttp(cmd) => cmd.run(self).await,
            SubCommand::HttpControl(cmd) => cmd.run(self).await,
            SubCommand::List(cmd) => cmd.run(self).await,
            SubCommand::Serve(cmd) => cmd.run(self).await,
            SubCommand::Undoc(cmd) => cmd.run(self).await,
        }
    }
}

pub fn opt_env_var<T: FromStr>(name: &str) -> anyhow::Result<Option<T>>
where
    <T as FromStr>::Err: std::fmt::Display,
{
    match std::env::var(name) {
        Ok(p) => {
            Ok(Some(p.parse().map_err(|err| {
                anyhow::anyhow!("parsing ${name}: {err:#}")
            })?))
        }
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(err) => anyhow::bail!("${name} is invalid: {err:#}"),
    }
}

fn setup_logger() {
    fn resolve_timezone() -> chrono_tz::Tz {
        std::env::var("TZ")
            .or_else(|_| iana_time_zone::get_timezone())
            .ok()
            .and_then(|name| name.parse().ok())
            .unwrap_or(chrono_tz::UTC)
    }

    let tz = resolve_timezone();
    let utc_suffix = if tz == chrono_tz::UTC { "Z" } else { "" };

    env_logger::builder()
        // A bit of boilerplate here to get timestamps printed in local time.
        // <https://github.com/rust-cli/env_logger/issues/158>
        .format(move |buf, record| {
            use chrono::Utc;
            use env_logger::fmt::Color;
            use std::io::Write;

            let subtle = buf
                .style()
                .set_color(Color::Black)
                .set_intense(true)
                .clone();
            write!(buf, "{}", subtle.value("["))?;
            write!(
                buf,
                "{}{utc_suffix} ",
                Utc::now().with_timezone(&tz).format("%Y-%m-%dT%H:%M:%S")
            )?;
            write!(buf, "{:<5}", buf.default_styled_level(record.level()))?;
            if let Some(path) = record.module_path() {
                write!(buf, " {}", path)?;
            }
            write!(buf, "{}", subtle.value("]"))?;
            writeln!(buf, " {}", record.args())
        })
        .filter_level(log::LevelFilter::Info)
        .parse_env("RUST_LOG")
        .init();
}

#[tokio::main(worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    color_backtrace::install();
    if let Ok(path) = dotenvy::dotenv() {
        eprintln!("Loading environment overrides from {path:?}");
    }

    setup_logger();

    let args = Args::parse();
    args.run().await
}
