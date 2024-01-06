use crate::lan_api::{DeviceColor, DeviceStatus};
use crate::platform_api::from_json;
use crate::service::state::StateHandle;
use crate::undoc_api::{ms_timestamp, DeviceEntry, LoginAccountResponse, ParsedOneClick};
use crate::Args;
use anyhow::Context;
use mosquitto_rs::{Event, QoS};
use serde::Deserialize;
use std::time::Duration;

#[derive(Clone)]
pub struct IotClient {
    client: mosquitto_rs::Client,
}

impl IotClient {
    pub fn is_device_compatible(&self, device: &DeviceEntry) -> bool {
        device.device_ext.device_settings.topic.is_some()
    }

    pub async fn request_status_update(&self, device: &DeviceEntry) -> anyhow::Result<()> {
        let device_topic = device
            .device_ext
            .device_settings
            .topic
            .as_ref()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "device {id} has no topic, is it a BLE-only device?",
                    id = device.device
                )
            })?;

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

    pub async fn activate_one_click(&self, item: &ParsedOneClick) -> anyhow::Result<()> {
        for entry in &item.entries {
            for command in &entry.msgs {
                self.client
                    .publish(
                        &entry.topic,
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
    args: &Args,
    state: StateHandle,
    acct: Option<LoginAccountResponse>,
) -> anyhow::Result<()> {
    let client = args.undoc_args.api_client()?;
    let acct = match acct {
        Some(a) => a,
        None => client.login_account_cached().await?,
    };
    log::trace!("{acct:#?}");
    let res = client.get_iot_key(&acct.token).await?;
    log::trace!("{res:#?}");

    let key_bytes = data_encoding::BASE64.decode(res.p12.as_bytes())?;

    let container = p12::PFX::parse(&key_bytes).context("PFX::parse")?;
    for key in container.key_bags(&res.p12_pass).context("key_bags")? {
        let priv_key = openssl::pkey::PKey::private_key_from_der(&key).context("from_der")?;
        let pem = priv_key
            .private_key_to_pem_pkcs8()
            .context("to_pem_pkcs8")?;
        std::fs::write(&args.undoc_args.govee_iot_key, &pem)?;
    }
    for cert in container.cert_bags(&res.p12_pass).context("cert_bags")? {
        let cert = openssl::x509::X509::from_der(&cert).context("x509 from der")?;
        let pem = cert.to_pem().context("cert.to_pem")?;
        std::fs::write(&args.undoc_args.govee_iot_cert, &pem)?;
    }

    let client = mosquitto_rs::Client::with_id(
        &format!(
            "AP/{account_id}/{id}",
            account_id = acct.account_id,
            id = uuid::Uuid::new_v4().simple()
        ),
        true,
    )
    .context("new client")?;
    client
        .configure_tls(
            Some(&args.undoc_args.amazon_root_ca),
            None::<&std::path::Path>,
            Some(&args.undoc_args.govee_iot_cert),
            Some(&args.undoc_args.govee_iot_key),
            None,
        )
        .context("configure_tls")?;
    let status = client
        .connect(&res.endpoint, 8883, Duration::from_secs(120), None)
        .await
        .context("connect")?;
    log::info!("Connected to IoT: {status}");

    let subscriptions = client.subscriber().expect("first and only");

    state
        .set_iot_client(IotClient {
            client: client.clone(),
        })
        .await;

    tokio::spawn(async move {
        while let Ok(event) = subscriptions.recv().await {
            match event {
                Event::Message(msg) => {
                    let payload = String::from_utf8_lossy(&msg.payload);
                    log::trace!("{} -> {payload}", msg.topic);

                    #[derive(Deserialize, Debug)]
                    #[allow(dead_code)]
                    struct Packet {
                        sku: Option<String>,
                        device: String,
                        cmd: String,
                        state: StateUpdate,
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
                    }

                    impl Packet {
                        /// The sku can be in a couple of different places(!)
                        fn sku(&self) -> Option<&str> {
                            if let Some(sku) = self.sku.as_deref() {
                                return Some(sku);
                            }
                            self.state.sku.as_deref()
                        }
                    }

                    match from_json::<Packet, _>(&msg.payload) {
                        Ok(packet) => {
                            log::debug!("{packet:?}");
                            if let Some(sku) = packet.sku() {
                                let mut device = state.device_mut(sku, &packet.device).await;
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
                                // Check on/off last, as we can synthesize "on"
                                // if the other fields are present
                                if let Some(on_off) = packet.state.on_off {
                                    state.on = on_off != 0;
                                }
                                device.set_iot_device_status(state);
                            }
                            state.notify_of_state_change(&packet.device).await?;
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
                        .await?;
                }
            }
        }

        log::info!("IoT loop terminated");
        Ok::<(), anyhow::Error>(())
    });

    Ok(())
}
