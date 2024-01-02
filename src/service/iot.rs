use crate::lan_api::DeviceColor;
use crate::service::state::StateHandle;
use crate::Args;
use anyhow::Context;
use serde::Deserialize;
use std::time::Duration;

pub async fn start_iot_client(args: &Args, state: StateHandle) -> anyhow::Result<()> {
    let client = args.undoc_args.api_client()?;
    let acct = client.login_account().await?;
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
        &format!("AP/{account_id}/foo", account_id = acct.account_id),
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
        .connect(&res.endpoint, 8883, Duration::from_secs(10), None)
        .await
        .context("connect")?;
    log::info!("Connected to IoT: {status}");

    let subscriptions = client.subscriber().expect("first and only");

    client
        .subscribe(&acct.topic, mosquitto_rs::QoS::AtMostOnce)
        .await?;

    tokio::spawn(async move {
        log::info!("Waiting for data from IoT");
        while let Ok(msg) = subscriptions.recv().await {
            let payload = String::from_utf8_lossy(&msg.payload);
            log::trace!("{} -> {payload}", msg.topic);

            #[derive(Deserialize, Debug)]
            struct Packet {
                sku: String,
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
            }

            match serde_json::from_slice::<Packet>(&msg.payload) {
                Ok(packet) => {
                    log::info!("{packet:?}");
                    let mut device = state.device_mut(&packet.sku, &packet.device).await;
                    let mut state = device.iot_device_status.clone().unwrap_or_default();
                    if let Some(on_off) = packet.state.on_off {
                        state.on = on_off != 0;
                    }
                    if let Some(v) = packet.state.brightness {
                        state.brightness = v;
                    }
                    if let Some(v) = packet.state.color {
                        state.color = v;
                    }
                    if let Some(v) = packet.state.color_temperature_kelvin {
                        state.color_temperature_kelvin = v;
                    }
                    device.set_iot_device_status(state);
                }
                Err(err) => {
                    log::error!("{err:#} {payload}");
                }
            }
        }

        log::info!("IoT loop terminated");
        drop(client); // keep the client alive. TODO: wrap it up and put it in State
        Ok::<(), anyhow::Error>(())
    });

    Ok(())
}
