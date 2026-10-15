use crate::ble::{Base64HexBytes, GoveeBlePacket, HumidifierAutoMode, NotifyHumidifierMode};
use crate::lan_api::{DeviceColor, DeviceStatus};
use crate::platform_api::from_json;
use crate::service::state::StateHandle;
use crate::undoc_api::{ms_timestamp, DeviceEntry, LoginAccountResponse, ParsedOneClick};
use crate::UndocApiArguments;
use anyhow::Context;
use async_channel::Receiver;
use mosquitto_rs::{Event, QoS};
use serde::Deserialize;
use std::time::Duration;
use tokio::time::timeout;

#[derive(Clone)]
pub struct IotClient {
    client: mosquitto_rs::Client,
}

impl IotClient {
    pub fn is_device_compatible(&self, device: &DeviceEntry) -> bool {
        device.device_ext.device_settings.topic.is_some()
    }

    pub async fn request_status_update(&self, device: &DeviceEntry) -> anyhow::Result<()> {
        let device_topic = device.device_topic()?;

        self.client
            .publish(
                device_topic,
                serde_json::to_string(&serde_json::json!({
                    "msg": {
                        "cmd": "status",
                        "cmdVersion": 2,
                        "transaction": format!("v_{}000", ms_timestamp()),
                        "type": 0,
                    }
                }))?,
                QoS::AtMostOnce,
                false,
            )
            .await?;

        Ok(())
    }

    pub async fn set_power_state(&self, device: &DeviceEntry, on: bool) -> anyhow::Result<()> {
        log::trace!("set_power_state for {} to {on}", device.device);
        let device_topic = device.device_topic()?;

        fn pwr(is_on: bool, on: u8, off: u8) -> u8 {
            if is_on {
                on
            } else {
                off
            }
        }

        let power_state = match device.sku.as_str() {
            "H5080" | "H5083" => pwr(on, 17, 16),
            _ => pwr(on, 1, 0),
        };

        self.client
            .publish(
                device_topic,
                serde_json::to_string(&serde_json::json!({
                    "msg": {
                        "cmd": "turn",
                        "data": {
                            "val": power_state,
                        },
                        "cmdVersion": 0,
                        "transaction": format!("v_{}000", ms_timestamp()),
                        "type": 1,
                    }
                }))?,
                QoS::AtMostOnce,
                false,
            )
            .await
            .context("IotClient::set_power_state")?;
        Ok(())
    }

    pub async fn set_brightness(&self, device: &DeviceEntry, percent: u8) -> anyhow::Result<()> {
        log::trace!("set_brightness for {} to {percent}", device.device);
        let device_topic = device.device_topic()?;
        self.client
            .publish(
                device_topic,
                serde_json::to_string(&serde_json::json!({
                    "msg": {
                        "cmd": "brightness",
                        "data": {
                            "val": percent,
                        },
                        "cmdVersion": 0,
                        "transaction": format!("v_{}000", ms_timestamp()),
                        "type": 1,
                    }
                }))?,
                QoS::AtMostOnce,
                false,
            )
            .await
            .context("IotClient::set_brightness")?;
        Ok(())
    }

    pub async fn set_color_temperature(
        &self,
        device: &DeviceEntry,
        kelvin: u32,
    ) -> anyhow::Result<()> {
        log::trace!("set_color_temperature for {} to {kelvin}", device.device);
        let device_topic = device.device_topic()?;

        self.client
            .publish(
                device_topic,
                serde_json::to_string(&serde_json::json!({
                    "msg": {
                        "cmd": "colorwc",
                        "data": {
                            "color": {
                                "r": 0,
                                "g": 0,
                                "b": 0,
                            },
                            "colorTemInKelvin": kelvin,
                        },
                        "cmdVersion": 0,
                        "transaction": format!("v_{}000", ms_timestamp()),
                        "type": 1,
                    }
                }))?,
                QoS::AtMostOnce,
                false,
            )
            .await
            .context("IotClient::set_color_temperature")?;
        Ok(())
    }

