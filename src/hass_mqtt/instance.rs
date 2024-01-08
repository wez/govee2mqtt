use crate::service::hass::HassClient;
use crate::service::state::StateHandle;
use anyhow::Context;
use async_trait::async_trait;
use std::sync::Arc;

#[async_trait]
pub trait EntityInstance: Send + Sync {
    async fn publish_config(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()>;
    async fn notify_state(&self, client: &HassClient) -> anyhow::Result<()>;
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

    pub async fn publish_config(
        &self,
        state: &StateHandle,
        client: &HassClient,
    ) -> anyhow::Result<()> {
        for e in &self.entities {
            e.publish_config(state, client)
                .await
                .context("EntityList::publish_config")?;
        }
        Ok(())
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
