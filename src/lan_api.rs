use crate::ble::{Base64HexBytes, SetSceneCode};
use crate::opt_env_var;
use crate::platform_api::from_json;
use crate::undoc_api::GoveeUndocumentedApi;
use anyhow::Context;
use if_addrs::IfAddr;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::Mutex;
use tokio::time::Instant;

// <https://app-h5.govee.com/user-manual/wlan-guide>

/// The port on which govee devices listen for scan requests
const SCAN_PORT: u16 = 4001;
/// The port on which a client needs to listen to receive responses
/// from govee devices
const LISTEN_PORT: u16 = 4002;
/// The port on which govee devices listen for control requests
const CMD_PORT: u16 = 4003;
/// The multicast group of which govee LAN-API enabled devices are members
const MULTICAST: IpAddr = IpAddr::V4(Ipv4Addr::new(239, 255, 255, 250));

#[derive(clap::Parser, Debug)]
pub struct LanDiscoArguments {
    /// Prevent the use of the default multicast broadcast address.
    /// You may also set GOVEE_LAN_NO_MULTICAST=true via the environment.
    #[arg(long, global = true)]
    pub no_multicast: bool,

    /// Enumerate all interfaces, and for each one that has
    /// a broadcast address, broadcast to it
    /// You may also set GOVEE_LAN_BROADCAST_ALL=true via the environment.
    #[arg(long, global = true)]
    pub broadcast_all: bool,

    /// Broadcast to the global broadcast address 255.255.255.255
    /// You may also set GOVEE_LAN_BROADCAST_GLOBAL=true via the environment.
    #[arg(long, global = true)]
    pub global_broadcast: bool,

    /// Addresses to scan. May be broadcast addresses or individual
    /// IP addresses. Can be specified multiple times.
    /// You may also set GOVEE_LAN_SCAN=10.0.0.1,10.0.0.2 via the environment.
    #[arg(long, global = true)]
    pub scan: Vec<String>,

    /// How long to wait for discovery to complete, in seconds
    /// You may also set GOVEE_LAN_DISCO_TIMEOUT via the environment.
    #[arg(long, default_value_t = 3, global = true)]
    disco_timeout: u64,
}

pub fn truthy(s: &str) -> anyhow::Result<bool> {
    if s.eq_ignore_ascii_case("true")
        || s.eq_ignore_ascii_case("yes")
        || s.eq_ignore_ascii_case("on")
        || s == "1"
    {
        Ok(true)
    } else if s.eq_ignore_ascii_case("false")
        || s.eq_ignore_ascii_case("no")
        || s.eq_ignore_ascii_case("off")
        || s == "0"
    {
        Ok(false)
    } else {
        anyhow::bail!("invalid value '{s}', expected true/yes/on/1 or false/no/off/0");
    }
}

impl LanDiscoArguments {
    pub fn to_disco_options(&self) -> anyhow::Result<DiscoOptions> {
        let mut scan_names = self.scan.clone();
        let mut options = DiscoOptions {
            enable_multicast: !self.no_multicast,
            additional_addresses: vec![],
            broadcast_all_interfaces: self.broadcast_all,
            global_broadcast: self.global_broadcast,
        };

        if let Some(v) = opt_env_var::<String>("GOVEE_LAN_NO_MULTICAST")? {
            options.enable_multicast = !truthy(&v)?;
        }

        if let Some(v) = opt_env_var::<String>("GOVEE_LAN_BROADCAST_ALL")? {
            options.broadcast_all_interfaces = truthy(&v)?;
        }

        if let Some(v) = opt_env_var::<String>("GOVEE_LAN_BROADCAST_GLOBAL")? {
            options.global_broadcast = truthy(&v)?;
        }

        if let Some(v) = opt_env_var::<String>("GOVEE_LAN_SCAN")? {
            for addr in v.split(',') {
                scan_names.push(addr.trim().to_string());
            }
        }

        for name in scan_names {
            match name.parse::<IpAddr>() {
                Ok(addr) => {
                    options.additional_addresses.push(addr);
                }
                Err(err1) => match (name.as_str(), SCAN_PORT).to_socket_addrs() {
                    Ok(addrs) => {
                        for a in addrs {
                            options.additional_addresses.push(a.ip());
                        }
                    }
                    Err(err2) => {
                        anyhow::bail!(
                            "{name} could not be parsed as either an \
                            IpAddr ({err1:#}) or a DNS name ({err2:#}"
                        );
                    }
                },
            }
        }

        Ok(options)
    }

