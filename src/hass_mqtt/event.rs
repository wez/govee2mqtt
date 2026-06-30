use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::hass_mqtt::instance::{publish_entity_config, EntityInstance};
use crate::platform_api::DeviceCapability;
use crate::service::device::Device as ServiceDevice;
use crate::service::hass::{
    availability_topic, camel_case_to_space_separated, event_state_topic, topic_safe_id,
    topic_safe_string, HassClient,
};
use crate::service::state::StateHandle;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

/// Information about an event-type capability, derived from the `eventState`
/// metadata returned for the device by the Govee platform API.
#[derive(Debug, Clone)]
pub struct EventCapabilityInfo {
    /// The list of distinct event type labels that this capability can emit.
    /// Home Assistant requires this to be declared up front when configuring
    /// an MQTT `event` entity.
    pub event_types: Vec<String>,
    /// Maps a normalized capability state value to its event type label, so
    /// that incoming event values can be matched back to a declared event
    /// type regardless of the human-readable name/message in the payload.
    pub label_by_value: HashMap<String, String>,
}

#[derive(Deserialize, Debug)]
struct EventStateMeta {
    #[serde(default)]
    options: Vec<EventOptionMeta>,
}

#[derive(Deserialize, Debug)]
struct EventOptionMeta {
    #[serde(default)]
    name: String,
    #[serde(default)]
    value: JsonValue,
    #[serde(default)]
    message: Option<String>,
}

/// Normalize a capability state value into a stable string key so that
/// incoming event values can be matched against the metadata options
/// regardless of whether the value is encoded as an integer or a string
/// (eg: the integer `1` and the string `"1"` map to the same key).
pub fn value_key(value: &JsonValue) -> String {
    if let Some(i) = value.as_i64() {
        i.to_string()
    } else if let Some(s) = value.as_str() {
        s.to_string()
    } else {
        value.to_string()
    }
}

/// Compute the human-facing event type label for an option or incoming
/// state entry. Prefers the `message` field when it is present and
/// non-empty, otherwise falls back to the `name` field.
pub fn event_type_label(name: &str, message: Option<&str>) -> String {
    match message {
        Some(m) if !m.is_empty() => m.to_string(),
        _ => name.to_string(),
    }
}

/// Parse the `eventState` metadata for an event capability into the set of
/// possible event types and a value->label lookup table.
/// Returns `None` if the capability has no usable event metadata.
pub fn parse_event_capability(cap: &DeviceCapability) -> Option<EventCapabilityInfo> {
    let event_state = cap.event_state.as_ref()?;
    let meta: EventStateMeta = serde_json::from_value(event_state.clone()).ok()?;

    let mut event_types: Vec<String> = vec![];
    let mut label_by_value = HashMap::new();

    for opt in &meta.options {
        let label = event_type_label(&opt.name, opt.message.as_deref());
        if label.is_empty() {
            continue;
        }
        if !event_types.contains(&label) {
            event_types.push(label.clone());
        }
        if !opt.value.is_null() {
            label_by_value.insert(value_key(&opt.value), label);
        }
    }

    if event_types.is_empty() {
        return None;
    }

    Some(EventCapabilityInfo {
        event_types,
        label_by_value,
    })
}

#[derive(Serialize, Clone, Debug)]
pub struct EventConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub state_topic: String,
    pub event_types: Vec<String>,
}

/// Friendly name and icon for an event capability. Govee's instance names
/// are terse and inconsistent, so map the well-known ones to nicer names and
/// icons. Anything unknown falls back to a humanized version of the instance
/// name (with a trailing "Event" stripped) and no icon.
fn event_presentation(instance: &str) -> (String, Option<&'static str>) {
    match instance {
        "iceFull" => ("Ice Maker Full".to_string(), Some("mdi:bucket")),
        "lackWaterEvent" => ("Out of Water".to_string(), Some("mdi:water-alert-outline")),
        _ => {
            // Drop a trailing `Event` so eg: `lackWaterEvent` -> "Lack Water".
            let label = instance.strip_suffix("Event").unwrap_or(instance);
            (camel_case_to_space_separated(label), None)
        }
    }
}

