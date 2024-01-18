use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::hass_mqtt::button::ButtonConfig;
use crate::hass_mqtt::climate::TargetTemperatureEntity;
use crate::hass_mqtt::humidifier::Humidifier;
use crate::hass_mqtt::instance::EntityList;
use crate::hass_mqtt::light::DeviceLight;
use crate::hass_mqtt::number::WorkModeNumber;
use crate::hass_mqtt::scene::SceneConfig;
use crate::hass_mqtt::select::WorkModeSelect;
use crate::hass_mqtt::sensor::{CapabilitySensor, DeviceStatusDiagnostic, GlobalFixedDiagnostic};
use crate::hass_mqtt::switch::CapabilitySwitch;
use crate::platform_api::{
    DeviceCapability, DeviceCapabilityKind, DeviceParameters, DeviceType, EnumOption,
};
use crate::service::device::Device as ServiceDevice;
use crate::service::hass::{availability_topic, oneclick_topic, purge_cache_topic};
use crate::service::state::StateHandle;
use crate::version_info::govee_version;
use anyhow::{anyhow, Context};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::ops::Range;
use uuid::Uuid;

pub async fn enumerate_all_entites(state: &StateHandle) -> anyhow::Result<EntityList> {
    let mut entities = EntityList::new();

    enumerate_global_entities(state, &mut entities).await?;
    enumerate_scenes(state, &mut entities).await?;

    let devices = state.devices().await;

    for d in &devices {
        enumerate_entities_for_device(d, state, &mut entities)
            .await
            .with_context(|| format!("Config::for_device({d})"))?;
    }

    Ok(entities)
}

async fn enumerate_global_entities(
    _state: &StateHandle,
    entities: &mut EntityList,
) -> anyhow::Result<()> {
    entities.add(GlobalFixedDiagnostic::new("Version", govee_version()));
    entities.add(ButtonConfig::new("Purge Caches", purge_cache_topic()));
    Ok(())
}

async fn enumerate_scenes(state: &StateHandle, entities: &mut EntityList) -> anyhow::Result<()> {
    if let Some(undoc) = state.get_undoc_client().await {
        match undoc.parse_one_clicks().await {
            Ok(items) => {
                for oc in items {
                    let unique_id = format!(
                        "gv2mqtt-one-click-{}",
                        Uuid::new_v5(&Uuid::NAMESPACE_DNS, oc.name.as_bytes()).simple()
                    );
                    entities.add(SceneConfig {
                        base: EntityConfig {
                            availability_topic: availability_topic(),
                            name: Some(oc.name.to_string()),
                            entity_category: None,
                            origin: Origin::default(),
                            device: Device::this_service(),
                            unique_id: unique_id.clone(),
                            device_class: None,
                            icon: None,
                        },
                        command_topic: oneclick_topic(),
                        payload_on: oc.name,
                    });
                }
            }
            Err(err) => {
                log::warn!("Failed to parse one-clicks: {err:#}");
            }
        }
    }

    Ok(())
}

#[derive(Default, Debug)]
pub struct ParsedWorkMode {
    pub modes: HashMap<String, WorkMode>,
}

impl ParsedWorkMode {
    pub fn with_device(device: &ServiceDevice) -> anyhow::Result<Self> {
        let info = device
            .http_device_info
            .as_ref()
            .ok_or_else(|| anyhow!("no platform state, so no known work mode"))?;
        let cap = info
            .capability_by_instance("workMode")
            .ok_or_else(|| anyhow!("device has no workMode capability"))?;
        let mut parsed = Self::with_capability(cap)?;
        parsed.adjust_for_device(&device.sku);
        Ok(parsed)
    }

