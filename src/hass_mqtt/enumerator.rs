use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::hass_mqtt::button::ButtonConfig;
use crate::hass_mqtt::humidifier::Humidifier;
use crate::hass_mqtt::instance::EntityList;
use crate::hass_mqtt::light::DeviceLight;
use crate::hass_mqtt::number::WorkModeNumber;
use crate::hass_mqtt::scene::SceneConfig;
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

#[derive(Default)]
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
            _ => {
                for mode in self.modes.values_mut() {
                    mode.label = format!("{} Parameter", mode.name);
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

    pub fn get_mode_labels(&self) -> Vec<String> {
        let mut names: Vec<_> = self
            .modes
            .values()
            .map(|mode| mode.label().to_string())
            .collect();
        names.sort();
        names
    }

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

#[derive(Default)]
pub struct WorkMode {
    pub name: String,
    pub value: JsonValue,
    pub label: String,
    pub values: Vec<WorkModeValue>,
}

impl WorkMode {
    pub fn add_values(&mut self, opt: &EnumOption) {
        #[derive(Deserialize)]
        struct ModeOption {
            value: JsonValue,
        }

        let Some(options) = opt.extras.get("options") else {
            return;
        };

        let Ok(options) = serde_json::from_value::<Vec<ModeOption>>(options.clone()) else {
            return;
        };

        for opt in options {
            let label = opt.value.to_string();
            self.values.push(WorkModeValue {
                value: opt.value,
                label,
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
            values.push(v.value.as_i64()?);
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

pub struct WorkModeValue {
    pub value: JsonValue,
    pub label: String,
}

async fn entities_for_work_mode<'a>(
    d: &ServiceDevice,
    state: &StateHandle,
    cap: &DeviceCapability,
    entities: &mut EntityList,
) -> anyhow::Result<()> {
    let mut work_modes = ParsedWorkMode::with_capability(cap)?;
    work_modes.adjust_for_device(&d.sku);

    for work_mode in work_modes.modes_with_values() {
        let range = work_mode.contiguous_value_range();
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
                    let sensor = CapabilitySensor::new(&d, state, cap).await?;

                    entities.add(sensor.clone());
                    // Synthesize a variant that reports in F
                    if let Some(f) = sensor.into_temperature_farenheit() {
                        entities.add(f);
                    }
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
