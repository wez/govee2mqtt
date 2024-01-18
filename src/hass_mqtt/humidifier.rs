use crate::ble::TargetHumidity;
use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::hass_mqtt::instance::{publish_entity_config, EntityInstance};
use crate::hass_mqtt::work_mode::ParsedWorkMode;
use crate::platform_api::{DeviceParameters, DeviceType, IntegerRange};
use crate::service::device::Device as ServiceDevice;
use crate::service::hass::{availability_topic, topic_safe_id, HassClient, IdParameter};
use crate::service::state::StateHandle;
use anyhow::anyhow;
use async_trait::async_trait;
use mosquitto_rs::router::{Params, Payload, State};
use serde::Serialize;
use serde_json::json;

pub const DEVICE_CLASS_HUMIDITY: &str = "humidity";

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

        let mut min_humidity = None;
        let mut max_humidity = None;

        let work_mode = ParsedWorkMode::with_device(device).ok();
        let modes = work_mode
            .as_ref()
            .map(|wm| wm.get_mode_names())
            .unwrap_or(vec![]);

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
        } else {
            // We need an initial value otherwise hass will not enable
            // the target humidity control in its UI.
            // Because we are setting this in the device state,
            // this latches so we only do this once.
            let guessed_value = self.humidifier.min_humidity.unwrap_or(0);
            self.state
                .device_mut(&device.sku, &device.id)
                .await
                .set_target_humidity(guessed_value);
            client
                .publish(
                    &self.humidifier.target_humidity_state_topic,
                    guessed_value.to_string(),
                )
                .await?;
        }

        if let Some(mode_value) = device.humidifier_work_mode {
            if let Ok(work_mode) = ParsedWorkMode::with_device(&device) {
                let mode_value_json = json!(mode_value);
                if let Some(mode) = work_mode.mode_for_value(&mode_value_json) {
                    client
                        .publish(&self.humidifier.mode_state_topic, mode.name.to_string())
                        .await?;
                }
            }
        } else {
            let work_modes = ParsedWorkMode::with_device(&device)?;

            if let Some(state) = &device.http_device_state {
                for cap in &state.capabilities {
                    if cap.instance == "workMode" {
                        if let Some(mode_num) = cap.state.pointer("/value/workMode") {
                            if let Some(mode) = work_modes.mode_for_value(mode_num) {
                                return client
                                    .publish(
                                        &self.humidifier.mode_state_topic,
                                        mode.name.to_string(),
                                    )
                                    .await;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

pub async fn mqtt_device_set_work_mode(
    Payload(mode): Payload<String>,
    Params(IdParameter { id }): Params<IdParameter>,
    State(state): State<StateHandle>,
) -> anyhow::Result<()> {
    log::info!("mqtt_humidifier_set_mode: {id}: {mode}");
    let device = state
        .resolve_device(&id)
        .await
        .ok_or_else(|| anyhow::anyhow!("device '{id}' not found"))?;

    let work_modes = ParsedWorkMode::with_device(&device)?;
    let work_mode = work_modes
        .mode_by_name(&mode)
        .ok_or_else(|| anyhow!("mode {mode} not found"))?;
    let mode_num = work_mode
        .value
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("expected workMode to be a number"))?;

    let value = work_mode
        .values
        .get(0)
        .and_then(|v| v.value.as_i64())
        .unwrap_or(0);

    state
        .humidifier_set_parameter(&device, mode_num, value)
        .await?;

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

                // We're running in optimistic mode; stash
                // the last set value so that we can report it
                // to hass
                state
                    .device_mut(&device.sku, &device.id)
                    .await
                    .set_target_humidity(percent as u8);

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

    let work_modes = ParsedWorkMode::with_device(&device)?;
    let work_mode = work_modes
        .mode_by_name("Auto")
        .ok_or_else(|| anyhow!("mode Auto not found"))?;
    let mode_num = work_mode
        .value
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("expected workMode to be a number"))?;

    let value = TargetHumidity::from_percent(percent as u8);

    state
        .humidifier_set_parameter(&device, mode_num, value.into_inner().into())
        .await?;

    Ok(())
}
