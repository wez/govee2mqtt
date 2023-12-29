use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::Mutex;
use tokio::time::Instant;

// <https://app-h5.govee.com/user-manual/wlan-guide>

const SCAN_PORT: u16 = 4001;
const LISTEN_PORT: u16 = 4002;
const CMD_PORT: u16 = 4003;

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "cmd", content = "data")]
pub enum Request {
    #[serde(rename = "scan")]
    Scan { account_topic: AccountTopic },
    #[serde(rename = "devStatus")]
    DevStatus {},
}

#[derive(Serialize, Deserialize, Debug)]
struct RequestMessage {
    msg: Request,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ScannedDevice {
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DeviceStatus {
    #[serde(rename = "onOff")]
    pub on_off: u8,
    pub brightness: u8,
    pub color: DeviceColor,
    #[serde(rename = "colorTemInKelvin")]
    pub color_temperature_kelvin: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct DeviceColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug)]
pub struct LanDevice {
    info: ScannedDevice,
    addr: SocketAddr,
}

impl LanDevice {
    pub fn with_scan(info: ScannedDevice, addr: IpAddr) -> Self {
        let addr = SocketAddr::from((addr, CMD_PORT));
        Self { info, addr }
    }

    pub async fn send_request(&self, msg: Request) -> anyhow::Result<()> {
        let client = UdpSocket::bind("0.0.0.0:0").await?;
        let data = serde_json::to_string(&RequestMessage { msg })?;
        client.send_to(data.as_bytes(), self.addr).await?;

        Ok(())
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "cmd", content = "data")]
pub enum Response {
    #[serde(rename = "scan")]
    Scan(ScannedDevice),
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

#[derive(Serialize, Debug)]
struct RequestScan {}

struct ClientListener {
    addr: IpAddr,
    tx: Sender<Response>,
}

#[derive(Default)]
struct ClientInner {
    mux: Mutex<Vec<ClientListener>>,
}

pub struct Client {
    inner: Arc<ClientInner>,
}

async fn lan_disco(inner: Arc<ClientInner>) -> anyhow::Result<Receiver<LanDevice>> {
    let mcast = UdpSocket::bind("0.0.0.0:0").await?;
    mcast.set_multicast_loop_v4(false)?;
    mcast.join_multicast_v4(Ipv4Addr::new(239, 255, 255, 250), Ipv4Addr::UNSPECIFIED)?;

    let listen = UdpSocket::bind(("0.0.0.0", LISTEN_PORT)).await?;
    let (tx, rx) = channel(8);

    async fn send_scan(mcast: &UdpSocket) -> anyhow::Result<()> {
        let scan = serde_json::to_string(&RequestMessage {
            msg: Request::Scan {
                account_topic: AccountTopic::Reserve,
            },
        })
        .expect("to serialize scan message");
        mcast
            .send_to(scan.as_bytes(), ("239.255.255.250", SCAN_PORT))
            .await?;
        Ok(())
    }

    async fn process_packet(
        addr: SocketAddr,
        data: &[u8],
        inner: &Arc<ClientInner>,
        tx: &Sender<LanDevice>,
    ) -> anyhow::Result<()> {
        let response: ResponseWrapper = serde_json::from_slice(data)
            .with_context(|| format!("Parsing: {}", String::from_utf8_lossy(data)))?;

        let mut mux = inner.mux.lock().await;
        mux.retain(|l| !l.tx.is_closed());
        for l in mux.iter() {
            if l.addr == addr.ip() {
                l.tx.send(response.msg.clone()).await.ok();
            }
        }

        if let Response::Scan(info) = response.msg {
            tx.send(LanDevice::with_scan(info, addr.ip())).await?;
        }

        Ok(())
    }

    async fn run_disco(
        mcast: UdpSocket,
        listen: UdpSocket,
        tx: Sender<LanDevice>,
        inner: Arc<ClientInner>,
    ) -> anyhow::Result<()> {
        send_scan(&mcast).await?;

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
                    send_scan(&mcast).await?;
                    last_send = Instant::now();
                    retry_interval = (retry_interval * 2).min(max_retry);
                }
            }
        }
    }

    tokio::spawn(async move {
        if let Err(err) = run_disco(mcast, listen, tx, inner).await {
            log::error!("Error at the disco: {err:#}");
        }
    });

    Ok(rx)
}

impl Client {
    pub async fn new() -> anyhow::Result<(Self, Receiver<LanDevice>)> {
        let inner = Arc::new(ClientInner::default());
        let rx = lan_disco(Arc::clone(&inner)).await?;

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
        let client = UdpSocket::bind("0.0.0.0:0").await?;
        let scan = serde_json::to_string(&RequestMessage {
            msg: Request::Scan {
                account_topic: AccountTopic::Reserve,
            },
        })
        .expect("to serialize scan message");
        client.send_to(scan.as_bytes(), (addr, SCAN_PORT)).await?;
        loop {
            match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
                Ok(Some(Response::Scan(info))) => {
                    return Ok(LanDevice::with_scan(info, addr));
                }
                Ok(Some(_)) => {}
                Ok(None) => anyhow::bail!("listener thread terminated"),
                Err(_) => anyhow::bail!("timeout waiting for response"),
            }
        }
    }

    pub async fn query_status(&self, device: &LanDevice) -> anyhow::Result<DeviceStatus> {
        let mut rx = self.add_listener(device.addr.ip()).await?;
        device.send_request(Request::DevStatus {}).await?;
        loop {
            match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
                Ok(Some(Response::DevStatus(status))) => {
                    return Ok(status);
                }
                Ok(Some(_)) => {}
                Ok(None) => anyhow::bail!("listener thread terminated"),
                Err(_) => anyhow::bail!("timeout waiting for response"),
            }
        }
    }
}
