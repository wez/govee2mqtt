use crate::ble::NotifyEnergyMonitoring;
use crate::commands::serve::POLL_INTERVAL;
use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::hass_mqtt::humidifier::DEVICE_CLASS_HUMIDITY;
use crate::hass_mqtt::instance::{publish_entity_config, EntityInstance};
use crate::platform_api::DeviceCapability;
use crate::service::device::Device as ServiceDevice;
use crate::service::hass::{
    availability_topic, energy_counter_topic, topic_safe_id, topic_safe_string, HassClient,
};
use crate::service::quirks::HumidityUnits;
use crate::service::state::StateHandle;
use crate::temperature::{TemperatureUnits, TemperatureValue, DEVICE_CLASS_TEMPERATURE};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;

#[derive(Serialize, Clone, Debug)]
pub struct SensorConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub state_topic: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_class: Option<StateClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit_of_measurement: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_attributes_topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_template: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_reset_value_template: Option<&'static str>,
}

#[allow(unused)]
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateClass {
    #[serde(rename = "measurement")]
    Measurement,
    #[serde(rename = "total")]
    Total,
    #[serde(rename = "total_increasing")]
    TotalIncreasing,
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
        self.sensor.publish(state, client).await
    }

    async fn notify_state(&self, client: &HassClient) -> anyhow::Result<()> {
        self.sensor.notify_state(client, &self.value).await
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
                state_class: None,
                unit_of_measurement: None,
                json_attributes_topic: None,
                value_template: None,
                last_reset_value_template: None,
            },
            value: value.into(),
        }
    }
}

#[derive(Clone)]
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
            "sensorTemperature" => Some(state.get_temperature_scale().await.unit_of_measurement()),
            "sensorHumidity" => Some("%"),
            _ => None,
        };

        let device_class = match instance.instance.as_str() {
            "sensorTemperature" => Some(DEVICE_CLASS_TEMPERATURE),
            "sensorHumidity" => Some(DEVICE_CLASS_HUMIDITY),
            _ => None,
        };

        let state_class = match instance.instance.as_str() {
            "sensorTemperature" => Some(StateClass::Measurement),
            "sensorHumidity" => Some(StateClass::Measurement),
            _ => None,
        };

        let name = match instance.instance.as_str() {
            "sensorTemperature" => "Temperature".to_string(),
            "sensorHumidity" => "Humidity".to_string(),
            "online" => "Connected to Govee Cloud".to_string(),
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
                    device_class,
                    icon: None,
                },
                state_topic: format!("gv2mqtt/sensor/{unique_id}/state"),
                state_class,
                unit_of_measurement,
                json_attributes_topic: None,
                value_template: None,
                last_reset_value_template: None,
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
        self.sensor.publish(state, client).await
    }

    async fn notify_state(&self, client: &HassClient) -> anyhow::Result<()> {
        let device = self
            .state
            .device_by_id(&self.device_id)
            .await
            .expect("device to exist");

        let quirk = device.resolve_quirk();

        if let Some(cap) = device.get_state_capability_by_instance(&self.instance_name) {
            let value = match self.instance_name.as_str() {
                "sensorTemperature" => {
                    let units = quirk
                        .and_then(|q| q.platform_temperature_sensor_units)
                        .unwrap_or(TemperatureUnits::Fahrenheit);

                    match cap
                        .state
                        .pointer("/value")
                        .and_then(|v| v.as_f64())
                        .map(|v| TemperatureValue::new(v, units))
                    {
                        Some(v) => {
                            let value = v
                                .as_unit(self.state.get_temperature_scale().await.into())
                                .value();
                            format!("{value:.2}")
                        }
                        None => "".to_string(),
                    }
                }
                "sensorHumidity" => {
                    let units = quirk
                        .and_then(|q| q.platform_humidity_sensor_units)
                        .unwrap_or(HumidityUnits::RelativePercent);
                    match cap
                        .state
                        .pointer("/value")
                        .and_then(|v| v.as_f64())
                        .map(|v| units.from_reading_to_relative_percent(v))
                    {
                        Some(v) => format!("{v:.2}"),
                        None => "".to_string(),
                    }
                }
                _ => cap.state.to_string(),
            };

            return self.sensor.notify_state(client, &value).await;
        }
        log::trace!(
            "CapabilitySensor::notify_state: didn't find state for {device} {instance}",
            instance = self.instance_name
        );
        Ok(())
    }
}

pub struct EnergyMonitoringReading {
    name: &'static str,
    id: &'static str,
    device_class: &'static str,
    unit_of_measurement: &'static str,
    diagnostic: bool,
    /// Whether this is the energy counter, published along with its
    /// last_reset, see EnergyCounterState
    energy_counter: bool,
    format: fn(&NotifyEnergyMonitoring) -> Option<String>,
}

