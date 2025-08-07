use crate::hass_mqtt::base::EntityConfig;
use crate::service::hass::HassClient;
use crate::service::hass_gc::PublishedEntity;
use crate::service::state::StateHandle;
use anyhow::Context;
use async_trait::async_trait;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Arc;

#[async_trait]
pub trait EntityInstance: Send + Sync {
    async fn publish_config(
        &self,
        state: &StateHandle,
        client: &HassClient,
    ) -> anyhow::Result<PublishedEntity>;
    async fn notify_state(&self, client: &HassClient) -> anyhow::Result<()>;
}

pub async fn publish_entity_config<T: Serialize>(
    integration: &str,
    state: &StateHandle,
    client: &HassClient,
    base: &EntityConfig,
    config: &T,
) -> anyhow::Result<PublishedEntity> {
    let disco = state.get_hass_disco_prefix().await;
    let topic = format!(
        "{disco}/{integration}/{unique_id}/config",
        unique_id = base.unique_id
    );

    client.publish_obj(topic, config, true).await?;

    Ok(PublishedEntity {
        unique_id: base.unique_id.clone(),
        integration: integration.to_string(),
    })
}

#[derive(Default, Clone)]
pub struct EntityList {
    entities: Vec<Arc<dyn EntityInstance + Send + Sync + 'static>>,
}

impl EntityList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add<E: EntityInstance + Send + Sync + 'static>(&mut self, e: E) {
        self.entities.push(Arc::new(e));
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub async fn publish_config(
        &self,
        state: &StateHandle,
        client: &HassClient,
    ) -> anyhow::Result<HashSet<PublishedEntity>> {
        let mut published = HashSet::new();
        // Allow HASS time to process each entity before registering the next
        let delay = tokio::time::Duration::from_millis(100);
        for e in &self.entities {
            let entity = e
                .publish_config(state, client)
                .await
                .context("EntityList::publish_config")?;
            published.insert(entity);
            tokio::time::sleep(delay).await;
        }
        Ok(published)
    }

    pub async fn notify_state(&self, client: &HassClient) -> anyhow::Result<()> {
        for e in &self.entities {
            e.notify_state(client)
                .await
                .context("EntityList::notify_state")?;
        }
        Ok(())
    }
}
