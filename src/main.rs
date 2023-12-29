use clap::Parser;

mod commands;
mod http_api;
mod lan_api;
mod version_info;

#[derive(clap::Parser, Debug)]
#[command(version = version_info::govee_version())]
pub struct Args {
    #[command(subcommand)]
    cmd: SubCommand,
}

#[derive(clap::Parser, Debug)]
pub enum SubCommand {
    LanDisco(commands::lan_disco::LanDiscoCommand),
    ListHttp(commands::list_http::ListHttpCommand),
}

impl Args {
    pub async fn run(&self) -> anyhow::Result<()> {
        match &self.cmd {
            SubCommand::LanDisco(cmd) => cmd.run(self).await,
            SubCommand::ListHttp(cmd) => cmd.run(self).await,
        }
    }
}

#[tokio::main(worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    color_backtrace::install();
    if let Ok(path) = dotenvy::dotenv() {
        eprintln!("Loading environment overrides from {path:?}");
    }

    let tz: chrono_tz::Tz = iana_time_zone::get_timezone()
        .ok()
        .and_then(|name| name.parse().ok())
        .unwrap_or(chrono_tz::UTC);
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

    let args = Args::parse();
    args.run().await
}
