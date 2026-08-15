use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::hass_mqtt::instance::EntityInstance;
use crate::hass_mqtt::select::SelectConfig;
use crate::platform_api::{DeviceCapability, DeviceParameters};
use crate::service::device::Device as ServiceDevice;
use crate::service::hass::{
    availability_topic, camel_case_to_space_separated, topic_safe_id, topic_safe_string, HassClient,
};
use crate::service::state::StateHandle;
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use mosquitto_rs::router::{Params, Payload, State};
use serde::Deserialize;
use serde_json::Value as JsonValue;

pub fn mode_capability_display_name(instance: &str) -> String {
    match instance {
        "fanSpeedMode" => "Fan Speed".to_string(),
        other => camel_case_to_space_separated(other),
    }
}

pub fn mode_option_labels(cap: &DeviceCapability) -> Vec<String> {
    match &cap.parameters {
        Some(DeviceParameters::Enum { options }) => {
            options.iter().map(|o| o.name.clone()).collect()
        }
        _ => vec![],
    }
}

pub fn mode_value_for_label(cap: &DeviceCapability, label: &str) -> Option<JsonValue> {
    match &cap.parameters {
        Some(DeviceParameters::Enum { options }) => options
            .iter()
            .find(|o| o.name == label)
            .map(|o| o.value.clone()),
        _ => None,
    }
}

pub fn mode_label_for_platform_value(cap: &DeviceCapability, value: &JsonValue) -> Option<String> {
    if value.as_str() == Some("") {
        return None;
    }
    match &cap.parameters {
        Some(DeviceParameters::Enum { options }) => options
            .iter()
            .find(|o| &o.value == value)
            .map(|o| o.name.clone()),
        _ => None,
    }
}

pub struct CapabilityModeSelect {
    select: SelectConfig,
    device_id: String,
    state: StateHandle,
    instance_name: String,
}

impl CapabilityModeSelect {
    pub async fn new(
        device: &ServiceDevice,
        state: &StateHandle,
        cap: &DeviceCapability,
    ) -> anyhow::Result<Self> {
        let options = mode_option_labels(cap);
        if options.is_empty() {
            anyhow::bail!("mode capability {} has no enum options", cap.instance);
        }

        let instance = topic_safe_string(&cap.instance);
        let command_topic = format!(
            "gv2mqtt/select/{id}/command/{instance}",
            id = topic_safe_id(device),
        );
        let state_topic = format!(
            "gv2mqtt/select/{id}/state/{instance}",
            id = topic_safe_id(device),
        );
        let unique_id = format!("gv2mqtt-{id}-{instance}", id = topic_safe_id(device),);

        Ok(Self {
            select: SelectConfig {
                base: EntityConfig {
                    availability_topic: availability_topic(),
                    name: Some(mode_capability_display_name(&cap.instance)),
                    device_class: None,
                    origin: Origin::default(),
                    device: Device::for_device(device),
                    unique_id,
                    entity_category: None,
                    icon: None,
                },
                command_topic,
                state_topic,
                options,
                // These devices report an empty platform value for the mode,
                // so the shown option is whatever we last sent.
                optimistic: device.has_empty_platform_state().then_some(true),
            },
            device_id: device.id.to_string(),
            state: state.clone(),
            instance_name: cap.instance.clone(),
        })
    }
}

#[async_trait]
impl EntityInstance for CapabilityModeSelect {
    async fn publish_config(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        self.select.publish(state, client).await
    }

    async fn notify_state(&self, client: &HassClient) -> anyhow::Result<()> {
        let device = self
            .state
            .device_by_id(&self.device_id)
            .await
            .expect("device to exist");

        let cap = device
            .get_capability_by_instance(&self.instance_name)
            .context("mode capability metadata missing")?;

        if let Some(state_cap) = device.get_state_capability_by_instance(&self.instance_name) {
            if let Some(platform_value) = state_cap.state.pointer("/value") {
                if let Some(label) = mode_label_for_platform_value(cap, platform_value) {
                    return client.publish(&self.select.state_topic, label).await;
                }
            }
        }

        if let Some(label) = device.get_mode_capability_label(&self.instance_name) {
            return client.publish(&self.select.state_topic, label).await;
        }

        log::trace!(
            "CapabilityModeSelect::notify_state: no state for {device} {}",
            self.instance_name
        );
        Ok(())
    }
}

