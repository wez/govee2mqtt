use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::hass_mqtt::instance::EntityInstance;
use crate::hass_mqtt::number::NumberConfig;
use crate::service::device::Device as ServiceDevice;
use crate::service::hass::{availability_topic, topic_safe_id, HassClient, IdParameter};
use crate::service::state::StateHandle;
use crate::undoc_api::GoveeUndocumentedApi;
use anyhow::Context;
use async_trait::async_trait;
use mosquitto_rs::router::{Params, Payload, State};

/// The "Effect Speed" number entity for lights that support scene
/// speed control
pub struct EffectSpeedNumber {
    number: NumberConfig,
    device_id: String,
    state: StateHandle,
}

impl EffectSpeedNumber {
    /// Create the entity for the given device.  Returns `None` if
    /// none of the scenes for the device support speed control.
    pub async fn new(device: &ServiceDevice, state: &StateHandle) -> anyhow::Result<Option<Self>> {
        let categories = match GoveeUndocumentedApi::get_scenes_for_device(&device.sku).await {
            Ok(categories) => categories,
            Err(err) => {
                log::error!(
                    "Unable to list scenes for {device} to determine speed support: {err:#}"
                );
                return Ok(None);
            }
        };

        // Determine the maximum number of speed options across
        // all of the speed-capable scenes for this device
        let mut max_options = 0usize;
        for category in categories {
            for scene in category.scenes {
                for effect in scene.light_effects {
                    if let Some(speed) = effect.speed_config() {
                        max_options = max_options.max(speed.option_count());
                    }
                }
            }
        }

        if max_options < 2 {
            return Ok(None);
        }

        let id = topic_safe_id(device);
        let availability_topic = availability_topic();

        Ok(Some(Self {
            number: NumberConfig {
                base: EntityConfig {
                    availability_topic,
                    name: Some("Effect Speed".to_string()),
                    device_class: None,
                    origin: Origin::default(),
                    device: Device::for_device(device),
                    unique_id: format!("gv2mqtt-{id}-effect-speed"),
                    entity_category: None,
                    icon: Some("mdi:speedometer".to_string()),
                },
                command_topic: format!("gv2mqtt/effect-speed/{id}/command"),
                state_topic: Some(format!("gv2mqtt/effect-speed/{id}/state")),
                min: Some(1.),
                max: Some(max_options as f32),
                step: 1.,
                unit_of_measurement: None,
            },
            device_id: device.id.to_string(),
            state: state.clone(),
        }))
    }
}

#[async_trait]
impl EntityInstance for EffectSpeedNumber {
    async fn publish_config(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        self.number.publish(state, client).await
    }

    async fn notify_state(&self, client: &HassClient) -> anyhow::Result<()> {
        let state_topic = self
            .number
            .state_topic
            .as_deref()
            .expect("effect speed has a state topic");

        let device = self
            .state
            .device_by_id(&self.device_id)
            .await
            .expect("device to exist");

        let Some(scene_name) = device.active_scene_name() else {
            // No active scene: report unknown
            client.publish(state_topic, "").await?;
            return Ok(());
        };
        let scene_name = scene_name.to_string();

        let max = self.number.max.expect("effect speed has a max") as usize;

        // Prefer the speed that we last set, falling back to the
        // default speed for the active scene
        let speed_index = match device.active_scene_speed() {
            Some(index) => Some(index),
            None => match crate::undoc_api::find_scene_for_device(&device.sku, &scene_name).await {
                Ok(Some(scene)) => scene.speed.map(|speed| speed.default_index()),
                _ => None,
            },
        };

        match speed_index {
            Some(index) => {
                let value = index.min(max - 1) + 1;
                client.publish(state_topic, value.to_string()).await?;
            }
            None => {
                // The active scene doesn't support speed control
                client.publish(state_topic, "").await?;
            }
        }

        Ok(())
    }
}

/// HASS is requesting that the speed of the active scene be changed
pub async fn mqtt_effect_speed_command(
    Payload(value): Payload<i64>,
    Params(IdParameter { id }): Params<IdParameter>,
    State(state): State<StateHandle>,
) -> anyhow::Result<()> {
    log::info!("Effect speed for {id}: {value}");
    if value < 1 {
        anyhow::bail!("invalid speed {value} for {id}");
    }
    let device = state.resolve_device_for_control(&id).await?;

    state
        .device_set_scene_speed(&device, (value - 1) as usize)
        .await
        .context("mqtt_effect_speed_command")?;

    Ok(())
}