    pub async fn set_color_rgb(
        &self,
        device: &DeviceEntry,
        r: u8,
        g: u8,
        b: u8,
    ) -> anyhow::Result<()> {
        log::trace!("set_color_rgb for {} to {r},{g},{b}", device.device);
        let device_topic = device.device_topic()?;

        self.client
            .publish(
                device_topic,
                serde_json::to_string(&serde_json::json!({
                    "msg": {
                        "cmd": "colorwc",
                        "data": {
                            "color":{
                                "r": r,
                                "g": g,
                                "b": b,
                            },
                            "colorTemInKelvin": 0,
                        },
                        "cmdVersion": 0,
                        "transaction": format!("v_{}000", ms_timestamp()),
                        "type": 1,
                    }
                }))?,
                QoS::AtMostOnce,
                false,
            )
            .await
            .context("IotClient::set_color_rgb")?;
        Ok(())
    }

    pub async fn send_real(
        &self,
        device: &DeviceEntry,
        commands: Vec<String>,
    ) -> anyhow::Result<()> {
        log::trace!("send_real for {} to {commands:?}", device.device);
        let device_topic = device.device_topic()?;

        self.client
            .publish(
                device_topic,
                serde_json::to_string(&serde_json::json!({
                    "msg": {
                        "cmd": "ptReal",
                        "data": {
                            "command": commands,
                        },
                        "cmdVersion": 0,
                        "transaction": format!("v_{}000", ms_timestamp()),
                        "type": 1,
                    }
                }))?,
                QoS::AtMostOnce,
                false,
            )
            .await
            .context("IotClient::send_real")?;
        Ok(())
    }

    pub async fn activate_one_click(&self, item: &ParsedOneClick) -> anyhow::Result<()> {
        for entry in &item.entries {
            for command in &entry.msgs {
                self.client
                    .publish(
                        entry.topic.as_str(),
                        serde_json::to_string(command)?,
                        QoS::AtMostOnce,
                        false,
                    )
                    .await
                    .context("sending OneClick")?;
            }
        }
        Ok(())
    }
}

/// How long we give libmosquitto to re-establish a dropped IoT connection
/// before we give up on that client and build a fresh one.
///
/// libmosquitto's own reconnect normally succeeds within a couple of
/// seconds, but it does not always try: when the broker drops the TCP
/// connection without a TLS close_notify, OpenSSL 3 reports
/// `unexpected eof while reading` as a protocol error, libmosquitto sets
/// `errno = EPROTO`, and `mosquitto_loop_forever` treats that as fatal and
/// returns. The loop thread exits, nothing reconnects, and the only
/// visible trace is a single `IoT disconnected` warning.
const IOT_RECONNECT_GRACE: Duration = Duration::from_secs(60);

/// Delay before building a replacement IoT client; doubles on each
/// consecutive failure up to `IOT_RESTART_MAX_DELAY`.
const IOT_RESTART_MIN_DELAY: Duration = Duration::from_secs(5);
const IOT_RESTART_MAX_DELAY: Duration = Duration::from_secs(300);

/// A connected IoT client together with its event stream
struct IotSession {
    client: mosquitto_rs::Client,
    subscriptions: Receiver<Event>,
    acct: LoginAccountResponse,
}

pub async fn start_iot_client(
    args: &UndocApiArguments,
    state: StateHandle,
    acct: Option<LoginAccountResponse>,
) -> anyhow::Result<()> {
    let session = connect_iot_client(args, &state, acct).await?;

    let args = args.clone();
    tokio::spawn(async move {
        run_iot_supervisor(args, state, session).await;
    });

    Ok(())
}

