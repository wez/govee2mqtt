use crate::hass_mqtt::base::EntityConfig;
use crate::hass_mqtt::instance::{publish_entity_config, EntityInstance};
use crate::service::hass_gc::PublishedEntity;
use crate::service::hass::HassClient;
use crate::service::state::StateHandle;
use async_trait::async_trait;
use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct SceneConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub command_topic: String,
    pub payload_on: String,
}

impl SceneConfig {
    pub async fn publish(
        &self,
        state: &StateHandle,
        client: &HassClient,
    ) -> anyhow::Result<PublishedEntity> {
        publish_entity_config("scene", state, client, &self.base, self).await
    }
}

#[async_trait]
impl EntityInstance for SceneConfig {
    async fn publish_config(
        &self,
        state: &StateHandle,
        client: &HassClient,
    ) -> anyhow::Result<PublishedEntity> {
        self.publish(&state, &client).await
    }

    async fn notify_state(&self, _client: &HassClient) -> anyhow::Result<()> {
        // Scenes have no state
        Ok(())
    }
}
