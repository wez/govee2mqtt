use crate::hass_mqtt::base::EntityConfig;
use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct NumberConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub command_topic: String,
    pub state_topic: String,
    pub min: f32,
    pub max: f32,
    pub step: f32,
}