async fn connect_iot_client(
    args: &UndocApiArguments,
    state: &StateHandle,
    acct: Option<LoginAccountResponse>,
) -> anyhow::Result<IotSession> {
    let client = args.api_client()?;
    let acct = match acct {
        Some(a) => a,
        None => client.login_account_cached().await?,
    };
    log::trace!("{acct:#?}");
    let res = client.get_iot_key(&acct.token).await?;
    log::trace!("{res:#?}");

    let key_bytes = data_encoding::BASE64.decode(res.p12.as_bytes())?;

    log::trace!("parsing IoT PFX key");
    let container = p12::PFX::parse(&key_bytes).context("PFX::parse")?;
    for key in container.key_bags(&res.p12_pass).context("key_bags")? {
        let priv_key = openssl::pkey::PKey::private_key_from_der(&key).context("from_der")?;
        let pem = priv_key
            .private_key_to_pem_pkcs8()
            .context("to_pem_pkcs8")?;
        std::fs::write(&args.govee_iot_key, &pem)?;
    }
    for cert in container.cert_bags(&res.p12_pass).context("cert_bags")? {
        let cert = openssl::x509::X509::from_der(&cert).context("x509 from der")?;
        let pem = cert.to_pem().context("cert.to_pem")?;
        std::fs::write(&args.govee_iot_cert, &pem)?;
    }

    let client = mosquitto_rs::Client::with_id(
        &format!(
            "AP/{account_id}/{id}",
            account_id = *acct.account_id,
            id = uuid::Uuid::new_v4().simple()
        ),
        true,
    )
    .context("new client")?;
    client
        .configure_tls(
            Some(&args.amazon_root_ca),
            None::<&std::path::Path>,
            Some(&args.govee_iot_cert),
            Some(&args.govee_iot_key),
            None,
        )
        .context("configure_tls")?;
    log::trace!("Connecting to IoT {} port 8883", res.endpoint);
    let status = timeout(
        Duration::from_secs(60),
        client.connect(&res.endpoint, 8883, Duration::from_secs(120), None),
    )
    .await
    .with_context(|| format!("timeout connecting to IoT {}:8883 in AWS", res.endpoint))?
    .with_context(|| format!("failed to connect to IoT {}:8883 in AWS", res.endpoint))?;
    log::info!("Connected to IoT: {}:8883 {status}", res.endpoint);

    let subscriptions = client.subscriber().expect("first and only");

    state
        .set_iot_client(IotClient {
            client: client.clone(),
        })
        .await;

    Ok(IotSession {
        client,
        subscriptions,
        acct,
    })
}

/// Runs the IoT event loop for `session`, and whenever that session ends,
/// for whatever reason, builds a replacement client so that the IoT path
/// recovers without needing the whole process to be restarted.
async fn run_iot_supervisor(args: UndocApiArguments, state: StateHandle, session: IotSession) {
    let mut session = session;
    let mut delay = IOT_RESTART_MIN_DELAY;
    loop {
        let started = tokio::time::Instant::now();
        let IotSession {
            client,
            subscriptions,
            acct,
        } = session;
        match run_iot_subscriber(subscriptions, state.clone(), client, acct).await {
            Ok(()) => log::warn!("IoT loop terminated"),
            Err(err) => log::error!("IoT loop failed: {err:#}"),
        }

        // A session that stayed up for a while earns a quick restart; one
        // that died young backs off so that we can't spin against the
        // Govee API.
        if started.elapsed() >= IOT_RESTART_MAX_DELAY {
            delay = IOT_RESTART_MIN_DELAY;
        }

        session = loop {
            log::info!("Starting a new IoT client in {delay:?}");
            tokio::time::sleep(delay).await;
            delay = next_restart_delay(delay);
            match connect_iot_client(&args, &state, None).await {
                Ok(session) => break session,
                Err(err) => log::error!("Failed to start a new IoT client: {err:#}"),
            }
        };
    }
}

fn next_restart_delay(delay: Duration) -> Duration {
    (delay * 2).min(IOT_RESTART_MAX_DELAY)
}

/// The outcome of waiting for the next IoT event
#[derive(Debug)]
enum WatchedEvent {
    Event(Event),
    /// The client closed its event stream; it will not produce more events
    Closed,
    /// The connection was lost and has not come back within the grace period
    Stalled(Duration),
}

/// Tracks whether the IoT connection is up, so that a disconnect that
/// libmosquitto never recovers from is noticed instead of waited on forever.
struct ConnectionWatch {
    grace: Duration,
    disconnected_since: Option<tokio::time::Instant>,
}

impl ConnectionWatch {
    fn new(grace: Duration) -> Self {
        Self {
            grace,
            disconnected_since: None,
        }
    }

