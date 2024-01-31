use crate::platform_api::{DeviceCapability, DeviceParameters, EnumOption};
use crate::service::device::Device as ServiceDevice;
use anyhow::anyhow;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;
use std::ops::Range;

#[derive(Default, Debug)]
pub struct ParsedWorkMode {
    pub modes: BTreeMap<String, WorkMode>,
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
            "H7173" => {
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
    pub default_value: Option<JsonValue>,
    pub label: String,
    pub values: Vec<WorkModeValue>,
    pub value_range: Option<Range<i64>>,
}

#[derive(Debug)]
pub struct WorkModeValue {
    pub value: JsonValue,
    pub name: Option<String>,
    pub computed_label: String,
}

impl WorkMode {
    pub fn add_values(&mut self, opt: &EnumOption) {
        self.default_value = opt.extras.get("defaultValue").cloned();

        #[derive(Deserialize)]
        struct ModeRange {
            min: i64,
            max: i64,
        }

        if let Some(range) = opt.extras.get("range") {
            if let Ok(range) = serde_json::from_value::<ModeRange>(range.clone()) {
                self.value_range = Some(range.min..range.max + 1);
                return;
            }
        }

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
            self.values.push(WorkModeValue {
                value: opt.value,
                name: opt.name,
                computed_label: String::new(),
            });
        }

        if let Some(range) = self.contiguous_value_range() {
            self.values.clear();
            self.value_range.replace(range);
        } else {
            // Now spend the resources updating the labels
            for v in &mut self.values {
                let option_name = match &v.name {
                    Some(name) => name.to_string(),
                    None => v.value.to_string(),
                };
                let computed_label = format!("Activate {} Preset {option_name}", self.name);
                v.computed_label = computed_label;
            }
        }
    }

    pub fn label(&self) -> &str {
        if self.label.is_empty() {
            &self.name
        } else {
            &self.label
        }
    }

    pub fn default_value(&self) -> i64 {
        self.default_value
            .as_ref()
            .and_then(|v| v.as_i64())
            .or_else(|| self.values.get(0).and_then(|wmv| wmv.value.as_i64()))
            .or_else(|| self.value_range.as_ref().map(|r| r.start))
            .unwrap_or(0)
    }

    pub fn contiguous_value_range(&self) -> Option<Range<i64>> {
        if let Some(range) = &self.value_range {
            return Some(range.clone());
        }

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

    pub fn should_show_as_preset(&self) -> bool {
        self.contiguous_value_range().is_none() && self.values.is_empty()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::platform_api::{from_json, DeviceCapabilityKind, StructField};
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn test_work_mode_parser() {
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
        assert!(!wm.mode_by_name("Normal").unwrap().should_show_as_preset());

        k9::snapshot!(
            wm,
            r#"
ParsedWorkMode {
    modes: {
        "Normal": WorkMode {
            name: "Normal",
            value: Number(1),
            default_value: None,
            label: "",
            values: [],
            value_range: Some(
                1..9,
            ),
        },
    },
}
"#
        );
    }

    #[test]
    fn test_work_mode_parser2() {
        let cap: DeviceCapability =
            from_json(&include_str!("../../test-data/work-mode-issue-81.json")).unwrap();

        let wm = ParsedWorkMode::with_capability(&cap).unwrap();

        // We shouldn't show this as a set of preset buttons, because
        // we should get a contiguous range that we can show as a slider
        let auto_mode = wm.mode_by_name("Auto").unwrap();
        assert!(auto_mode.contiguous_value_range().is_some());
        k9::snapshot!(
            wm,
            r#"
ParsedWorkMode {
    modes: {
        "Auto": WorkMode {
            name: "Auto",
            value: Number(3),
            default_value: None,
            label: "",
            values: [],
            value_range: Some(
                40..81,
            ),
        },
        "Custom": WorkMode {
            name: "Custom",
            value: Number(2),
            default_value: Some(
                Number(0),
            ),
            label: "",
            values: [],
            value_range: None,
        },
        "Manual": WorkMode {
            name: "Manual",
            value: Number(1),
            default_value: None,
            label: "",
            values: [],
            value_range: Some(
                1..10,
            ),
        },
    },
}
"#
        );
    }

    #[test]
    fn test_work_mode_parser3() {
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
                                            // hole here at 3
                                            {"value": 4},
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

        assert!(wm
            .mode_by_name("Normal")
            .unwrap()
            .contiguous_value_range()
            .is_none());

        k9::snapshot!(
            wm,
            r#"
ParsedWorkMode {
    modes: {
        "Normal": WorkMode {
            name: "Normal",
            value: Number(1),
            default_value: None,
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
                    value: Number(4),
                    name: None,
                    computed_label: "Activate Normal Preset 4",
                },
            ],
            value_range: None,
        },
    },
}
"#
        );
    }