    pub fn with_capability(cap: &DeviceCapability) -> anyhow::Result<Self> {
        let mut work_modes = Self::default();

        let wm = cap
            .struct_field_by_name("workMode")
            .ok_or_else(|| anyhow!("workMode not found in {cap:?}"))?;

        match &wm.field_type {
            DeviceParameters::Enum { options } => {
                for opt in options {
                    work_modes.add(opt.name.to_string(), opt.value.clone());
                }
            }
            _ => {}
        }

        if let Some(mv) = cap.struct_field_by_name("modeValue") {
            match &mv.field_type {
                DeviceParameters::Enum { options } => {
                    for opt in options {
                        let mode_name = &opt.name;
                        if let Some(work_mode) = work_modes.get_mut(mode_name) {
                            work_mode.add_values(opt);
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(work_modes)
    }

    pub fn add(&mut self, name: String, value: JsonValue) {
        self.modes.insert(
            name.clone(),
            WorkMode {
                name,
                value,
                ..WorkMode::default()
            },
        );
    }

    pub fn get_mut(&mut self, mode: &str) -> Option<&mut WorkMode> {
        self.modes.get_mut(mode)
    }

    pub fn adjust_for_device(&mut self, sku: &str) {
        match sku {
            "H7160" | "H7143" => {
                self.modes
                    .get_mut("Manual")
                    .map(|m| m.label = "Manual: Mist Level".to_string());
            }
            "H7131" => {
                self.modes.get_mut("gearMode").map(|m| {
                    m.label = "Heat".to_string();
                });
            }
            _ => {
                for mode in self.modes.values_mut() {
                    mode.label = mode.name.clone();
                }
            }
        }
    }

    pub fn mode_for_value(&self, value: &JsonValue) -> Option<&WorkMode> {
        for mode in self.modes.values() {
            if mode.value == *value {
                return Some(mode);
            }
        }
        None
    }

    pub fn mode_by_name(&self, name: &str) -> Option<&WorkMode> {
        self.modes.get(name)
    }

    #[allow(unused)]
    pub fn mode_by_label(&self, name: &str) -> Option<&WorkMode> {
        for mode in self.modes.values() {
            if mode.label() == name {
                return Some(mode);
            }
        }
        None
    }

    pub fn get_mode_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self
            .modes
            .values()
            .map(|mode| mode.name.to_string())
            .collect();
        names.sort();
        names
    }

    #[allow(unused)]
    pub fn get_mode_labels(&self) -> Vec<String> {
        let mut names: Vec<_> = self
            .modes
            .values()
            .map(|mode| mode.label().to_string())
            .collect();
        names.sort();
        names
    }

    #[allow(unused)]
    pub fn modes_with_values(&self) -> impl Iterator<Item = &WorkMode> {
        self.modes.values().filter_map(|mode| {
            if mode.values.is_empty() {
                None
            } else {
                Some(mode)
            }
        })
    }
}

#[derive(Default, Debug)]
pub struct WorkMode {
    pub name: String,
    pub value: JsonValue,
    pub label: String,
    pub values: Vec<WorkModeValue>,
}

#[derive(Debug)]
pub struct WorkModeValue {
    pub value: JsonValue,
    pub name: Option<String>,
    pub computed_label: String,
}

impl WorkMode {
    pub fn add_values(&mut self, opt: &EnumOption) {
        #[derive(Deserialize)]
        struct ModeOption {
            name: Option<String>,
            value: JsonValue,
        }

        let Some(options) = opt.extras.get("options") else {
            return;
        };

        let Ok(options) = serde_json::from_value::<Vec<ModeOption>>(options.clone()) else {
            return;
        };

        for opt in options {
            let option_name = match &opt.name {
                Some(name) => name.to_string(),
                None => opt.value.to_string(),
            };
            let computed_label = format!("Activate {} Preset {option_name}", self.name);
            self.values.push(WorkModeValue {
                value: opt.value,
                name: opt.name,
                computed_label,
            });
        }
    }

    pub fn label(&self) -> &str {
        if self.label.is_empty() {
            &self.name
        } else {
            &self.label
        }
    }

    pub fn contiguous_value_range(&self) -> Option<Range<i64>> {
        let mut values = vec![];
        for v in &self.values {
            let item_value = v.value.as_i64()?;
            if v.name.is_some() {
                // It's a preset mode, so it's not a contiguous
                // slider value
                return None;
            }
            values.push(item_value);
        }
        values.sort();

        let min = *values.iter().min()?;
        let max = *values.iter().max()?;

        let mut expect = min;
        for item in values {
            if item != expect {
                return None;
            }
            expect = expect + 1;
        }

        Some(min..max + 1)
    }
}

#[cfg(test)]
#[test]
fn test_work_mode_parser() {
    use crate::platform_api::StructField;
    use serde_json::json;

    let cap = DeviceCapability {
        kind: DeviceCapabilityKind::WorkMode,
        instance: "workMode".to_string(),
        alarm_type: None,
        event_state: None,
        parameters: Some(DeviceParameters::Struct {
            fields: vec![
                StructField {
                    field_name: "workMode".to_string(),
                    field_type: DeviceParameters::Enum {
                        options: vec![EnumOption {
                            name: "Normal".to_string(),
                            value: 1.into(),
                            extras: HashMap::new(),
                        }],
                    },
                    default_value: None,
                    required: true,
                },
                StructField {
                    field_name: "modeValue".to_string(),
                    field_type: DeviceParameters::Enum {
                        options: vec![EnumOption {
                            name: "Normal".to_string(),
                            value: JsonValue::Null,
                            extras: [(
                                "options".to_string(),
                                json!([
                                        {"value": 1},
                                        {"value": 2},
                                        {"value": 3},
                                        {"value": 4},
                                        {"value": 5},
                                        {"value": 6},
                                        {"value": 7},
                                        {"value": 8},
                                ]),
                            )]
                            .into_iter()
                            .collect(),
                        }],
                    },
                    default_value: None,
                    required: true,
                },
            ],
        }),
    };

    let wm = ParsedWorkMode::with_capability(&cap).unwrap();

    // We shouldn't show this as a set of preset buttons, because
    // we should get a contiguous range that we can show as a slider
    assert!(wm
        .mode_by_name("Normal")
        .unwrap()
        .contiguous_value_range()
        .is_some());

    k9::snapshot!(
        wm,
        r#"
ParsedWorkMode {
    modes: {
        "Normal": WorkMode {
            name: "Normal",
            value: Number(1),
            label: "",
            values: [
                WorkModeValue {
                    value: Number(1),
                    name: None,
                    computed_label: "Activate Normal Preset 1",
                },
                WorkModeValue {
                    value: Number(2),
                    name: None,
                    computed_label: "Activate Normal Preset 2",
                },
                WorkModeValue {
                    value: Number(3),
                    name: None,
                    computed_label: "Activate Normal Preset 3",
                },
                WorkModeValue {
                    value: Number(4),
                    name: None,
                    computed_label: "Activate Normal Preset 4",
                },
                WorkModeValue {
                    value: Number(5),
                    name: None,
                    computed_label: "Activate Normal Preset 5",
                },
                WorkModeValue {
                    value: Number(6),
                    name: None,
                    computed_label: "Activate Normal Preset 6",
                },
                WorkModeValue {
                    value: Number(7),
                    name: None,
                    computed_label: "Activate Normal Preset 7",
                },
                WorkModeValue {
                    value: Number(8),
                    name: None,
                    computed_label: "Activate Normal Preset 8",
                },
            ],
        },
    },
}
"#
    );
}

async fn entities_for_work_mode<'a>(
    d: &ServiceDevice,
    state: &StateHandle,
    cap: &DeviceCapability,
    entities: &mut EntityList,
) -> anyhow::Result<()> {
    let mut work_modes = ParsedWorkMode::with_capability(cap)?;
    work_modes.adjust_for_device(&d.sku);

    let quirk = d.resolve_quirk();

    for work_mode in work_modes.modes.values() {
        let Some(mode_num) = work_mode.value.as_i64() else {
            continue;
        };

        let range = work_mode.contiguous_value_range();

        let show_as_preset = work_mode.values.is_empty()
            || range.is_none()
            || quirk
                .as_ref()
                .map(|q| q.should_show_mode_as_preset(&work_mode.name))
                .unwrap_or(false);

        if show_as_preset {
            if work_mode.values.is_empty() {
                entities.add(ButtonConfig::activate_work_mode_preset(
                    d,
                    &format!("Activate Mode: {}", work_mode.label()),
                    &work_mode.name,
                    mode_num,
                    0,
                ));
            } else {
                for value in &work_mode.values {
                    if let Some(mode_value) = value.value.as_i64() {
                        entities.add(ButtonConfig::activate_work_mode_preset(
                            d,
                            &value.computed_label,
                            &work_mode.name,
                            mode_num,
                            mode_value,
                        ));
                    }
                }
            }
        } else {
            let label = work_mode.label().to_string();

            entities.add(WorkModeNumber::new(
                d,
                state,
                label,
                &work_mode.name,
                work_mode.value.clone(),
                range,
            ));
        }
    }

    entities.add(WorkModeSelect::new(d, &work_modes, state));

    Ok(())
}

pub async fn enumerate_entities_for_device<'a>(
    d: &'a ServiceDevice,
    state: &StateHandle,
    entities: &mut EntityList,
) -> anyhow::Result<()> {
    if !d.is_controllable() {
        return Ok(());
    }

    entities.add(DeviceStatusDiagnostic::new(d, state));
    entities.add(ButtonConfig::request_platform_data_for_device(d));

    if d.supports_rgb() || d.get_color_temperature_range().is_some() || d.supports_brightness() {
        entities.add(DeviceLight::for_device(&d, state, None).await?);
    }

    if d.device_type() == DeviceType::Humidifier {
        entities.add(Humidifier::new(&d, state).await?);
    }

    if let Some(info) = &d.http_device_info {
        for cap in &info.capabilities {
            match &cap.kind {
                DeviceCapabilityKind::Toggle | DeviceCapabilityKind::OnOff => {
                    entities.add(CapabilitySwitch::new(&d, state, cap).await?);
                }
                DeviceCapabilityKind::ColorSetting
                | DeviceCapabilityKind::SegmentColorSetting
                | DeviceCapabilityKind::MusicSetting
                | DeviceCapabilityKind::Event
                | DeviceCapabilityKind::DynamicScene => {}

                DeviceCapabilityKind::Range if cap.instance == "brightness" => {}
                DeviceCapabilityKind::Range if cap.instance == "humidity" => {}
                DeviceCapabilityKind::WorkMode => {
                    entities_for_work_mode(d, state, cap, entities).await?;
                }

                DeviceCapabilityKind::Property => {
                    entities.add(CapabilitySensor::new(&d, state, cap).await?);
                }

                DeviceCapabilityKind::TemperatureSetting => {
                    entities.add(TargetTemperatureEntity::new(&d, state, cap).await?);
                }

                kind => {
                    log::warn!(
                        "Do something about {kind:?} {} for {d} {cap:?}",
                        cap.instance
                    );
                }
            }
        }

        if let Some(segments) = info.supports_segmented_rgb() {
            for n in segments {
                entities.add(DeviceLight::for_device(&d, state, Some(n)).await?);
            }
        }
    }
    Ok(())
}
