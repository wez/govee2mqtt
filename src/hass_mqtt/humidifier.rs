use crate::ble::TargetHumidity;
use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::hass_mqtt::instance::{publish_entity_config, EntityInstance};
use crate::platform_api::{DeviceParameters, DeviceType, EnumOption, IntegerRange};
use crate::service::device::Device as ServiceDevice;
use crate::service::hass::{availability_topic, topic_safe_id, HassClient, IdParameter};
use crate::service::state::StateHandle;
use async_trait::async_trait;
use mosquitto_rs::router::{Params, Payload, State};
use serde::Serialize;
use serde_json::json;

/// <https://www.home-assistant.io/integrations/humidifier.mqtt>
#[derive(Serialize, Clone, Debug)]
pub struct HumidifierConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub command_topic: String,
    /// HASS will publish here to change the humidity target percentage
    pub target_humidity_command_topic: String,
    /// HASS will subscribe here to receive the humidity target percentage
    pub target_humidity_state_topic: String,

    /// HASS will publish here to change the current mode
    pub mode_command_topic: String,
    /// we will publish the current mode here
    pub mode_state_topic: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_humidity: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_humidity: Option<u8>,

    pub optimistic: bool,

    /// The list of supported modes
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub modes: Vec<String>,

    pub state_topic: String,
}

#[derive(Clone)]
pub struct Humidifier {
    humidifier: HumidifierConfig,
    state: StateHandle,
    device_id: String,
}

fn resolve_work_mode(device: &ServiceDevice) -> Option<&[EnumOption]> {
    let info = device.http_device_info.as_ref()?;
    let wm = info
        .capability_by_instance("workMode")
        .and_then(|cap| cap.struct_field_by_name("workMode"))?;

    match &wm.field_type {
        DeviceParameters::Enum { options } => Some(options),
        _ => None,
    }
}