    pub fn disco_timeout(&self) -> anyhow::Result<u64> {
        if let Some(v) = opt_env_var("GOVEE_LAN_DISCO_TIMEOUT")? {
            Ok(v)
        } else {
            Ok(self.disco_timeout)
        }
    }
}

pub struct DiscoOptions {
    /// Use the MULTICAST address defined in the LAN protocol
    pub enable_multicast: bool,
    /// Send to this list of additional addresses, which may
    /// include multicast addresses
    pub additional_addresses: Vec<IpAddr>,
    /// Enumerate all interfaces, and for each one that has
    /// a broadcast address, broadcast to it
    pub broadcast_all_interfaces: bool,
    /// Broadcast to the global broadcast address
    pub global_broadcast: bool,
}

impl DiscoOptions {
    pub fn is_empty(&self) -> bool {
        !self.enable_multicast
            && self.additional_addresses.is_empty()
            && !self.broadcast_all_interfaces
            && !self.global_broadcast
    }
}

impl Default for DiscoOptions {
    fn default() -> Self {
        Self {
            enable_multicast: true,
            additional_addresses: vec![],
            broadcast_all_interfaces: false,
            global_broadcast: false,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "cmd", content = "data")]
pub enum Request {
    #[serde(rename = "scan")]
    Scan { account_topic: AccountTopic },
    #[serde(rename = "devStatus")]
    DevStatus {},
    #[serde(rename = "turn")]
    Turn { value: u8 },
    #[serde(rename = "brightness")]
    Brightness { value: u8 },
    #[serde(rename = "colorwc")]
    Color {
        color: DeviceColor,
        #[serde(rename = "colorTemInKelvin")]
        color_temperature_kelvin: u32,
    },
    #[serde(rename = "ptReal")]
    PtReal { command: Vec<String> },
}

#[derive(Serialize, Deserialize, Debug)]
struct RequestMessage {
    msg: Request,
}

fn unspec_ip() -> IpAddr {
    Ipv4Addr::UNSPECIFIED.into()
}

#[derive(Serialize, Deserialize, Debug, Clone, Hash, Eq, PartialEq)]
pub struct LanDevice {
    #[serde(default = "unspec_ip")]
    pub ip: IpAddr,
    pub device: String,
    pub sku: String,
    #[serde(rename = "bleVersionHard")]
    pub ble_version_hard: String,
    #[serde(rename = "bleVersionSoft")]
    pub ble_version_soft: String,
    #[serde(rename = "wifiVersionHard")]
    pub wifi_version_hard: String,
    #[serde(rename = "wifiVersionSoft")]
    pub wifi_version_soft: String,
}

impl LanDevice {
    pub async fn send_request(&self, msg: Request) -> anyhow::Result<()> {
        log::trace!("LanDevice::send_request to {:?} {msg:?}", self.ip);
        let client = udp_socket_for_target(self.ip).await?;
        let data = serde_json::to_string(&RequestMessage { msg })?;
        client.send_to(data.as_bytes(), (self.ip, CMD_PORT)).await?;

        Ok(())
    }

    pub async fn send_turn(&self, on: bool) -> anyhow::Result<()> {
        self.send_request(Request::Turn {
            value: if on { 1 } else { 0 },
        })
        .await
    }

