use serde::Serialize;
use crate::hass_mqtt::base::EntityConfig;
use crate::service::hass::HassClient;
use crate::service::state::StateHandle;

#[derive(Serialize, Clone, Debug)]
pub struct SceneConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub command_topic: String,
    pub payload_on: String,
}

impl SceneConfig {
    pub async fn publish(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        let disco = state.get_hass_disco_prefix().await;
        let topic = format!(
            "{disco}/scene/{unique_id}/config",
            unique_id = self.base.unique_id
        );

        client.publish_obj(topic, self).await
    }

    pub async fn notify_state(&self, _client: &HassClient, _: &str) -> anyhow::Result<()> {
        // Scenes have no state
        Ok(())
    }
}