impl EventConfig {
    /// Create an event entity for the given event capability, if it has
    /// usable metadata. Returns `None` when no event types can be derived,
    /// since Home Assistant rejects event entities without an
    /// `event_types` list.
    pub fn new(device: &ServiceDevice, cap: &DeviceCapability) -> Option<Self> {
        let info = parse_event_capability(cap)?;

        let unique_id = format!(
            "gv2mqtt-{id}-event-{inst}",
            id = topic_safe_id(device),
            inst = topic_safe_string(&cap.instance)
        );

        let (name, icon) = event_presentation(&cap.instance);

        Some(Self {
            base: EntityConfig {
                availability_topic: availability_topic(),
                name: Some(name),
                entity_category: None,
                origin: Origin::default(),
                device: Device::for_device(device),
                unique_id,
                device_class: None,
                icon: icon.map(|s| s.to_string()),
            },
            state_topic: event_state_topic(device, &cap.instance),
            event_types: info.event_types,
        })
    }
}

#[async_trait]
impl EntityInstance for EventConfig {
    async fn publish_config(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        publish_entity_config("event", state, client, &self.base, self).await
    }

    async fn notify_state(&self, _client: &HassClient) -> anyhow::Result<()> {
        // Events are momentary and have no persistent state to publish
        // during the regular notification cycle. Event states are published
        // by the platform MQTT subscriber when events actually arrive.
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::platform_api::from_json;

    fn cap(json: &str) -> DeviceCapability {
        from_json(json).unwrap()
    }

    #[test]
    fn parse_lack_water_event() {
        let cap = cap(r#"{
                "type": "devices.capabilities.event",
                "instance": "lackWaterEvent",
                "alarmType": 51,
                "eventState": {
                    "options": [
                        {"name": "lack", "value": 1, "message": "Lack of Water"}
                    ]
                }
            }"#);
        let info = parse_event_capability(&cap).expect("usable metadata");
        assert_eq!(info.event_types, vec!["Lack of Water".to_string()]);
        assert_eq!(
            info.label_by_value.get("1"),
            Some(&"Lack of Water".to_string())
        );
    }

    #[test]
    fn parse_presence_event_without_message() {
        let cap = cap(r#"{
                "type": "devices.capabilities.event",
                "instance": "bodyAppearedEvent",
                "eventState": {
                    "options": [
                        {"name": "Presence", "value": 1},
                        {"name": "Absence", "value": 2}
                    ]
                }
            }"#);
        let info = parse_event_capability(&cap).expect("usable metadata");
        assert_eq!(
            info.event_types,
            vec!["Presence".to_string(), "Absence".to_string()]
        );
        assert_eq!(info.label_by_value.get("1"), Some(&"Presence".to_string()));
        assert_eq!(info.label_by_value.get("2"), Some(&"Absence".to_string()));
    }

    #[test]
    fn parse_event_without_options_is_none() {
        let cap = cap(r#"{
                "type": "devices.capabilities.event",
                "instance": "someEvent",
                "eventState": {"options": []}
            }"#);
        assert!(parse_event_capability(&cap).is_none());
    }

    #[test]
    fn value_key_normalizes() {
        assert_eq!(value_key(&serde_json::json!(1)), "1");
        assert_eq!(value_key(&serde_json::json!("abc")), "abc");
        // An integer and its string form normalize to the same key.
        assert_eq!(
            value_key(&serde_json::json!(1)),
            value_key(&serde_json::json!("1"))
        );
    }

    #[test]
    fn label_prefers_message() {
        assert_eq!(
            event_type_label("lack", Some("Lack of Water")),
            "Lack of Water"
        );
        assert_eq!(event_type_label("Presence", None), "Presence");
        assert_eq!(event_type_label("Presence", Some("")), "Presence");
    }

    #[test]
    fn presentation_known_and_fallback() {
        assert_eq!(
            event_presentation("iceFull"),
            ("Ice Maker Full".to_string(), Some("mdi:bucket"))
        );
        assert_eq!(
            event_presentation("lackWaterEvent"),
            ("Out of Water".to_string(), Some("mdi:water-alert-outline"))
        );
        // Unknown instance: humanized name, trailing "Event" dropped, no icon.
        assert_eq!(
            event_presentation("bodyAppearedEvent"),
            ("Body Appeared".to_string(), None)
        );
    }

    #[test]
    fn event_config_uses_friendly_name_and_icon() {
        let device = ServiceDevice::new("H7172", "9A:52:60:74:F4:48:A5:DE");
        let cap = cap(r#"{
                "type": "devices.capabilities.event",
                "instance": "iceFull",
                "eventState": {
                    "options": [
                        {"name": "iceFull", "value": 1, "message": "ice maker full"}
                    ]
                }
            }"#);
        let ev = EventConfig::new(&device, &cap).expect("entity");
        assert_eq!(ev.base.name.as_deref(), Some("Ice Maker Full"));
        assert_eq!(ev.base.icon.as_deref(), Some("mdi:bucket"));
        assert_eq!(ev.event_types, vec!["ice maker full".to_string()]);
    }
}
