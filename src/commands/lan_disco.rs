use crate::lan_api::{Client, DiscoOptions};
use std::collections::HashSet;
use std::net::IpAddr;
use tokio::time::{Duration, Instant};

#[derive(clap::Parser, Debug)]
pub struct LanDiscoCommand {
    /// Prevent the use of the default multicast broadcast address
    #[arg(long)]
    pub no_multicast: bool,

    /// Enumerate all interfaces, and for each one that has
    /// a broadcast address, broadcast to it
    #[arg(long)]
    pub broadcast_all: bool,

    /// Broadcast to the global broadcast address 255.255.255.255
    #[arg(long)]
    pub global_broadcast: bool,

    /// Addresses to scan. May be broadcast addresses or individual
    /// IP addresses
    #[arg(long)]
    pub scan: Vec<IpAddr>,

    /// How long to wait for discovery to complete, in seconds
    #[arg(long, default_value = "15")]
    timeout: u64,
}

impl LanDiscoCommand {
    pub async fn run(&self, _args: &crate::Args) -> anyhow::Result<()> {
        let options = DiscoOptions {
            enable_multicast: !self.no_multicast,
            additional_addresses: self.scan.clone(),
            broadcast_all_interfaces: self.broadcast_all,
            global_broadcast: self.global_broadcast,
        };

        if options.is_empty() {
            anyhow::bail!("Discovery options are empty");
        }

        let (client, mut scan) = Client::new(options).await?;

        let deadline = Instant::now() + Duration::from_secs(self.timeout);

        let mut devices = HashSet::new();

        while let Ok(Some(device)) = tokio::time::timeout_at(deadline, scan.recv()).await {
            if !devices.contains(&device) {
                let status = match client.query_status(&device).await {
                    Ok(status) => {
                        if status.on {
                            format!(
                                "{pct}% #{r:02x}{g:02x}{b:02x} {k}k",
                                pct = status.brightness,
                                r = status.color.r,
                                g = status.color.g,
                                b = status.color.b,
                                k = status.color_temperature_kelvin
                            )
                        } else {
                            "off".to_string()
                        }
                    }
                    Err(err) => format!("{err:#}"),
                };

                println!(
                    "{ip:<15} {sku:<7} {id} {status}",
                    ip = device.ip,
                    sku = device.sku,
                    id = device.device
                );

                devices.insert(device);
            }
        }
        Ok(())
    }
}