    pub async fn send_brightness(&self, percent: u8) -> anyhow::Result<()> {
        self.send_request(Request::Brightness { value: percent })
            .await
    }

    pub async fn send_color_rgb(&self, color: DeviceColor) -> anyhow::Result<()> {
        self.send_request(Request::Color {
            color,
            color_temperature_kelvin: 0,
        })
        .await
    }

    pub async fn send_real(&self, commands: Vec<String>) -> anyhow::Result<()> {
        self.send_request(Request::PtReal { command: commands })
            .await
    }

    pub async fn send_color_temperature_kelvin(
        &self,
        color_temperature_kelvin: u32,
    ) -> anyhow::Result<()> {
        self.send_request(Request::Color {
            color: DeviceColor { r: 0, g: 0, b: 0 },
            color_temperature_kelvin,
        })
        .await
    }

    pub async fn set_scene_by_name(&self, scene_name: &str) -> anyhow::Result<()> {
        for category in GoveeUndocumentedApi::get_scenes_for_device(&self.sku).await? {
            for scene in category.scenes {
                for effect in scene.light_effects {
                    if scene.scene_name == scene_name && effect.scene_code != 0 {
                        let encoded = Base64HexBytes::encode_for_sku(
                            "Generic:Light",
                            &SetSceneCode::new(effect.scene_code, effect.scence_param),
                        )?
                        .base64();
                        log::info!(
                            "sending scene packet {encoded:x?} for {scene_name}, code {}",
                            effect.scene_code
                        );
                        return self.send_real(encoded).await;
                    }
                }
            }
        }

        anyhow::bail!("unable to set scene {scene_name} for {}", self.device);
    }
}

pub fn boolean_int<'de, D: serde::de::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    Ok(match serde::de::Deserialize::deserialize(deserializer)? {
        JsonValue::Bool(b) => b,
        JsonValue::Number(num) => {
            num.as_i64()
                .ok_or(serde::de::Error::custom("Invalid number"))?
                != 0
        }
        JsonValue::Null => false,
        _ => return Err(serde::de::Error::custom("Wrong type, expected boolean")),
    })
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceStatus {
    #[serde(rename = "onOff", deserialize_with = "boolean_int")]
    pub on: bool,
    pub brightness: u8,
    pub color: DeviceColor,
    #[serde(rename = "colorTemInKelvin")]
    pub color_temperature_kelvin: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeviceColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "cmd", content = "data")]
pub enum Response {
    #[serde(rename = "scan")]
    Scan(LanDevice),
    #[serde(rename = "devStatus")]
    DevStatus(DeviceStatus),
}

#[derive(Serialize, Deserialize, Debug)]
struct ResponseWrapper {
    msg: Response,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum AccountTopic {
    #[serde(rename = "reserve")]
    Reserve,
}

struct ClientListener {
    addr: IpAddr,
    tx: Sender<Response>,
}

#[derive(Default)]
struct ClientInner {
    mux: Mutex<Vec<ClientListener>>,
}

#[derive(Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

#[derive(Debug)]
struct Broadcaster {
    addr: IpAddr,
    socket: UdpSocket,
}

async fn udp_socket_for_target(addr: IpAddr) -> std::io::Result<UdpSocket> {
    match addr {
        IpAddr::V4(_) => UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await,
        IpAddr::V6(_) => UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)).await,
    }
}

impl Broadcaster {
    pub async fn new(addr: IpAddr) -> std::io::Result<Self> {
        let socket = udp_socket_for_target(addr).await?;

        if addr.is_multicast() {
            match addr {
                IpAddr::V4(v4) => {
                    socket.join_multicast_v4(v4, Ipv4Addr::UNSPECIFIED)?;
                    socket.set_multicast_loop_v4(false)?;
                }
                IpAddr::V6(v6) => {
                    socket.join_multicast_v6(&v6, 0)?;
                    socket.set_multicast_loop_v6(false)?;
                }
            }
        } else {
            socket.set_broadcast(true)?;
        }

        Ok(Self { addr, socket })
    }

