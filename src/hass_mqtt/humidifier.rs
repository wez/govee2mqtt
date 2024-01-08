use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::hass_mqtt::instance::EntityInstance;
use crate::platform_api::{DeviceParameters, DeviceType, IntegerRange};
use crate::service::device::Device as ServiceDevice;
use crate::service::hass::{availability_topic, topic_safe_id, HassClient};
use crate::service::state::StateHandle;
use async_trait::async_trait;
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

    /// The list of supported modes
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub modes: Vec<String>,

    pub state_topic: String,
}

impl HumidifierConfig {
    pub async fn publish(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        let disco = state.get_hass_disco_prefix().await;
        let topic = format!(
            "{disco}/humidifier/{unique_id}/config",
            unique_id = self.base.unique_id
        );

        client.publish_obj(topic, self).await
    }
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
        let command_topic = format!(
            "gv2mqtt/humidifier/{id}/command",
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

        if let Some(info) = &device.http_device_info {
            if let Some(cap) = info.capability_by_instance("workMode") {
                if let Some(wm) = cap.struct_field_by_name("workMode") {
                    match &wm.field_type {
                        DeviceParameters::Enum { options } => {
                            for opt in options {
                                modes.push(opt.name.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
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
            },
            device_id: device.id.to_string(),
            state: state.clone(),
        })
    }
}

#[async_trait]
impl EntityInstance for Humidifier {
    async fn publish_config(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        self.humidifier.publish(state, client).await
    }

    async fn notify_state(&self, client: &HassClient) -> anyhow::Result<()> {
        let device = self
            .state
            .device_by_id(&self.device_id)
            .await
            .expect("device to exist");

        // TODO: update on/off state and mode

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
            if let Some(info) = &device.http_device_info {
                if let Some(cap) = info.capability_by_instance("workMode") {
                    if let Some(wm) = cap.struct_field_by_name("workMode") {
                        match &wm.field_type {
                            DeviceParameters::Enum { options } => {
                                let mode_value_json = json!(mode_value);
                                for opt in options {
                                    if opt.value == mode_value_json {
                                        client
                                            .publish(
                                                &self.humidifier.mode_state_topic,
                                                opt.name.to_string(),
                                            )
                                            .await?;

                                        break;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
