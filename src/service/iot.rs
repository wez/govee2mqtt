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

pub async fn start_iot_client(
    args: &UndocApiArguments,
    state: StateHandle,
    acct: Option<LoginAccountResponse>,
) -> anyhow::Result<()> {
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

    tokio::spawn(async move {
        if let Err(err) = run_iot_subscriber(subscriptions, state, client, acct).await {
            log::error!("IoT loop failed: {err:#}");
        }
        log::info!("IoT loop terminated");
        Ok::<(), anyhow::Error>(())
    });

    Ok(())
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
    while let Ok(event) = subscriptions.recv().await {
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
                                                device.set_nightlight_state(nl.clone());
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
