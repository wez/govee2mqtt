use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::hass_mqtt::instance::{publish_entity_config, EntityInstance};
use crate::service::device::Device as ServiceDevice;
use crate::service::hass::{availability_topic, topic_safe_id, HassClient};
use crate::service::state::StateHandle;
use async_trait::async_trait;
use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct BinarySensorConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub state_topic: String,
    pub payload_on: &'static str,
    pub payload_off: &'static str,
}

impl BinarySensorConfig {
    pub async fn publish(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        publish_entity_config("binary_sensor", state, client, &self.base, self).await
    }

    pub async fn notify_state(&self, client: &HassClient, on: bool) -> anyhow::Result<()> {
        client
            .publish(
                &self.state_topic,
                if on {
                    self.payload_on
                } else {
                    self.payload_off
                },
            )
            .await
    }
}

/// Turns on when an H717x kettle reports that it stopped heating
/// because it ran out of water (boil-dry protection), and turns off
/// again once the kettle reports that it is on or off its base.
/// The kettle only sends this over the IoT connection, so the sensor
/// stays unknown until the first IoT status report arrives.
pub struct KettleOutOfWater {
    sensor: BinarySensorConfig,
    device_id: String,
    state: StateHandle,
}

impl KettleOutOfWater {
    pub fn new(device: &ServiceDevice, state: &StateHandle) -> Self {
        let unique_id = format!(
            "binary_sensor-{id}-kettle-out-of-water",
            id = topic_safe_id(device)
        );

        Self {
            sensor: BinarySensorConfig {
                base: EntityConfig {
                    availability_topic: availability_topic(),
                    name: Some("Out of Water".to_string()),
                    entity_category: None,
                    origin: Origin::default(),
                    device: Device::for_device(device),
                    unique_id: unique_id.clone(),
                    device_class: Some("problem"),
                    icon: Some("mdi:kettle-alert".to_string()),
                },
                state_topic: format!("gv2mqtt/binary_sensor/{unique_id}/state"),
                payload_on: "ON",
                payload_off: "OFF",
            },
            device_id: device.id.to_string(),
            state: state.clone(),
        }
    }
}

#[async_trait]
impl EntityInstance for KettleOutOfWater {
    async fn publish_config(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        self.sensor.publish(state, client).await
    }

    async fn notify_state(&self, client: &HassClient) -> anyhow::Result<()> {
        let device = self
            .state
            .device_by_id(&self.device_id)
            .await
            .expect("device to exist");

        match device.kettle_base_status {
            Some(status) => {
                self.sensor
                    .notify_state(client, status.is_out_of_water())
                    .await
            }
            None => Ok(()),
        }
    }
}

/// On while an H717x kettle is sitting on its base, off once it is lifted.
/// Like [KettleOutOfWater] it stays unknown until the first IoT status report.
pub struct KettleOnBase {
    sensor: BinarySensorConfig,
    device_id: String,
    state: StateHandle,
}

impl KettleOnBase {
    pub fn new(device: &ServiceDevice, state: &StateHandle) -> Self {
        let unique_id = format!(
            "binary_sensor-{id}-kettle-on-base",
            id = topic_safe_id(device)
        );

        Self {
            sensor: BinarySensorConfig {
                base: EntityConfig {
                    availability_topic: availability_topic(),
                    name: Some("On Base".to_string()),
                    entity_category: None,
                    origin: Origin::default(),
                    device: Device::for_device(device),
                    unique_id: unique_id.clone(),
                    device_class: None,
                    icon: Some("mdi:kettle".to_string()),
                },
                state_topic: format!("gv2mqtt/binary_sensor/{unique_id}/state"),
                payload_on: "ON",
                payload_off: "OFF",
            },
            device_id: device.id.to_string(),
            state: state.clone(),
        }
    }
}

#[async_trait]
impl EntityInstance for KettleOnBase {
    async fn publish_config(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        self.sensor.publish(state, client).await
    }

    async fn notify_state(&self, client: &HassClient) -> anyhow::Result<()> {
        let device = self
            .state
            .device_by_id(&self.device_id)
            .await
            .expect("device to exist");

        match device.kettle_base_status {
            Some(status) => self.sensor.notify_state(client, status.is_on_base()).await,
            None => Ok(()),
        }
    }
}