    async fn recv(&mut self, subscriptions: &Receiver<Event>) -> WatchedEvent {
        let event = match self.disconnected_since {
            None => subscriptions.recv().await,
            Some(since) => {
                match tokio::time::timeout_at(since + self.grace, subscriptions.recv()).await {
                    Ok(event) => event,
                    Err(_) => return WatchedEvent::Stalled(since.elapsed()),
                }
            }
        };
        let Ok(event) = event else {
            return WatchedEvent::Closed;
        };
        match &event {
            Event::Disconnected(_) => {
                if self.disconnected_since.is_none() {
                    self.disconnected_since = Some(tokio::time::Instant::now());
                }
            }
            Event::Connected(_) => {
                self.disconnected_since = None;
            }
            Event::Message(_) => {}
        }
        WatchedEvent::Event(event)
    }
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct Packet {
    sku: Option<String>,
    device: Option<String>,
    /// may actually be found in msg.cmd
    cmd: Option<String>,
    /// This is an embedded json string
    msg: Option<String>,
    state: StateUpdate,
    op: Option<OpData>,
}

#[derive(Deserialize, Debug)]
struct StateUpdate {
    #[serde(rename = "onOff")]
    pub on_off: Option<u8>,
    pub brightness: Option<u8>,
    pub color: Option<DeviceColor>,
    #[serde(rename = "colorTemInKelvin")]
    pub color_temperature_kelvin: Option<u32>,
    pub sku: Option<String>,
    pub device: Option<String>,
}

#[derive(Deserialize, Debug)]
#[allow(unused)]
struct OpData {
    #[serde(default)]
    command: Vec<Base64HexBytes>,

    // The next 4 fields are sourced from H6199
    // <https://github.com/wez/govee2mqtt/issues/36>
    #[serde(rename = "modeValue", default)]
    mode_value: Vec<Base64HexBytes>,
    #[serde(rename = "sleepValue", default)]
    sleep_value: Vec<Base64HexBytes>,
    #[serde(rename = "wakeupValue", default)]
    wakeup_value: Vec<Base64HexBytes>,
    #[serde(rename = "timerValue", default)]
    timer_value: Vec<Base64HexBytes>,
}

impl Packet {
    /// The sku can be in a couple of different places(!)
    fn sku(&self) -> Option<&str> {
        if let Some(sku) = self.sku.as_deref() {
            return Some(sku);
        }
        self.state.sku.as_deref()
    }
    fn device(&self) -> Option<&str> {
        if let Some(device) = self.device.as_deref() {
            return Some(device);
        }
        self.state.device.as_deref()
    }

