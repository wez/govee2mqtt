use crate::hass_mqtt::base::EntityConfig;
use serde::Serialize;

#[allow(unused)]
#[derive(Serialize, Clone, Debug)]
pub struct CoverConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub state_topic: String,
    pub position_topic: String,
    pub set_position_topic: String,
    pub command_topic: String,
}
