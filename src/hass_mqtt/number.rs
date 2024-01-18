use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::hass_mqtt::instance::{publish_entity_config, EntityInstance};
use crate::service::device::Device as ServiceDevice;
use crate::service::hass::{availability_topic, topic_safe_id, topic_safe_string, HassClient};
use crate::service::state::StateHandle;
use anyhow::anyhow;
use async_trait::async_trait;
use mosquitto_rs::router::{Params, Payload, State};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::ops::Range;

#[derive(Serialize, Clone, Debug)]
pub struct NumberConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub command_topic: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f32>,
    pub step: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit_of_measurement: Option<&'static str>,
}

impl NumberConfig {
    pub async fn publish(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        publish_entity_config("number", state, client, &self.base, self).await
    }
}

pub struct WorkModeNumber {
    number: NumberConfig,
    device_id: String,
    state: StateHandle,
    mode_name: String,
    work_mode: JsonValue,
}

impl WorkModeNumber {
    pub fn new(
        device: &ServiceDevice,
        state: &StateHandle,
        label: String,
        mode_name: &str,
        work_mode: JsonValue,
        range: Option<Range<i64>>,
    ) -> Self {
        let command_topic = format!(
            "gv2mqtt/number/{id}/command/{mode_name}/{mode_num}",
            id = topic_safe_id(device),
            mode_num = work_mode
                .as_i64()
                .map(|n| n.to_string())
                .unwrap_or_else(|| "work-mode-was-not-int".to_string()),
        );
        let state_topic = format!(
            "gv2mqtt/number/{id}/state/{mode}",
            id = topic_safe_id(device),
            mode = topic_safe_string(mode_name)
        );

        let availability_topic = availability_topic();
        let unique_id = format!(
            "gv2mqtt-{id}-{mode_name}-number",
            id = topic_safe_id(device),
        );

        Self {
            number: NumberConfig {
                base: EntityConfig {
                    availability_topic,
                    name: Some(label),
                    device_class: None,
                    origin: Origin::default(),
                    device: Device::for_device(device),
                    unique_id,
                    entity_category: None,
                    icon: None,
                },
                command_topic,
                state_topic: Some(state_topic),
                min: range.as_ref().map(|r| r.start as f32).or(Some(0.)),
                max: range
                    .as_ref()
                    .map(|r| r.end.saturating_sub(1) as f32)
                    .or(Some(255.)),
                step: 1f32,
                unit_of_measurement: None,
            },
            device_id: device.id.to_string(),
            state: state.clone(),
            mode_name: mode_name.to_string(),
            work_mode,
        }
    }
}

#[async_trait]
impl EntityInstance for WorkModeNumber {
    async fn publish_config(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        self.number.publish(&state, &client).await
    }

    async fn notify_state(&self, client: &HassClient) -> anyhow::Result<()> {
        let state_topic = self
            .number
            .state_topic
            .as_ref()
            .ok_or_else(|| anyhow!("state_topic is None!?"))?;

        let device = self
            .state
            .device_by_id(&self.device_id)
            .await
            .expect("device to exist");

        if let Some(state) = &device.http_device_state {
            for cap in &state.capabilities {
                if cap.instance == "workMode" {
                    if let Some(work_mode) = cap.state.pointer("/value/workMode") {
                        if *work_mode == self.work_mode {
                            // The current mode matches us, so it is valid to
                            // read the current parameter for that mode

                            if let Some(value) = cap.state.pointer("/value/modeValue") {
                                if let Some(n) = value.as_i64() {
                                    client.publish(state_topic, n.to_string()).await?;
                                    return Ok(());
                                }
                            }
                        }
                    }
                    break;
                }
            }
        }

        if let Some(work_mode) = self.work_mode.as_i64() {
            // FIXME: assuming humidifier, rename that field?
            if let Some(n) = device.humidifier_param_by_mode.get(&(work_mode as u8)) {
                client.publish(state_topic, n.to_string()).await?;
                return Ok(());
            }
        }

        // We might get some data to report later, so this is just debug for now
        log::debug!(
            "Don't know how to report state for {} workMode {} value",
            self.device_id,
            self.mode_name
        );

        Ok(())
    }
}

#[derive(Deserialize)]
pub struct IdAndModeName {
    id: String,
    mode_name: String,
    work_mode: String,
}

pub async fn mqtt_number_command(
    Payload(value): Payload<i64>,
    Params(IdAndModeName {
        id,
        mode_name,
        work_mode,
    }): Params<IdAndModeName>,
    State(state): State<StateHandle>,
) -> anyhow::Result<()> {
    log::info!("{mode_name} for {id}: {value}");
    let work_mode: i64 = work_mode.parse()?;
    let device = state.resolve_device_for_control(&id).await?;

    state
        .humidifier_set_parameter(&device, work_mode, value)
        .await?;

    Ok(())
}