    pub async fn broadcast<B: AsRef<[u8]>>(&self, bytes: B) -> std::io::Result<()> {
        self.socket
            .send_to(bytes.as_ref(), (self.addr, SCAN_PORT))
            .await?;
        Ok(())
    }
}

async fn send_scan(options: &DiscoOptions) -> anyhow::Result<()> {
    let mut addresses = options.additional_addresses.clone();
    if options.enable_multicast {
        addresses.push(MULTICAST);
    }
    if options.global_broadcast {
        addresses.push(Ipv4Addr::BROADCAST.into());
    }
    if options.broadcast_all_interfaces {
        match if_addrs::get_if_addrs() {
            Ok(ifaces) => {
                for iface in ifaces {
                    if iface.is_loopback() {
                        continue;
                    }
                    let bcast = match iface.addr {
                        IfAddr::V4(v4) => v4.broadcast.map(IpAddr::V4),
                        IfAddr::V6(v6) => v6.broadcast.map(IpAddr::V6),
                    };
                    if let Some(addr) = bcast {
                        log::debug!("Adding bcast {addr} from if {}", iface.name);
                        addresses.push(addr);
                    }
                }
            }
            Err(err) => {
                log::error!("get_if_addrs: {err:#}");
            }
        }
    }

    let mut broadcasters = vec![];
    for addr in addresses {
        match Broadcaster::new(addr).await {
            Ok(b) => broadcasters.push(b),
            Err(err) => {
                log::error!("{addr}: {err:#}");
            }
        }
    }

    let scan = serde_json::to_string(&RequestMessage {
        msg: Request::Scan {
            account_topic: AccountTopic::Reserve,
        },
    })
    .expect("to serialize scan message");
    for b in broadcasters {
        log::trace!("Send disco packet to {:?}", b.addr);
        if let Err(err) = b.broadcast(&scan).await {
            log::error!("Error broadcasting to {b:?}: {err:#}");
        }
    }
    Ok(())
}

async fn lan_disco(
    options: DiscoOptions,
    inner: Arc<ClientInner>,
) -> anyhow::Result<Receiver<LanDevice>> {
    let listen = UdpSocket::bind(("0.0.0.0", LISTEN_PORT)).await.context(
        "Cannot bind to UDP Port 4002, which is required \
        for the Govee LAN API to function. Most likely cause is that you \
        are running another integration (perhaps `Govee LAN Control`, or \
        `homebridge-govee`) that is already bound to that port. \
        Both cannot run on the same machine at the same time. \
        Consider disabling `Govee LAN Control` or setting `lanDisable` in \
        `homebridge-govee`.",
    )?;
    let (tx, rx) = channel(8);

    async fn process_packet(
        addr: SocketAddr,
        data: &[u8],
        inner: &Arc<ClientInner>,
        tx: &Sender<LanDevice>,
    ) -> anyhow::Result<()> {
        log::trace!(
            "process_packet: addr={addr:?} data={}",
            String::from_utf8_lossy(data)
        );

        let mut response: ResponseWrapper = from_json(data)
            .with_context(|| format!("Parsing: {}", String::from_utf8_lossy(data)))?;

        // This is frustrating; some newer devices don't emit the ip field
        // as defined in the spec.  What we do to deal with this is default
        // the ip to the v4 unspecified address during deserialization and
        // check for it here. We'll assume that the ip to use is the ip
        // from which we got the scan response.
        // <https://github.com/wez/govee2mqtt/issues/437>
        if let Response::Scan(dev) = &mut response.msg {
            if dev.ip.is_unspecified() {
                dev.ip = addr.ip();
            } else if dev.ip != addr.ip() {
                log::warn!(
                    "Got scan packet from {addr:?} which has ip set to a different device {dev:?}"
                );
            }
        }

        let mut mux = inner.mux.lock().await;
        mux.retain(|l| !l.tx.is_closed());
        for l in mux.iter() {
            if l.addr == addr.ip() {
                l.tx.send(response.msg.clone()).await.ok();
            }
        }

        if let Response::Scan(info) = response.msg {
            tx.send(info).await?;
        }

        Ok(())
    }

    async fn run_disco(
        options: &DiscoOptions,
        listen: UdpSocket,
        tx: Sender<LanDevice>,
        inner: Arc<ClientInner>,
    ) -> anyhow::Result<()> {
        send_scan(options).await?;

        let mut retry_interval = Duration::from_secs(2);
        let max_retry = Duration::from_secs(60);
        let mut last_send = Instant::now();
        loop {
            let mut buf = [0u8; 4096];

            let deadline = last_send + retry_interval;
            match tokio::time::timeout_at(deadline, listen.recv_from(&mut buf)).await {
                Ok(Ok((len, addr))) => {
                    if let Err(err) = process_packet(addr, &buf[0..len], &inner, &tx).await {
                        log::error!("process_packet: {err:#}");
                    }
                }
                Ok(Err(err)) => {
                    log::error!("recv_from: {err:#}");
                }
                Err(_) => {
                    send_scan(options).await?;
                    last_send = Instant::now();
                    retry_interval = (retry_interval * 2).min(max_retry);
                }
            }
        }
    }

    tokio::spawn(async move {
        if let Err(err) = run_disco(&options, listen, tx, inner).await {
            log::error!("Error at the disco: {err:#}");
        }
    });

    Ok(rx)
}