#[derive(Deserialize)]
pub(crate) struct IdAndInstance {
    id: String,
    instance: String,
}

pub async fn mqtt_capability_mode_command(
    Payload(label): Payload<String>,
    Params(IdAndInstance { id, instance }): Params<IdAndInstance>,
    State(state): State<StateHandle>,
) -> anyhow::Result<()> {
    log::info!("mode {instance} for {id}: {label}");
    let device = state.resolve_device_for_control(&id).await?;

    let cap = device
        .get_capability_by_instance(&instance)
        .ok_or_else(|| anyhow!("device has no mode capability {instance}"))?;

    let value = mode_value_for_label(cap, &label)
        .ok_or_else(|| anyhow!("mode label {label} not found for {instance}"))?;

    state
        .device_set_mode_capability(&device, cap, &label, value)
        .await?;

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::platform_api::{from_json, DeviceCapabilityKind, EnumOption, HttpDeviceInfo};
    use std::collections::HashMap;

    fn h1310_fan_speed_cap() -> DeviceCapability {
        DeviceCapability {
            kind: DeviceCapabilityKind::Mode,
            instance: "fanSpeedMode".to_string(),
            parameters: Some(DeviceParameters::Enum {
                options: (1..=6)
                    .map(|n| EnumOption {
                        name: format!("Speed {n}"),
                        value: JsonValue::from(n),
                        extras: HashMap::new(),
                    })
                    .collect(),
            }),
            alarm_type: None,
            event_state: None,
        }
    }

    #[test]
    fn mode_label_value_roundtrip() {
        let cap = h1310_fan_speed_cap();
        assert_eq!(mode_option_labels(&cap).len(), 6);
        let value = mode_value_for_label(&cap, "Speed 3").unwrap();
        assert_eq!(value, JsonValue::from(3));
        let label = mode_label_for_platform_value(&cap, &JsonValue::from(3)).unwrap();
        assert_eq!(label, "Speed 3");
        assert!(mode_label_for_platform_value(&cap, &JsonValue::from("")).is_none());
    }

    #[test]
    fn fan_speed_display_name() {
        assert_eq!(mode_capability_display_name("fanSpeedMode"), "Fan Speed");
    }

    #[test]
    fn h1310_fan_speed_mode_from_metadata() {
        let info: HttpDeviceInfo =
            from_json(include_str!("../../test-data/h1310_platform_metadata.json")).unwrap();
        let cap = info.capability_by_instance("fanSpeedMode").unwrap();
        assert_eq!(cap.kind, DeviceCapabilityKind::Mode);
        k9::assert_matches_snapshot!(format!("{:?}", mode_option_labels(cap)));
    }

    #[derive(Deserialize)]
    struct DevicesFixture {
        data: Vec<HttpDeviceInfo>,
    }

    /// Mode capabilities are exposed as a select for every device, not just
    /// H1310/H1370 (see `enumerator.rs`). This confirms a pre-existing,
    /// unrelated device family (H7131's `nightlightScene`) still resolves to
    /// a usable, non-empty option list via the platform metadata, i.e. that
    /// `CapabilityModeSelect::new` would succeed for it rather than bail.
    #[test]
    fn h7131_nightlight_scene_mode_capability_is_usable() {
        let fixture: DevicesFixture =
            from_json(include_str!("../../test-data/list_devices_issue4.json")).unwrap();
        let device = fixture
            .data
            .iter()
            .find(|d| d.sku == "H7131")
            .expect("H7131 fixture device");
        let cap = device
            .capability_by_instance("nightlightScene")
            .expect("nightlightScene capability");
        assert_eq!(cap.kind, DeviceCapabilityKind::Mode);

        let labels = mode_option_labels(cap);
        assert_eq!(labels, vec!["Flame", "Rainbow", "Rhythm", "Easy", "Sleep"]);

        let value = mode_value_for_label(cap, "Rhythm").expect("value for Rhythm");
        assert_eq!(
            mode_label_for_platform_value(cap, &value).as_deref(),
            Some("Rhythm")
        );
    }
}