pub static ENERGY_MONITORING_READINGS: [EnergyMonitoringReading; 6] = [
    EnergyMonitoringReading {
        name: "Power",
        id: "power",
        device_class: "power",
        unit_of_measurement: "W",
        diagnostic: false,
        energy_counter: false,
        format: |r| Some(format!("{:.2}", r.power())),
    },
    EnergyMonitoringReading {
        name: "Energy",
        id: "energy",
        device_class: "energy",
        unit_of_measurement: "kWh",
        diagnostic: false,
        energy_counter: true,
        format: |r| Some(format!("{:.4}", r.energy_kwh())),
    },
    EnergyMonitoringReading {
        name: "Voltage",
        id: "voltage",
        device_class: "voltage",
        unit_of_measurement: "V",
        diagnostic: false,
        energy_counter: false,
        format: |r| Some(format!("{:.2}", r.voltage())),
    },
    EnergyMonitoringReading {
        name: "Current",
        id: "current",
        device_class: "current",
        unit_of_measurement: "A",
        diagnostic: false,
        energy_counter: false,
        format: |r| r.current().map(|v| format!("{v:.2}")),
    },
    EnergyMonitoringReading {
        name: "Power Factor",
        id: "power-factor",
        device_class: "power_factor",
        unit_of_measurement: "%",
        diagnostic: false,
        energy_counter: false,
        format: |r| r.power_factor().map(|v| v.to_string()),
    },
    EnergyMonitoringReading {
        name: "On Time",
        id: "on-time",
        device_class: "duration",
        unit_of_measurement: "s",
        diagnostic: true,
        energy_counter: false,
        format: |r| Some(r.on_time_seconds.0.to_string()),
    },
];

/// The state of the energy sensor for hass
#[derive(Serialize)]
struct EnergyStatePayload {
    value: Option<String>,
    last_reset: DateTime<Utc>,
}

/// The state of the energy sensor, or None if nothing should be published:
/// a reading that the counter hasn't tracked must not go out with a
/// last_reset that doesn't match it
fn energy_state_payload(
    device: &ServiceDevice,
    format: fn(&NotifyEnergyMonitoring) -> Option<String>,
) -> Option<EnergyStatePayload> {
    let counter = device.energy_counter.as_ref()?;
    let report = device.energy_monitoring_report();
    if report.is_some_and(|r| r.energy_deciwatt_hours.0 != counter.energy_deciwatt_hours) {
        return None;
    }
    Some(EnergyStatePayload {
        value: report.and_then(format),
        last_reset: counter.last_reset,
    })
}

/// Represents an unknown value to the hass mqtt sensor
const PAYLOAD_NONE: &str = "None";

pub struct EnergyMonitoringSensor {
    sensor: SensorConfig,
    device_id: String,
    state: StateHandle,
    reading: &'static EnergyMonitoringReading,
}

impl EnergyMonitoringSensor {
    pub fn new(
        device: &ServiceDevice,
        state: &StateHandle,
        reading: &'static EnergyMonitoringReading,
    ) -> Self {
        let unique_id = format!(
            "sensor-{id}-energy-monitoring-{reading}",
            id = topic_safe_id(device),
            reading = reading.id
        );

        Self {
            sensor: SensorConfig {
                base: EntityConfig {
                    availability_topic: availability_topic(),
                    name: Some(reading.name.to_string()),
                    entity_category: reading.diagnostic.then(|| "diagnostic".to_string()),
                    origin: Origin::default(),
                    device: Device::for_device(device),
                    unique_id: unique_id.clone(),
                    device_class: Some(reading.device_class),
                    icon: None,
                },
                state_topic: format!("gv2mqtt/sensor/{unique_id}/state"),
                state_class: Some(if reading.energy_counter {
                    StateClass::Total
                } else {
                    StateClass::Measurement
                }),
                unit_of_measurement: Some(reading.unit_of_measurement),
                json_attributes_topic: None,
                value_template: reading.energy_counter.then_some("{{ value_json.value }}"),
                last_reset_value_template: reading
                    .energy_counter
                    .then_some("{{ value_json.last_reset }}"),
            },
            device_id: device.id.to_string(),
            state: state.clone(),
            reading,
        }
    }
}

#[async_trait]
impl EntityInstance for EnergyMonitoringSensor {
    async fn publish_config(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        self.sensor.publish(state, client).await
    }

    async fn notify_state(&self, client: &HassClient) -> anyhow::Result<()> {
        let device = self
            .state
            .device_by_id(&self.device_id)
            .await
            .expect("device to exist");

        if self.reading.energy_counter {
            let topic = energy_counter_topic(&device);
            // Complete the restore of any saved counter state, see mqtt_energy_counter
            if !device.energy_counter_restored {
                client.publish_reliably(&topic, "", false).await?;
            }
            let Some(counter) = &device.energy_counter else {
                return Ok(());
            };
            client
                .publish_reliably(&topic, serde_json::to_string(counter)?, true)
                .await?;
            let Some(payload) = energy_state_payload(&device, self.reading.format) else {
                return Ok(());
            };
            return client.publish_obj(&self.sensor.state_topic, payload).await;
        }

        let value = device
            .energy_monitoring_report()
            .and_then(self.reading.format);

        self.sensor
            .notify_state(client, value.as_deref().unwrap_or(PAYLOAD_NONE))
            .await
    }
}