impl Client {
    pub async fn new(options: DiscoOptions) -> anyhow::Result<(Self, Receiver<LanDevice>)> {
        let inner = Arc::new(ClientInner::default());
        let rx = lan_disco(options, Arc::clone(&inner)).await?;

        Ok((Self { inner }, rx))
    }

    async fn add_listener(&self, addr: IpAddr) -> anyhow::Result<Receiver<Response>> {
        let (tx, rx) = channel(1);
        let mut mux = self.inner.mux.lock().await;
        mux.push(ClientListener { addr, tx });
        Ok(rx)
    }

    /// Interrogate `addr` by sending a scan request to it.
    /// If it is a Govee device that supports the lan protocol,
    /// this method will yield a LanDevice representing it.
    /// In addition, its details will be routed via the discovery
    /// receiver.
    pub async fn scan_ip(&self, addr: IpAddr) -> anyhow::Result<LanDevice> {
        let mut rx = self.add_listener(addr).await?;

        let bcast = Broadcaster::new(addr).await?;
        let scan = serde_json::to_string(&RequestMessage {
            msg: Request::Scan {
                account_topic: AccountTopic::Reserve,
            },
        })
        .expect("to serialize scan message");
        bcast.broadcast(scan).await?;

        loop {
            match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
                Ok(Some(Response::Scan(info))) => {
                    return Ok(info);
                }
                Ok(Some(_)) => {}
                Ok(None) => anyhow::bail!("listener thread terminated"),
                Err(_) => anyhow::bail!("timeout waiting for response"),
            }
        }
    }

    pub async fn query_status(&self, device: &LanDevice) -> anyhow::Result<DeviceStatus> {
        let mut rx = self.add_listener(device.ip).await?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() <= deadline {
            log::trace!("query status of {}", device.ip);
            device.send_request(Request::DevStatus {}).await?;
            match tokio::time::timeout(Duration::from_millis(350), rx.recv()).await {
                Ok(Some(Response::DevStatus(status))) => {
                    return Ok(status);
                }
                Ok(Some(_)) => {}
                Ok(None) => anyhow::bail!("listener thread terminated"),
                Err(_) => {}
            }
        }

        anyhow::bail!("timed out waiting for status");
    }
}