    fn sku_and_device(&self) -> Option<(&str, &str)> {
        let sku = self.sku()?;
        let device = self.device()?;
        Some((sku, device))
    }
}

async fn run_iot_subscriber(
    subscriptions: Receiver<Event>,
    state: StateHandle,
    client: mosquitto_rs::Client,
    acct: LoginAccountResponse,
) -> anyhow::Result<()> {
    let mut watch = ConnectionWatch::new(IOT_RECONNECT_GRACE);
    loop {
        let event = match watch.recv(&subscriptions).await {
            WatchedEvent::Event(event) => event,
            WatchedEvent::Closed => break,
            WatchedEvent::Stalled(elapsed) => {
                anyhow::bail!("IoT connection was lost and has not reconnected after {elapsed:?}");
            }
        };
        match event {
            Event::Message(msg) => {
                let payload = String::from_utf8_lossy(&msg.payload);
                log::trace!("{} -> {payload}", msg.topic);

                match from_json::<Packet, _>(&msg.payload) {
                    Ok(packet) => {
                        log::debug!("{packet:?}");
                        if let Some((sku, device_id)) = packet.sku_and_device() {
                            {
                                let mut device = state.device_mut(sku, device_id).await;
                                let mut state = match device.iot_device_status.clone() {
                                    Some(state) => state,
                                    None => match device.device_state() {
                                        Some(state) => DeviceStatus {
                                            on: state.on,
                                            brightness: state.brightness,
                                            color: state.color,
                                            color_temperature_kelvin: state.kelvin,
                                        },
                                        None => DeviceStatus::default(),
                                    },
                                };

                                if let Some(v) = packet.state.brightness {
                                    state.brightness = v;
                                    state.on = v != 0;
                                }
                                if let Some(v) = packet.state.color {
                                    state.color = v;
                                    state.on = true;
                                }
                                if let Some(v) = packet.state.color_temperature_kelvin {
                                    state.color_temperature_kelvin = v;
                                    state.on = true;
                                }

                                if let Some(op) = &packet.op {
                                    for cmd in &op.command {
                                        let decoded = cmd.decode_for_sku(sku);
                                        log::debug!("Decoded: {decoded:?} for {sku}");
                                        match decoded {
                                            GoveeBlePacket::NotifyHumidifierNightlight(nl) => {
                                                state.brightness = nl.brightness;
                                                state.color = DeviceColor {
                                                    r: nl.r,
                                                    g: nl.g,
                                                    b: nl.b,
                                                };
                                                device.set_nightlight_state(nl);
                                            }
                                            GoveeBlePacket::NotifyHumidifierAutoMode(
                                                HumidifierAutoMode { target_humidity },
                                            ) => {
                                                device.set_target_humidity(
                                                    target_humidity.as_percent(),
                                                );
                                            }
                                            GoveeBlePacket::NotifyHumidifierMode(
                                                NotifyHumidifierMode { mode, param },
                                            ) => {
                                                device.set_humidifier_work_mode_and_param(
                                                    mode, param,
                                                );
                                            }
                                            GoveeBlePacket::Generic(_) => {
                                                // Ignore packets that we can't decode
                                            }
                                            GoveeBlePacket::SetHumidifierMode(_)
                                            | GoveeBlePacket::SetHumidifierNightlight(_) => {
                                                // Ignore packets that are essentially echoing
                                                // commands sent to the device
                                            }
                                            _ => {
                                                // But warn about the ones we could decode and
                                                // aren't handling here
                                                log::warn!(
                                                    "Taking no action for {decoded:?} for {sku}"
                                                );
                                            }
                                        }
                                    }
                                }

                                // Check on/off last, as we can synthesize "on"
                                // if the other fields are present
                                if let Some(on_off) = packet.state.on_off {
                                    state.on = on_off != 0;
                                }
                                device.set_iot_device_status(state);
                            }
                            state.notify_of_state_change(device_id).await?;
                        }
                    }
                    Err(err) => {
                        log::error!("Decoding IoT Packet: {err:#} {payload}");
                    }
                }
            }
            Event::Disconnected(reason) => {
                log::warn!("IoT disconnected with reason {reason}");
            }
            Event::Connected(status) => {
                log::info!("IoT (re)connected with status {status}");

                client
                    .subscribe(&acct.topic, mosquitto_rs::QoS::AtMostOnce)
                    .await
                    .context("subscribe to account topic")?;
                // This logic tries to subscribe to the same data that is
                // being sent to the individual devices, but the server
                // will close the connection on us when we try this.
                if false {
                    let devices = state.devices().await;
                    for d in devices {
                        if let Some(undoc) = &d.undoc_device_info {
                            if let Ok(topic) = undoc.entry.device_topic() {
                                client
                                    .subscribe(topic, mosquitto_rs::QoS::AtMostOnce)
                                    .await
                                    .with_context(|| {
                                        format!("subscribe to device topic {topic}")
                                    })?;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use mosquitto_rs::{ConnectionStatus, ReasonCode};

    fn connected() -> Event {
        Event::Connected(ConnectionStatus(0))
    }

    /// MOSQ_ERR_CONN_LOST, which is what the TLS EOF case reports
    fn conn_lost() -> Event {
        Event::Disconnected(ReasonCode(7))
    }

    #[tokio::test]
    async fn watch_passes_events_through_while_connected() {
        let (tx, rx) = async_channel::unbounded();
        let mut watch = ConnectionWatch::new(Duration::from_millis(50));
        tx.send(connected()).await.unwrap();
        assert!(matches!(
            watch.recv(&rx).await,
            WatchedEvent::Event(Event::Connected(_))
        ));
        // While connected, an idle stream is not a stall: it just waits.
        assert!(timeout(Duration::from_millis(200), watch.recv(&rx))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn watch_reports_a_disconnect_that_never_recovers() {
        let (tx, rx) = async_channel::unbounded();
        let mut watch = ConnectionWatch::new(Duration::from_millis(50));
        tx.send(connected()).await.unwrap();
        tx.send(conn_lost()).await.unwrap();
        assert!(matches!(watch.recv(&rx).await, WatchedEvent::Event(_)));
        assert!(matches!(
            watch.recv(&rx).await,
            WatchedEvent::Event(Event::Disconnected(_))
        ));
        match watch.recv(&rx).await {
            WatchedEvent::Stalled(elapsed) => assert!(elapsed >= Duration::from_millis(50)),
            other => panic!("expected Stalled, got {other:?}"),
        }
        drop(tx);
    }

    #[tokio::test]
    async fn watch_accepts_a_reconnect_within_the_grace_period() {
        let (tx, rx) = async_channel::unbounded();
        let mut watch = ConnectionWatch::new(Duration::from_millis(200));
        tx.send(conn_lost()).await.unwrap();
        assert!(matches!(watch.recv(&rx).await, WatchedEvent::Event(_)));
        tx.send(connected()).await.unwrap();
        assert!(matches!(
            watch.recv(&rx).await,
            WatchedEvent::Event(Event::Connected(_))
        ));
        // Back to connected: waiting past the old deadline is not a stall.
        assert!(timeout(Duration::from_millis(400), watch.recv(&rx))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn watch_does_not_extend_the_deadline_on_repeated_disconnects() {
        let (tx, rx) = async_channel::unbounded();
        let mut watch = ConnectionWatch::new(Duration::from_millis(150));
        tx.send(conn_lost()).await.unwrap();
        assert!(matches!(watch.recv(&rx).await, WatchedEvent::Event(_)));
        let tx2 = tx.clone();
        tokio::spawn(async move {
            for _ in 0..10 {
                tokio::time::sleep(Duration::from_millis(50)).await;
                let _ = tx2.send(conn_lost()).await;
            }
        });
        let started = tokio::time::Instant::now();
        loop {
            match watch.recv(&rx).await {
                WatchedEvent::Event(_) => continue,
                WatchedEvent::Stalled(_) => break,
                WatchedEvent::Closed => panic!("unexpected close"),
            }
        }
        assert!(started.elapsed() < Duration::from_millis(400));
    }

    #[tokio::test]
    async fn watch_reports_a_closed_stream() {
        let (tx, rx) = async_channel::unbounded::<Event>();
        let mut watch = ConnectionWatch::new(Duration::from_millis(50));
        drop(tx);
        assert!(matches!(watch.recv(&rx).await, WatchedEvent::Closed));
    }

    #[test]
    fn restart_delay_backs_off_and_is_capped() {
        let mut delay = IOT_RESTART_MIN_DELAY;
        let mut seen = vec![];
        for _ in 0..10 {
            seen.push(delay.as_secs());
            delay = next_restart_delay(delay);
        }
        assert_eq!(seen, vec![5, 10, 20, 40, 80, 160, 300, 300, 300, 300]);
    }

    /// A TLS listener that speaks just enough MQTT to accept a CONNECT.
    /// It drops the first connection at the TCP level, without a TLS
    /// close_notify, which is what the AWS IoT endpoint was observed to do.
    mod tls_eof_broker {
        use openssl::asn1::Asn1Time;
        use openssl::hash::MessageDigest;
        use openssl::pkey::PKey;
        use openssl::rsa::Rsa;
        use openssl::ssl::{SslAcceptor, SslMethod};
        use openssl::x509::extension::SubjectAlternativeName;
        use openssl::x509::{X509NameBuilder, X509};
        use std::io::{Read, Write};
        use std::net::{Shutdown, TcpListener};
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        pub struct Broker {
            pub port: u16,
            pub ca_file: PathBuf,
            pub connections: Arc<AtomicUsize>,
        }

        fn read_packet(stream: &mut impl Read) -> std::io::Result<u8> {
            let mut header = [0u8; 1];
            stream.read_exact(&mut header)?;
            let mut remaining = 0usize;
            let mut shift = 0;
            loop {
                let mut b = [0u8; 1];
                stream.read_exact(&mut b)?;
                remaining |= ((b[0] & 0x7f) as usize) << shift;
                shift += 7;
                if b[0] & 0x80 == 0 {
                    break;
                }
            }
            let mut body = vec![0u8; remaining];
            stream.read_exact(&mut body)?;
            Ok(header[0] & 0xf0)
        }

        pub fn start() -> Broker {
            let rsa = Rsa::generate(2048).unwrap();
            let key = PKey::from_rsa(rsa).unwrap();
            let mut name = X509NameBuilder::new().unwrap();
            name.append_entry_by_text("CN", "localhost").unwrap();
            let name = name.build();
            let mut cert = X509::builder().unwrap();
            cert.set_version(2).unwrap();
            cert.set_subject_name(&name).unwrap();
            cert.set_issuer_name(&name).unwrap();
            cert.set_pubkey(&key).unwrap();
            cert.set_not_before(&Asn1Time::days_from_now(0).unwrap())
                .unwrap();
            cert.set_not_after(&Asn1Time::days_from_now(1).unwrap())
                .unwrap();
            let san = SubjectAlternativeName::new()
                .dns("localhost")
                .ip("127.0.0.1")
                .build(&cert.x509v3_context(None, None))
                .unwrap();
            cert.append_extension(san).unwrap();
            cert.sign(&key, MessageDigest::sha256()).unwrap();
            let cert = cert.build();

            let dir = std::env::temp_dir().join(format!(
                "govee2mqtt-iot-test-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let ca_file = dir.join("ca.pem");
            std::fs::write(&ca_file, cert.to_pem().unwrap()).unwrap();

            let mut acceptor = SslAcceptor::mozilla_intermediate(SslMethod::tls()).unwrap();
            acceptor.set_private_key(&key).unwrap();
            acceptor.set_certificate(&cert).unwrap();
            let acceptor = acceptor.build();

            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let connections = Arc::new(AtomicUsize::new(0));
            let counter = connections.clone();

            std::thread::spawn(move || {
                let mut keep = vec![];
                for tcp in listener.incoming() {
                    let Ok(tcp) = tcp else { continue };
                    let Ok(mut tls) = acceptor.accept(tcp) else {
                        continue;
                    };
                    // 0x10 is CONNECT
                    if read_packet(&mut tls).ok() != Some(0x10) {
                        continue;
                    }
                    // CONNACK, session not present, accepted
                    if tls.write_all(&[0x20, 0x02, 0x00, 0x00]).is_err() {
                        continue;
                    }
                    let n = counter.fetch_add(1, Ordering::SeqCst);
                    if n == 0 {
                        std::thread::sleep(std::time::Duration::from_millis(200));
                        // Close the TCP connection under the TLS session,
                        // with no close_notify.
                        let _ = tls.get_ref().shutdown(Shutdown::Both);
                    } else {
                        keep.push(tls);
                    }
                }
            });

            Broker {
                port,
                ca_file,
                connections,
            }
        }
    }

    async fn connect_test_client(
        broker: &tls_eof_broker::Broker,
        id: &str,
    ) -> mosquitto_rs::Client {
        let client = mosquitto_rs::Client::with_id(id, true).unwrap();
        client
            .configure_tls(
                Some(&broker.ca_file),
                None::<&std::path::Path>,
                None::<&std::path::Path>,
                None::<&std::path::Path>,
                None,
            )
            .unwrap();
        timeout(
            Duration::from_secs(10),
            client.connect(
                "localhost",
                broker.port.into(),
                Duration::from_secs(60),
                None,
            ),
        )
        .await
        .expect("connect timed out")
        .expect("connect failed");
        client
    }

    /// Regression test for the IoT path silently dying.
    ///
    /// When the TLS connection is dropped without a close_notify, OpenSSL 3
    /// reports "unexpected eof while reading", libmosquitto treats the
    /// resulting EPROTO as fatal, and its loop thread exits without ever
    /// reconnecting. The ConnectionWatch must turn that into a Stalled
    /// result so that run_iot_supervisor builds a fresh client.
    #[tokio::test]
    async fn tls_eof_without_close_notify_is_detected_as_a_stall() {
        let broker = tls_eof_broker::start();
        let grace = Duration::from_secs(4);

        let client = connect_test_client(&broker, "govee2mqtt-test").await;
        let subscriptions = client.subscriber().unwrap();

        let mut watch = ConnectionWatch::new(grace);
        let mut saw_disconnect = false;
        let outcome = timeout(grace + Duration::from_secs(5), async {
            loop {
                match watch.recv(&subscriptions).await {
                    WatchedEvent::Event(Event::Disconnected(_)) => saw_disconnect = true,
                    WatchedEvent::Event(Event::Connected(_)) if saw_disconnect => {
                        break "reconnected"
                    }
                    WatchedEvent::Event(_) => {}
                    WatchedEvent::Stalled(_) => break "stalled",
                    WatchedEvent::Closed => break "closed",
                }
            }
        })
        .await
        .expect("neither reconnected nor reported a stall");

        assert!(saw_disconnect, "the broker's TCP drop was not observed");
        assert_eq!(
            outcome, "stalled",
            "libmosquitto is expected to give up after a TLS EOF; \
             if it now reconnects on its own this test needs revisiting"
        );
        assert_eq!(
            broker.connections.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "libmosquitto made no reconnect attempt"
        );

        // A freshly built client, which is what run_iot_supervisor does
        // next, connects fine to the same endpoint.
        connect_test_client(&broker, "govee2mqtt-test-2").await;

        let _ = std::fs::remove_dir_all(broker.ca_file.parent().unwrap());
    }
}
