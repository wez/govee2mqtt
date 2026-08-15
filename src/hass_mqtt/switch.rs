use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::hass_mqtt::instance::{publish_entity_config, EntityInstance};
use crate::platform_api::DeviceCapability;
use crate::service::device::Device as ServiceDevice;
use crate::service::hass::{
    availability_topic, camel_case_to_space_separated, switch_instance_state_topic, topic_safe_id,
    HassClient,
};
use crate::service::state::StateHandle;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::json;

#[derive(Serialize, Clone, Debug)]
pub struct SwitchConfig {
    #[serde(flatten)]
    pub base: EntityConfig,
    pub command_topic: String,
    pub state_topic: String,
    /// Set when we cannot observe the real state and instead infer it (see
    /// `resolve_capability_toggle_state`). Tells hass to treat the entity as
    /// assumed rather than presenting a toggle that implies certainty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optimistic: Option<bool>,
}

impl SwitchConfig {
    pub async fn for_device(
        device: &ServiceDevice,
        instance: &DeviceCapability,
    ) -> anyhow::Result<Self> {
        let command_topic = format!(
            "gv2mqtt/switch/{id}/command/{inst}",
            id = topic_safe_id(device),
            inst = instance.instance
        );
        let state_topic = switch_instance_state_topic(device, &instance.instance);
        let availability_topic = availability_topic();
        let unique_id = format!(
            "gv2mqtt-{id}-{inst}",
            id = topic_safe_id(device),
            inst = instance.instance
        );

        Ok(Self {
            base: EntityConfig {
                availability_topic,
                name: Some(camel_case_to_space_separated(&instance.instance)),
                device_class: None,
                origin: Origin::default(),
                device: Device::for_device(device),
                unique_id,
                entity_category: None,
                icon: None,
            },
            command_topic,
            state_topic,
            // powerSwitch is backed by real LAN/IoT state; only the platform
            // toggles of empty-state devices are guesses.
            optimistic: (instance.instance != "powerSwitch" && device.has_empty_platform_state())
                .then_some(true),
        })
    }

    pub async fn publish(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        publish_entity_config("switch", state, client, &self.base, self).await
    }
}

pub struct CapabilitySwitch {
    switch: SwitchConfig,
    device_id: String,
    state: StateHandle,
    instance_name: String,
}

impl CapabilitySwitch {
    pub async fn new(
        device: &ServiceDevice,
        state: &StateHandle,
        instance: &DeviceCapability,
    ) -> anyhow::Result<Self> {
        let switch = SwitchConfig::for_device(device, instance).await?;
        Ok(Self {
            switch,
            device_id: device.id.to_string(),
            state: state.clone(),
            instance_name: instance.instance.to_string(),
        })
    }
}

/// Resolve ON/OFF for platform toggle capabilities (not powerSwitch).
/// Priority: numeric platform value → optimistic cache → inference →
/// empty-string OFF default.
///
/// The last two steps only apply to devices carrying the
/// `empty_platform_state` quirk (H1310/H1370 ceiling fans): they report an
/// empty string for `fanToggle`, `mainLightToggle`, `reverseAirflowToggle`
/// and friends instead of omitting the capability. Everything else keeps the
/// pre-existing behavior, so a device that legitimately means "unknown" with
/// an empty string stays unknown rather than being defaulted to OFF.
pub fn resolve_capability_toggle_state(device: &ServiceDevice, instance: &str) -> Option<bool> {
    if let Some(cap) = device.get_state_capability_by_instance(instance) {
        if let Some(n) = cap.state.pointer("/value").and_then(|v| v.as_i64()) {
            return Some(n != 0);
        }
    }

    if let Some(on) = device.get_toggle_capability_state(instance) {
        return Some(on);
    }

    if let Some(on) = inferred_toggle_state(device, instance) {
        return Some(on);
    }

    if !device.has_empty_platform_state() {
        return None;
    }

    if let Some(cap) = device.get_state_capability_by_instance(instance) {
        if cap.state.pointer("/value") == Some(&json!("")) {
            return Some(false);
        }
        log::warn!("CapabilitySwitch: unhandled platform state for {instance}: {cap:#?}");
        return Some(false);
    }

    if device.http_device_state.is_some() {
        return Some(false);
    }

    None
}

/// Infer toggle state for H1310/H1370 when Govee returns empty platform values.
pub fn inferred_toggle_state(device: &ServiceDevice, instance: &str) -> Option<bool> {
    if !device.has_empty_platform_state() || !device.needs_platform_poll() {
        return None;
    }

    match instance {
        "mainLightToggle" => device
            .device_state()
            .map(|state| state.brightness > 0 || state.on),
        "fanToggle" => {
            if device.get_mode_capability_label("fanSpeedMode").is_some() {
                return Some(true);
            }
            device
                .get_state_capability_by_instance("fanSpeedMode")
                .and_then(|cap| cap.state.pointer("/value"))
                .and_then(|v| v.as_i64())
                .map(|n| n > 0)
        }
        "reverseAirflowToggle" => None,
        _ => None,
    }
}

fn toggle_mqtt_payload(on: bool) -> &'static str {
    if on {
        "ON"
    } else {
        "OFF"
    }
}

#[async_trait]
impl EntityInstance for CapabilitySwitch {
    async fn publish_config(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        self.switch.publish(state, client).await
    }

