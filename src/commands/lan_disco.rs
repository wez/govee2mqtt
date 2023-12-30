use crate::lan_api::{Client, DiscoOptions};
use std::net::IpAddr;

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
}

impl LanDiscoCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
        let options = DiscoOptions {
            enable_multicast: !self.no_multicast,
            additional_addresses: self.scan.clone(),
            broadcast_all_interfaces: self.broadcast_all,
            global_broadcast: self.global_broadcast,
        };

        let (client, mut scan) = Client::new(options).await?;

        while let Some(device) = scan.recv().await {
            log::info!("{device:?}");

            if let Ok(resp) = client.query_status(&device).await {
                log::info!("Got status: {resp:?}");
            }
        }
        Ok(())
    }
}