impl Humidifier {
    pub async fn new(device: &ServiceDevice, state: &StateHandle) -> anyhow::Result<Self> {
        let _quirk = device.resolve_quirk();
        let use_iot = device.iot_api_supported() && state.get_iot_client().await.is_some();
        let optimistic = !use_iot;

        // command_topic controls the power state; just route it to
        // the general power switch handler
        let command_topic = format!(
            "gv2mqtt/switch/{id}/command/powerSwitch",
            id = topic_safe_id(device)
        );

        let target_humidity_command_topic = format!(
            "gv2mqtt/humidifier/{id}/set-target",
            id = topic_safe_id(device)
        );
        let target_humidity_state_topic = format!(
            "gv2mqtt/humidifier/{id}/notify-target",
            id = topic_safe_id(device)
        );
        let state_topic = format!("gv2mqtt/humidifier/{id}/state", id = topic_safe_id(device));

        let mode_command_topic = format!(
            "gv2mqtt/humidifier/{id}/set-mode",
            id = topic_safe_id(device)
        );
        let mode_state_topic = format!(
            "gv2mqtt/humidifier/{id}/notify-mode",
            id = topic_safe_id(device)
        );

        let unique_id = format!("gv2mqtt-{id}-humidifier", id = topic_safe_id(device),);

        let mut modes = vec![];
        let mut min_humidity = None;
        let mut max_humidity = None;

        if let Some(options) = resolve_work_mode(device) {
            for opt in options {
                if device.sku == "H7160" && opt.name == "Custom" {
                    // Skip custom mode: we don't know how
                    // to configure it correctly
                    continue;
                }

                modes.push(opt.name.to_string());
            }
        }
        if let Some(info) = &device.http_device_info {
            if let Some(cap) = info.capability_by_instance("humidity") {
                match &cap.parameters {
                    Some(DeviceParameters::Integer {
                        range: IntegerRange { min, max, .. },
                        unit,
                    }) => {
                        if unit.as_deref() == Some("unit.percent") {
                            min_humidity.replace(*min as u8);
                            max_humidity.replace(*max as u8);
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(Self {
            humidifier: HumidifierConfig {
                base: EntityConfig {
                    availability_topic: availability_topic(),
                    name: if device.device_type() == DeviceType::Humidifier {
                        None
                    } else {
                        Some("Humidifier".to_string())
                    },
                    device_class: None,
                    origin: Origin::default(),
                    device: Device::for_device(device),
                    unique_id,
                    entity_category: None,
                    icon: None,
                },
                command_topic,
                target_humidity_command_topic,
                target_humidity_state_topic,

                min_humidity,
                max_humidity,

                mode_command_topic,
                mode_state_topic,
                modes,
                state_topic,
                optimistic,
            },
            device_id: device.id.to_string(),
            state: state.clone(),
        })
    }
}

#[async_trait]
impl EntityInstance for Humidifier {
    async fn publish_config(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        publish_entity_config(
            "humidifier",
            state,
            client,
            &self.humidifier.base,
            &self.humidifier,
        )
        .await
    }

    async fn notify_state(&self, client: &HassClient) -> anyhow::Result<()> {
        let device = self
            .state
            .device_by_id(&self.device_id)
            .await
            .expect("device to exist");

        match device.device_state() {
            Some(device_state) => {
                let is_on = device_state.on;
                client
                    .publish(
                        &self.humidifier.state_topic,
                        if is_on { "ON" } else { "OFF" },
                    )
                    .await?;
            }
            None => {
                client.publish(&self.humidifier.state_topic, "OFF").await?;
            }
        }

        if let Some(humidity) = device.target_humidity_percent {
            client
                .publish(
                    &self.humidifier.target_humidity_state_topic,
                    humidity.to_string(),
                )
                .await?;
        }
        if let Some(mode_value) = device.humidifier_work_mode {
            if let Some(options) = resolve_work_mode(&device) {
                let mode_value_json = json!(mode_value);
                for opt in options {
                    if opt.value == mode_value_json {
                        client
                            .publish(&self.humidifier.mode_state_topic, opt.name.to_string())
                            .await?;

                        break;
                    }
                }
            }
        }

        Ok(())
    }
}

pub async fn mqtt_humidifier_set_mode(
    Payload(mode): Payload<String>,
    Params(IdParameter { id }): Params<IdParameter>,
    State(state): State<StateHandle>,
) -> anyhow::Result<()> {
    log::info!("mqtt_humidifier_set_mode: {id}: {mode}");
    let device = state
        .resolve_device(&id)
        .await
        .ok_or_else(|| anyhow::anyhow!("device '{id}' not found"))?;

    if let Some(options) = resolve_work_mode(&device) {
        for opt in options {
            if opt.name == mode {
                let work_mode = opt
                    .value
                    .as_i64()
                    .ok_or_else(|| anyhow::anyhow!("expected workMode to be a number"))?;

                state
                    .humidifier_set_parameter(&device, work_mode, 0)
                    .await?;

                break;
            }
        }
    }

    Ok(())
}

pub async fn mqtt_humidifier_set_target(
    Payload(percent): Payload<i64>,
    Params(IdParameter { id }): Params<IdParameter>,
    State(state): State<StateHandle>,
) -> anyhow::Result<()> {
    log::info!("mqtt_humidifier_set_target: {id}: {percent}");

    let device = state
        .resolve_device(&id)
        .await
        .ok_or_else(|| anyhow::anyhow!("device '{id}' not found"))?;

    let use_iot = device.pollable_via_iot() && state.get_iot_client().await.is_some();

    if !use_iot {
        if let Some(info) = &device.http_device_info {
            if let Some(cap) = info.capability_by_instance("humidity") {
                state.device_control(&device, cap, percent).await?;

                // For the H7160 at least, setting the humidity
                // will put the device into auto mode and turn
                // it on, however, we don't know that the device
                // is actually turned on.
                //
                // This is handled by the device_was_controlled
                // stuff; it will cause us to poll the device
                // after a short delay, and that should fix up
                // the reported device state.
                return Ok(());
            }
        }
    }

    if let Some(options) = resolve_work_mode(&device) {
        for opt in options {
            if opt.name == "Auto" {
                let work_mode = opt
                    .value
                    .as_i64()
                    .ok_or_else(|| anyhow::anyhow!("expected workMode to be a number"))?;

                let value = TargetHumidity::from_percent(percent as u8);

                state
                    .humidifier_set_parameter(&device, work_mode, value.into_inner().into())
                    .await?;

                break;
            }
        }
    }

    Ok(())
}