    async fn notify_state(&self, client: &HassClient) -> anyhow::Result<()> {
        let device = self
            .state
            .device_by_id(&self.device_id)
            .await
            .expect("device to exist");

        if self.instance_name == "powerSwitch" {
            if let Some(state) = device.device_state() {
                client
                    .publish(&self.switch.state_topic, toggle_mqtt_payload(state.on))
                    .await?;
            }
            return Ok(());
        }

        if let Some(on) = resolve_capability_toggle_state(&device, &self.instance_name) {
            return client
                .publish(&self.switch.state_topic, toggle_mqtt_payload(on))
                .await;
        }

        log::trace!(
            "CapabilitySwitch::notify_state: no state for {device} {instance}",
            instance = self.instance_name
        );
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::platform_api::{from_json, HttpDeviceState};
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct StateFixture {
        payload: HttpDeviceState,
    }

    fn h1310_with_platform_state() -> ServiceDevice {
        let mut device = ServiceDevice::new("H1310", "47:64:F8:9C:BD:BC:DF:4A");
        let fixture: StateFixture =
            from_json(include_str!("../../test-data/h1310_platform_state.json")).unwrap();
        device.set_http_device_state(fixture.payload);
        device
    }

    #[test]
    fn empty_platform_toggle_defaults_off() {
        let device = h1310_with_platform_state();
        assert_eq!(
            resolve_capability_toggle_state(&device, "fanToggle"),
            Some(false)
        );
        assert_eq!(
            resolve_capability_toggle_state(&device, "reverseAirflowToggle"),
            Some(false)
        );
    }

    #[test]
    fn optimistic_toggle_state() {
        let mut device = h1310_with_platform_state();
        device.set_toggle_capability_state("fanToggle", true);
        assert_eq!(
            resolve_capability_toggle_state(&device, "fanToggle"),
            Some(true)
        );
    }

    #[test]
    fn inferred_main_light_from_brightness() {
        let device = h1310_with_platform_state();
        assert_eq!(
            inferred_toggle_state(&device, "mainLightToggle"),
            Some(true)
        );
    }

    #[test]
    fn inferred_fan_from_mode_label() {
        let mut device = h1310_with_platform_state();
        device.set_mode_capability_label("fanSpeedMode", "Speed 3".to_string());
        assert_eq!(inferred_toggle_state(&device, "fanToggle"), Some(true));
        assert_eq!(
            resolve_capability_toggle_state(&device, "fanToggle"),
            Some(true)
        );
    }

    /// Non-H1310/H1370 devices must keep the pre-existing behavior: an
    /// empty-string platform value (Govee's "no meaningful data yet" state)
    /// leaves the entity state unresolved (`None`, reported as "unknown" in
    /// HA) instead of being defaulted to OFF. Only the H1310/H1370 family
    /// has the empty-state quirk that justifies the OFF fallback.
    #[test]
    fn non_h1310_empty_platform_toggle_stays_unknown() {
        let mut device = ServiceDevice::new("H7131", "some-other-device-id");
        device.set_http_device_state(HttpDeviceState {
            sku: "H7131".to_string(),
            device: "some-other-device-id".to_string(),
            capabilities: vec![crate::platform_api::DeviceCapabilityState {
                kind: crate::platform_api::DeviceCapabilityKind::Toggle,
                instance: "gradientToggle".to_string(),
                state: json!({ "value": "" }),
            }],
        });

        assert_eq!(
            resolve_capability_toggle_state(&device, "gradientToggle"),
            None
        );
    }

    fn toggle_capability(instance: &str) -> DeviceCapability {
        DeviceCapability {
            kind: crate::platform_api::DeviceCapabilityKind::Toggle,
            instance: instance.to_string(),
            parameters: None,
            alarm_type: None,
            event_state: None,
        }
    }

    /// `optimistic` must only be emitted where we actually guess the state.
    /// Emitting it everywhere would change the discovery payload, and thus
    /// hass' behavior, for every existing device and switch.
    #[tokio::test]
    async fn optimistic_only_for_guessed_switch_state() {
        let fan = h1310_with_platform_state();
        let fan_toggle = SwitchConfig::for_device(&fan, &toggle_capability("fanToggle"))
            .await
            .unwrap();
        assert_eq!(fan_toggle.optimistic, Some(true));

        // Backed by real LAN/IoT state even on an empty-state device.
        let power = SwitchConfig::for_device(&fan, &toggle_capability("powerSwitch"))
            .await
            .unwrap();
        assert_eq!(power.optimistic, None);

        // A device without the quirk keeps its previous payload exactly.
        let other = ServiceDevice::new("H7131", "some-other-device-id");
        let other_toggle = SwitchConfig::for_device(&other, &toggle_capability("gradientToggle"))
            .await
            .unwrap();
        assert_eq!(other_toggle.optimistic, None);

        let json = serde_json::to_value(&other_toggle).unwrap();
        assert!(
            json.get("optimistic").is_none(),
            "optimistic must be omitted entirely, got {json:#}"
        );
    }

    #[test]
    fn numeric_platform_toggle() {
        let mut device = h1310_with_platform_state();
        let cap = device
            .http_device_state
            .as_mut()
            .unwrap()
            .capabilities
            .iter_mut()
            .find(|c| c.instance == "fanToggle")
            .unwrap();
        cap.state = json!({ "value": 1 });
        assert_eq!(
            resolve_capability_toggle_state(&device, "fanToggle"),
            Some(true)
        );
    }
}