    #[test]
    fn test_work_mode_parser4() {
        let cap: DeviceCapability =
            from_json(&include_str!("../../test-data/work-mode-issue-93.json")).unwrap();

        let wm = ParsedWorkMode::with_capability(&cap).unwrap();

        assert!(!wm.mode_by_name("FanSpeed").unwrap().should_show_as_preset());
        assert!(wm.mode_by_name("Auto").unwrap().should_show_as_preset());
        k9::snapshot!(
            wm,
            r#"
ParsedWorkMode {
    modes: {
        "Auto": WorkMode {
            name: "Auto",
            value: Number(3),
            default_value: Some(
                Number(0),
            ),
            label: "",
            values: [],
            value_range: None,
        },
        "Custom": WorkMode {
            name: "Custom",
            value: Number(2),
            default_value: Some(
                Number(0),
            ),
            label: "",
            values: [],
            value_range: None,
        },
        "FanSpeed": WorkMode {
            name: "FanSpeed",
            value: Number(1),
            default_value: None,
            label: "",
            values: [],
            value_range: Some(
                1..9,
            ),
        },
        "Nature": WorkMode {
            name: "Nature",
            value: Number(6),
            default_value: Some(
                Number(0),
            ),
            label: "",
            values: [],
            value_range: None,
        },
        "Sleep": WorkMode {
            name: "Sleep",
            value: Number(5),
            default_value: Some(
                Number(0),
            ),
            label: "",
            values: [],
            value_range: None,
        },
        "Storm": WorkMode {
            name: "Storm",
            value: Number(7),
            default_value: Some(
                Number(0),
            ),
            label: "",
            values: [],
            value_range: None,
        },
    },
}
"#
        );
    }

    #[test]
    fn test_issue100() {
        let cap: DeviceCapability =
            from_json(&include_str!("../../test-data/work-mode-issue-100.json")).unwrap();

        let mut wm = ParsedWorkMode::with_capability(&cap).unwrap();
        wm.adjust_for_device("H7173");

        k9::snapshot!(
            &wm,
            r#"
ParsedWorkMode {
    modes: {
        "Boiling": WorkMode {
            name: "Boiling",
            value: Number(2),
            default_value: Some(
                Number(0),
            ),
            label: "",
            values: [],
            value_range: None,
        },
        "Coffee": WorkMode {
            name: "Coffee",
            value: Number(4),
            default_value: None,
            label: "",
            values: [],
            value_range: Some(
                1..5,
            ),
        },
        "DIY": WorkMode {
            name: "DIY",
            value: Number(1),
            default_value: None,
            label: "",
            values: [],
            value_range: Some(
                1..5,
            ),
        },
        "Tea": WorkMode {
            name: "Tea",
            value: Number(3),
            default_value: None,
            label: "",
            values: [],
            value_range: Some(
                1..5,
            ),
        },
    },
}
"#
        );

        assert_eq!(wm.mode_by_name("Boiling").unwrap().default_value(), 0);
        assert_eq!(wm.mode_by_name("DIY").unwrap().default_value(), 1);
    }
}
