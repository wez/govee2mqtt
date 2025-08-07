use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::hass_mqtt::instance::{publish_entity_config, EntityInstance};
use crate::platform_api::DeviceCapability;
use crate::service::device::Device as ServiceDevice;
use crate::service::hass::{
    availability_topic, camel_case_to_space_separated, topic_safe_id, topic_safe_string, HassClient,
};
use crate::service::hass_gc::PublishedEntity;
use crate::service::state::StateHandle;
use async_trait::async_trait;
use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct ButtonConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub command_topic: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_press: Option<String>,
}

impl ButtonConfig {
    #[allow(dead_code)]
    pub async fn for_device(
        device: &ServiceDevice,
        instance: &DeviceCapability,
    ) -> anyhow::Result<Self> {
        let command_topic = format!(
            "gv2mqtt/switch/{id}/command/{inst}",
            id = topic_safe_id(device),
            inst = instance.instance
        );
        let availability_topic = availability_topic();
        let unique_id = format!(
            "gv2mqtt-{id}-{inst}",
            id = topic_safe_id(device),
            inst = instance.instance
        );

        Ok(Self {
            base: EntityConfig {
                availability_topic,
                name: Some(camel_case_to_space_separated(&instance.instance)),
                device_class: None,
                origin: Origin::default(),
                device: Device::for_device(device),
                unique_id,
                entity_category: None,
                icon: None,
            },
            command_topic,
            payload_press: None,
        })
    }

    pub fn new<NAME: Into<String>, TOPIC: Into<String>>(name: NAME, topic: TOPIC) -> Self {
        let name = name.into();
        let unique_id = format!("global-{}", topic_safe_string(&name));
        Self {
            base: EntityConfig {
                availability_topic: availability_topic(),
                name: Some(name.to_string()),
                entity_category: None,
                origin: Origin::default(),
                device: Device::this_service(),
                unique_id: unique_id.clone(),
                device_class: None,
                icon: None,
            },
            command_topic: topic.into(),
            payload_press: None,
        }
    }

    pub fn activate_work_mode_preset(
        device: &ServiceDevice,
        name: &str,
        mode_name: &str,
        mode_num: i64,
        value: i64,
    ) -> Self {
        let unique_id = format!(
            "gv2mqtt-{id}-preset-{mode}-{mode_num}-{value}",
            id = topic_safe_id(device),
            mode = topic_safe_string(mode_name),
        );
        let command_topic = format!(
            "gv2mqtt/number/{id}/command/{mode}/{mode_num}",
            id = topic_safe_id(device),
            mode = topic_safe_string(mode_name),
        );
        Self {
            base: EntityConfig {
                availability_topic: availability_topic(),
                name: Some(name.to_string()),
                entity_category: None,
                origin: Origin::default(),
                device: Device::for_device(device),
                unique_id: unique_id.clone(),
                device_class: None,
                icon: None,
            },
            command_topic,
            payload_press: Some(value.to_string()),
        }
    }

    pub fn request_platform_data_for_device(device: &ServiceDevice) -> Self {
        let unique_id = format!(
            "gv2mqtt-{id}-request-platform-data",
            id = topic_safe_id(device)
        );
        let command_topic = format!(
            "gv2mqtt/{id}/request-platform-data",
            id = topic_safe_id(device)
        );
        Self {
            base: EntityConfig {
                availability_topic: availability_topic(),
                name: Some("Request Platform API State".to_string()),
                entity_category: Some("diagnostic".to_string()),
                origin: Origin::default(),
                device: Device::for_device(device),
                unique_id: unique_id.clone(),
                device_class: None,
                icon: None,
            },
            command_topic,
            payload_press: None,
        }
    }
}

#[async_trait]
impl EntityInstance for ButtonConfig {
    async fn publish_config(
        &self,
        state: &StateHandle,
        client: &HassClient,
    ) -> anyhow::Result<PublishedEntity> {
        publish_entity_config("button", state, client, &self.base, self).await
    }

    async fn notify_state(&self, _client: &HassClient) -> anyhow::Result<()> {
        // Buttons have no state
        Ok(())
    }
}