pub struct DeviceStatusDiagnostic {
    sensor: SensorConfig,
    device_id: String,
    state: StateHandle,
}

impl DeviceStatusDiagnostic {
    pub fn new(device: &ServiceDevice, state: &StateHandle) -> Self {
        let unique_id = format!("sensor-{id}-gv2mqtt-status", id = topic_safe_id(device),);

        Self {
            sensor: SensorConfig {
                base: EntityConfig {
                    availability_topic: availability_topic(),
                    name: Some("Status".to_string()),
                    entity_category: Some("diagnostic".to_string()),
                    origin: Origin::default(),
                    device: Device::for_device(device),
                    unique_id: unique_id.clone(),
                    device_class: None,
                    icon: None,
                },
                state_topic: format!("gv2mqtt/sensor/{unique_id}/state"),
                state_class: None,
                json_attributes_topic: Some(format!("gv2mqtt/sensor/{unique_id}/attributes")),
                value_template: None,
                last_reset_value_template: None,
                unit_of_measurement: None,
            },
            device_id: device.id.to_string(),
            state: state.clone(),
        }
    }
}

#[async_trait]
impl EntityInstance for DeviceStatusDiagnostic {
    async fn publish_config(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        self.sensor.publish(state, client).await
    }

    async fn notify_state(&self, client: &HassClient) -> anyhow::Result<()> {
        let device = self
            .state
            .device_by_id(&self.device_id)
            .await
            .expect("device to exist");

        let iot_state = device.compute_iot_device_state();
        let lan_state = device.compute_lan_device_state();
        let http_state = device.compute_http_device_state();
        let platform_metadata = &device.http_device_info;
        let platform_state = &device.http_device_state;
        let device_state = device.device_state();

        let now = Utc::now();

        let threshold = *POLL_INTERVAL + chrono::Duration::seconds(30);

        let summary = match &device_state {
            Some(state) => {
                if now - state.updated > threshold {
                    "Missing".to_string()
                } else {
                    "Available".to_string()
                }
            }
            None => "Unknown".to_string(),
        };

        let attributes = json!({
            "iot": iot_state,
            "lan": lan_state,
            "http": http_state,
            "platform_metadata": platform_metadata,
            "platform_state": platform_state,
            "overall": device_state,
        });

        self.sensor.notify_state(client, &summary).await?;
        if let Some(topic) = &self.sensor.json_attributes_topic {
            client.publish_obj(topic, attributes).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod energy_counter_test {
    use super::*;
    use crate::ble::BigEndian24;
    use crate::service::device::EnergyCounterState;

    fn report(energy_deciwatt_hours: u32) -> NotifyEnergyMonitoring {
        NotifyEnergyMonitoring {
            energy_deciwatt_hours: BigEndian24(energy_deciwatt_hours),
            ..Default::default()
        }
    }

    #[test]
    fn publish_only_tracked_readings() {
        let format: fn(&NotifyEnergyMonitoring) -> Option<String> =
            |r| Some(format!("{:.4}", r.energy_kwh()));
        let mut device = ServiceDevice::new("H5086", "08:E3:98:17:3C:95:2F:EE");

        // Nothing is known yet
        assert!(energy_state_payload(&device, format).is_none());

        // After a restart, the saved state is restored and a reading arrives
        // that the counter hasn't tracked yet, here after a midnight reset
        let before = Utc::now() - chrono::Duration::minutes(10);
        device.restore_energy_counter(EnergyCounterState::new(before, 58518, 86284, 29790));
        device.set_energy_monitoring(report(312));
        assert!(energy_state_payload(&device, format).is_none());

        // Readings aren't tracked before the restore is complete
        device.update_energy_counter(&report(312), 29790 + 560);
        assert!(energy_state_payload(&device, format).is_none());

        // Once it's tracked, it goes out with the matching last_reset
        device.energy_counter_restored = true;
        device.update_energy_counter(&report(312), 29790 + 560);
        let payload = energy_state_payload(&device, format).unwrap();
        assert_eq!(payload.value.as_deref(), Some("0.0312"));
        assert!(payload.last_reset > before);

        // When the readings expire, the value becomes unknown
        device.energy_monitoring = None;
        let payload = energy_state_payload(&device, format).unwrap();
        assert_eq!(payload.value, None);
    }

    #[test]
    fn payloads() {
        let counter =
            EnergyCounterState::new("2026-10-09T04:00:06Z".parse().unwrap(), 70328, 54652, 40);
        let payload = EnergyStatePayload {
            value: Some("7.0328".to_string()),
            last_reset: counter.last_reset,
        };
        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            json!({"value": "7.0328", "last_reset": "2026-10-09T04:00:06Z"})
        );

        // The counter state round-trips through its retained message
        let saved = serde_json::to_string(&counter).unwrap();
        let restored: EnergyCounterState = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored, counter);
    }
}
