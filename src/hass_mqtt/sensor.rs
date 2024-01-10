use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::hass_mqtt::instance::{publish_entity_config, EntityInstance};
use crate::platform_api::DeviceCapability;
use crate::service::device::Device as ServiceDevice;
use crate::service::hass::{availability_topic, topic_safe_id, topic_safe_string, HassClient};
use crate::service::state::StateHandle;
use async_trait::async_trait;
use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct SensorConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub state_topic: String,
    pub unit_of_measurement: Option<String>,
}

impl SensorConfig {
    pub async fn publish(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        publish_entity_config("sensor", state, client, &self.base, self).await
    }

    pub async fn notify_state(&self, client: &HassClient, value: &str) -> anyhow::Result<()> {
        client.publish(&self.state_topic, value).await
    }
}

#[derive(Clone)]
pub struct GlobalFixedDiagnostic {
    sensor: SensorConfig,
    value: String,
}

#[async_trait]
impl EntityInstance for GlobalFixedDiagnostic {
    async fn publish_config(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        self.sensor.publish(&state, &client).await
    }

    async fn notify_state(&self, client: &HassClient) -> anyhow::Result<()> {
        self.sensor.notify_state(&client, &self.value).await
    }
}

impl GlobalFixedDiagnostic {
    pub fn new<NAME: Into<String>, VALUE: Into<String>>(name: NAME, value: VALUE) -> Self {
        let name = name.into();
        let unique_id = format!("global-{}", topic_safe_string(&name));

        Self {
            sensor: SensorConfig {
                base: EntityConfig {
                    availability_topic: availability_topic(),
                    name: Some(name),
                    entity_category: Some("diagnostic".to_string()),
                    origin: Origin::default(),
                    device: Device::this_service(),
                    unique_id: unique_id.clone(),
                    device_class: None,
                    icon: None,
                },
                state_topic: format!("gv2mqtt/sensor/{unique_id}/state"),
                unit_of_measurement: None,
            },
            value: value.into(),
        }
    }
}

pub struct CapabilitySensor {
    sensor: SensorConfig,
    device_id: String,
    state: StateHandle,
    instance_name: String,
}

impl CapabilitySensor {
    pub async fn new(
        device: &ServiceDevice,
        state: &StateHandle,
        instance: &DeviceCapability,
    ) -> anyhow::Result<Self> {
        let unique_id = format!(
            "sensor-{id}-{inst}",
            id = topic_safe_id(device),
            inst = topic_safe_string(&instance.instance)
        );

        let unit_of_measurement = match instance.instance.as_str() {
            "sensorTemperature" => Some("°C".to_string()),
            "sensorHumidity" => Some("%".to_string()),
            _ => None,
        };

        let name = match instance.instance.as_str() {
            "sensorTemperature" => "Temperature".to_string(),
            "sensorHumidity" => "Humidity".to_string(),
            _ => instance.instance.to_string(),
        };

        Ok(Self {
            sensor: SensorConfig {
                base: EntityConfig {
                    availability_topic: availability_topic(),
                    name: Some(name),
                    entity_category: Some("diagnostic".to_string()),
                    origin: Origin::default(),
                    device: Device::for_device(device),
                    unique_id: unique_id.clone(),
                    device_class: None,
                    icon: None,
                },
                state_topic: format!("gv2mqtt/sensor/{unique_id}/state"),
                unit_of_measurement,
            },
            device_id: device.id.to_string(),
            state: state.clone(),
            instance_name: instance.instance.to_string(),
        })
    }
}

#[async_trait]
impl EntityInstance for CapabilitySensor {
    async fn publish_config(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        self.sensor.publish(&state, &client).await
    }

    async fn notify_state(&self, client: &HassClient) -> anyhow::Result<()> {
        let device = self
            .state
            .device_by_id(&self.device_id)
            .await
            .expect("device to exist");

        if let Some(state) = &device.http_device_state {
            for cap in &state.capabilities {
                if cap.instance == self.instance_name {
                    let value = match self.instance_name.as_str() {
                        "sensorTemperature" => {
                            // Valid for H5103 at least
                            match cap
                                .state
                                .pointer("/value")
                                .and_then(|v| v.as_f64())
                                .map(|v| v / 100.)
                            {
                                Some(v) => v.to_string(),
                                None => "".to_string(),
                            }
                        }
                        "sensorHumidity" => {
                            // Valid for H5103 at least
                            match cap
                                .state
                                .pointer("/value/currentHumidity")
                                .and_then(|v| v.as_f64())
                                .map(|v| v / 100.)
                            {
                                Some(v) => v.to_string(),
                                None => "".to_string(),
                            }
                        }
                        _ => cap.state.to_string(),
                    };

                    return self.sensor.notify_state(&client, &value).await;
                }
            }
        }
        log::trace!(
            "CapabilitySensor::notify_state: didn't find state for {device} {instance}",
            instance = self.instance_name
        );
        Ok(())
    }
}
