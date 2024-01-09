use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::hass_mqtt::instance::EntityInstance;
use crate::platform_api::EnumOption;
use crate::service::device::Device as ServiceDevice;
use crate::service::hass::{availability_topic, number_state_topic, topic_safe_id, HassClient};
use crate::service::state::StateHandle;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::ops::Range;

#[derive(Serialize, Clone, Debug)]
pub struct NumberConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub command_topic: String,
    pub state_topic: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f32>,
    pub step: f32,
}

impl NumberConfig {
    async fn publish(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        let disco = state.get_hass_disco_prefix().await;
        let topic = format!(
            "{disco}/number/{unique_id}/config",
            unique_id = self.base.unique_id
        );

        client.publish_obj(topic, self).await
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
            "gv2mqtt/number/{id}/command/{mode_name}",
            id = topic_safe_id(device),
        );
        let state_topic = number_state_topic(device, mode_name);
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
                state_topic,
                min: range.as_ref().map(|r| r.start as f32),
                max: range.as_ref().map(|r| r.end.saturating_sub(1) as f32),
                step: 1f32,
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
                                    client
                                        .publish(&self.number.state_topic, n.to_string())
                                        .await?;
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
                client
                    .publish(&self.number.state_topic, n.to_string())
                    .await?;
                return Ok(());
            }
        }

        log::warn!(
            "Don't know how to report state for {} workMode {} value",
            self.device_id,
            self.mode_name
        );

        Ok(())
    }
}
