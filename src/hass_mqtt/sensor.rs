use serde::Serialize;
use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::service::hass::{availability_topic, topic_safe_string, HassClient};
use crate::service::state::StateHandle;

#[derive(Serialize, Clone, Debug)]
pub struct SensorConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub state_topic: String,
    pub unit_of_measurement: Option<String>,
}

impl SensorConfig {
    pub fn global_fixed_diagnostic(name: &str) -> Self {
        let unique_id = format!("global-{}", topic_safe_string(name));
        Self {
            base: EntityConfig {
                availability_topic: availability_topic(),
                name: Some(name.to_string()),
                entity_category: Some("diagnostic".to_string()),
                origin: Origin::default(),
                device: Device::this_service(),
                unique_id: unique_id.clone(),
                device_class: None,
                icon: None,
            },
            state_topic: format!("gv2mqtt/sensor/{unique_id}/state"),
            unit_of_measurement: None,
        }
    }

    pub async fn publish(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        let disco = state.get_hass_disco_prefix().await;
        let topic = format!(
            "{disco}/sensor/{unique_id}/config",
            unique_id = self.base.unique_id
        );

        client.publish_obj(topic, self).await
    }

    pub async fn notify_state(&self, client: &HassClient, value: &str) -> anyhow::Result<()> {
        client.publish(&self.state_topic, value).await
    }
}
