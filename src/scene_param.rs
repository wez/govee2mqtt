//! Decoding and manipulation of Govee scene parameters.
//!
//! The `scenceParam` blob that Govee publishes via their light-effect-libraries
//! endpoint is a sequence of layers; each layer encodes the colors and
//! movement/animation parameters for a scene.  This module implements
//! enough of the format to locate and patch the speed fields.
//!
//! The format is:
//! ```text
//! [num_layers: u8]
//! for each layer:
//!   [len: u8]
//!   [applied_area: u8]
//!   [select_type: u8]
//!   [select_param_1: u8]
//!   [select_param_2: u8]
//!   [layer_flags: u8]
//!   [num_brightness_blocks: u8]
//!   for each brightness block:
//!     [scope_start, scope_end, order, change_speed,
//!      brightest_retention, darkest_retention: 6 * u8]
//!   [direction_distribution: u8]
//!   [colour_speed: u8]
//!   [colour_retention: u8]
//!   [num_palette: u8]
//!   for each palette entry:
//!     [r, g, b: 3 * u8]
//!   [selected_area_movement: packed, interval, speed = 3 * u8]
//!   [overall_movement: packed, interval, speed = 3 * u8]
//!   [priority: u8]
//! [padding: 0x00 ...]
//! ```
//! For reference, see:
//! <https://github.com/AlgoClaw/Govee/blob/main/decoded/v1.2/explanation_v1.2.md>
//! <https://github.com/teh-hippo/ha-govee-led-ble/blob/master/tools/ble/kaitai/govee_shared.ksy>
use anyhow::Context;
use serde_json::Value as JsonValue;

/// The speed control information that Govee publishes for a scene in
/// the `speedInfo` field of their light-effect-libraries endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneSpeedConfig {
    pub pages: Vec<SceneSpeedPage>,
}

/// Speed control info for a single layer of a scene.  The `page`
/// field is the layer index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneSpeedPage {
    pub page: usize,
    /// The index of the default speed option for this page
    pub default_index: usize,
    /// The values for the `selected_area_movement.speed` field
    pub move_in: Vec<u8>,
    /// The values for the `overall_movement.speed` field
    pub move_all: Vec<u8>,
    /// The values for the `colour_speed` field
    pub colour_speed: Vec<u8>,
    /// The values for the `brightness_blocks[*].change_speed` fields
    pub brightness: Vec<SceneBrightnessSpeed>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneBrightnessSpeed {
    /// The index of the brightness block within the layer
    pub block: usize,
    /// The values for the `change_speed` field
    pub values: Vec<u8>,
}

impl SceneSpeedConfig {
    /// The number of discrete speed settings that are available.
    /// This is the maximum number of options across all pages;
    /// pages with fewer options are clamped when applying a speed.
    pub fn option_count(&self) -> usize {
        self.pages
            .iter()
            .flat_map(|p| {
                [&p.move_in, &p.move_all, &p.colour_speed]
                    .into_iter()
                    .chain(p.brightness.iter().map(|b| &b.values))
                    .map(|v| v.len())
            })
            .max()
            .unwrap_or(0)
    }

    /// Returns true if this scene has a usable speed control
    pub fn has_speed_control(&self) -> bool {
        self.option_count() >= 2
    }

    /// The default speed index for the scene; derived from the
    /// first page that reports a default.
    #[cfg(test)]
    pub fn default_index(&self) -> usize {
        self.pages.first().map(|p| p.default_index).unwrap_or(0)
    }
}

fn parse_byte_array(value: Option<&JsonValue>) -> Vec<u8> {
    value
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_u64())
                .map(|n| n as u8)
                .collect()
        })
        .unwrap_or_default()
}

/// Parse the `speedInfo` field from Govee's light-effect-libraries
/// endpoint into a typed speed config.  Returns `None` if the scene
/// doesn't support speed control, or if the metadata is malformed.
pub fn scene_speed_config(speed_info: &JsonValue) -> Option<SceneSpeedConfig> {
    if !speed_info.get("supSpeed")?.as_bool()? {
        return None;
    }

    // The config is a JSON array that has itself been serialized as a string
    let config_text = speed_info.get("config")?.as_str()?;
    let pages_json: JsonValue = serde_json::from_str(config_text).ok()?;
    let pages = pages_json.as_array()?;

    let mut result = vec![];
    for page_json in pages {
        let page = page_json.get("page")?.as_u64()? as usize;
        let default_index = page_json
            .get("defaultIndex")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        let brightness = page_json
            .get("bright")
            .and_then(|v| v.as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|block| {
                        let index = match block.get("brightPage") {
                            Some(JsonValue::Number(n)) => n.as_u64()? as usize,
                            Some(JsonValue::String(s)) => s.parse::<usize>().ok()?,
                            _ => return None,
                        };
                        Some(SceneBrightnessSpeed {
                            block: index,
                            values: parse_byte_array(block.get("brightValue")),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        result.push(SceneSpeedPage {
            page,
            default_index,
            move_in: parse_byte_array(page_json.get("moveIn")),
            move_all: parse_byte_array(page_json.get("moveAll")),
            colour_speed: parse_byte_array(page_json.get("colourSpeed")),
            brightness,
        });
    }

    let config = SceneSpeedConfig { pages: result };

    if !config.has_speed_control() {
        return None;
    }

    Some(config)
}

/// The absolute byte offsets of the speed fields within a layer body
#[derive(Debug, PartialEq, Eq)]
struct LayerSpeedOffsets {
    colour_speed: usize,
    selected_movement_speed: usize,
    overall_movement_speed: usize,
    brightness_speeds: Vec<usize>,
}

fn parse_layer_speed_offsets(body: &[u8]) -> anyhow::Result<LayerSpeedOffsets> {
    if body.len() < 6 {
        anyhow::bail!("layer body is too short: {body:?}");
    }
    let num_brightness_blocks = body[5] as usize;

    // direction_distribution is the byte that follows the
    // brightness blocks
    let dir_distribution_offset = 6 + 6 * num_brightness_blocks;

    let mut brightness_speeds = vec![];
    for i in 0..num_brightness_blocks {
        // change_speed is the 4th byte in each brightness block
        brightness_speeds.push(6 + 6 * i + 3);
    }

    let colour_speed = dir_distribution_offset + 1;
    let num_palette_offset = dir_distribution_offset + 3;
    if num_palette_offset >= body.len() {
        anyhow::bail!("layer body is truncated: {body:?}");
    }
    let num_palette = body[num_palette_offset] as usize;

    let selected_area_movement = dir_distribution_offset + 4 + 3 * num_palette;
    let overall_movement = selected_area_movement + 3;

    // Each movement is 3 bytes: packed, interval, speed
    let selected_movement_speed = selected_area_movement + 2;
    let overall_movement_speed = overall_movement + 2;

    // +1 for the priority byte that follows the movements
    if overall_movement_speed + 1 >= body.len() {
        anyhow::bail!("layer body is truncated: {body:?}");
    }

    Ok(LayerSpeedOffsets {
        colour_speed,
        selected_movement_speed,
        overall_movement_speed,
        brightness_speeds,
    })
}

fn parse_layers(data: &[u8]) -> anyhow::Result<Vec<(usize, LayerSpeedOffsets)>> {
    let num_layers = *data.first().context("scene param is empty")? as usize;
    let mut off = 1usize;
    let mut layers = vec![];

    for i in 0..num_layers {
        let len = *data
            .get(off)
            .with_context(|| format!("layer {i} is missing its length"))?
            as usize;
        off += 1;
        let body_end = off
            .checked_add(len)
            .with_context(|| format!("layer {i} length overflow"))?;
        let body = data
            .get(off..body_end)
            .with_context(|| format!("layer {i} is truncated"))?;

        layers.push((
            off,
            parse_layer_speed_offsets(body).with_context(|| format!("layer {i} is malformed"))?,
        ));

        off = body_end;
    }

    // The bytes that follow the layers must be zero padding
    if let Some(non_zero) = data[off..].iter().find(|b| **b != 0) {
        anyhow::bail!("scene param has unexpected non-zero padding byte: {non_zero:#x}");
    }

    Ok(layers)
}

/// A requested speed for a scene
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneSpeed {
    /// One of the discrete options from Govee's catalog (0-based)
    Option(usize),
    /// A percentage (0-100) where 50 is the scene's default speed;
    /// see [`value_for_percent`]
    Percent(u8),
}

impl SceneSpeed {
    pub fn apply(&self, scence_param: &str, config: &SceneSpeedConfig) -> anyhow::Result<String> {
        match *self {
            Self::Option(index) => apply_scene_speed(scence_param, config, index),
            Self::Percent(percent) => apply_scene_speed_percent(scence_param, config, percent),
        }
    }
}

/// The catalog's speed options rarely span the full range that the
/// device accepts; percentages extend past the slowest and fastest
/// options by the width of that span, but by at least this much.
const MIN_PERCENT_EXTENSION: i32 = 64;

/// Map a speed percentage onto a byte value for a single speed field.
///
/// 50% is the scene's default option. 0% and 100% extend below the
/// slowest and above the fastest catalog options respectively (higher
/// values animate faster), clamped to the valid 0-255 byte range.
/// Values in between are linearly interpolated.
pub fn value_for_percent(values: &[u8], default_index: usize, percent: u8) -> u8 {
    let Some(&default) = values.get(default_index.min(values.len().saturating_sub(1))) else {
        return 0;
    };
    let lo = *values.iter().min().expect("non-empty") as i32;
    let hi = *values.iter().max().expect("non-empty") as i32;
    let default = default as i32;
    let margin = (hi - lo).max(MIN_PERCENT_EXTENSION);
    let slowest = (lo - margin).max(0);
    let fastest = (hi + margin).min(255);

    let percent = percent.min(100) as i32;
    let (from, to, step) = if percent <= 50 {
        (slowest, default, percent)
    } else {
        (default, fastest, percent - 50)
    };
    // round to the nearest integer
    (from + ((to - from) * step * 2 + 50).div_euclid(100)) as u8
}

/// Rewrites the speed fields in the supplied `scence_param` (base64
/// encoded) so that they correspond to the supplied speed option
/// index, returning the updated base64 encoded parameter.
///
/// Only the speed fields indicated by the `config` are modified;
/// every other byte in the parameter is preserved.
pub fn apply_scene_speed(
    scence_param: &str,
    config: &SceneSpeedConfig,
    index: usize,
) -> anyhow::Result<String> {
    apply_scene_speed_with(scence_param, config, |values, _default_index| {
        values[index.min(values.len() - 1)]
    })
}

/// Like [`apply_scene_speed`], but sets each speed field to the value
/// computed by [`value_for_percent`]
pub fn apply_scene_speed_percent(
    scence_param: &str,
    config: &SceneSpeedConfig,
    percent: u8,
) -> anyhow::Result<String> {
    apply_scene_speed_with(scence_param, config, |values, default_index| {
        value_for_percent(values, default_index, percent)
    })
}

/// Rewrites every speed field listed in `config`, using `pick` to
/// choose a value from that field's catalog options and the page's
/// default option index.
fn apply_scene_speed_with(
    scence_param: &str,
    config: &SceneSpeedConfig,
    pick: impl Fn(&[u8], usize) -> u8,
) -> anyhow::Result<String> {
    let mut data = data_encoding::BASE64
        .decode(scence_param.as_bytes())
        .context("scence_param is not valid base64")?;

    let layers = parse_layers(&data).context("unable to parse scence_param")?;

    for page in &config.pages {
        let (body_start, offsets) = layers
            .get(page.page)
            .with_context(|| format!("speed config references missing layer {}", page.page))?;

        if !page.move_in.is_empty() {
            data[body_start + offsets.selected_movement_speed] =
                pick(&page.move_in, page.default_index);
        }
        if !page.move_all.is_empty() {
            data[body_start + offsets.overall_movement_speed] =
                pick(&page.move_all, page.default_index);
        }
        if !page.colour_speed.is_empty() {
            data[body_start + offsets.colour_speed] = pick(&page.colour_speed, page.default_index);
        }
        for block in &page.brightness {
            if block.values.is_empty() {
                continue;
            }
            let offset = offsets
                .brightness_speeds
                .get(block.block)
                .with_context(|| {
                    format!(
                        "speed config references missing brightness block {} in layer {}",
                        block.block, page.page
                    )
                })?;
            data[body_start + offset] = pick(&block.values, page.default_index);
        }
    }

    Ok(data_encoding::BASE64.encode(&data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// River (H61F2): 3 layers; the speed control is the
    /// selected_movement.speed of layer 0
    const RIVER_PARAM: &str = "AyMAAAABAAGbmwKAFBQDgBQEIf53Vf6mPP72Mv+5FADyAACAACAAAgEBAAEyAADJFKoAgBQD/3xL/0FO/+kyEwD0AACAACMAAAABAAExEQCAFBQDgBQEG0L/KJT/PP72Mv+5EAD5AACAAA==";
    const RIVER_SPEED_INFO: &str = r#"{
        "config": "[{\"page\":0,\"defaultIndex\":2,\"moveIn\":[242,249,252]}]",
        "speedIndex": 0,
        "supSpeed": true
    }"#;

    /// Forest (H61F2): 3 layers; the speed control is the brightness
    /// block change_speed of layers 0 and 2
    const FOREST_PARAM: &str = "AyMAAgcDAgH/FAPMAQEB+goEBP8A/89kR////+kWAAAAAAAAACAAAAABAgH/KAHIFBQD7hQDAP8ALP9/AMA/AAAAAAAAACMAAgMCAgH/FAPOAQEB+goE/95dlf7/R////23mAAAAAAAAAA==";
    const FOREST_SPEED_INFO: &str = r#"{
        "config": "[{\"bright\":[{\"brightPage\":\"0\",\"brightValue\":[204,204,216,234]}],\"defaultIndex\":1,\"page\":0},{\"page\":2,\"bright\":[{\"brightPage\":\"0\",\"brightValue\":[206,211,216,234]}],\"defaultIndex\":1}]",
        "speedIndex": 0,
        "supSpeed": true
    }"#;

    fn decode(param: &str) -> Vec<u8> {
        data_encoding::BASE64.decode(param.as_bytes()).unwrap()
    }

    #[test]
    fn test_parse_layers_river() {
        let data = decode(RIVER_PARAM);
        let layers = parse_layers(&data).unwrap();
        assert_eq!(layers.len(), 3);

        // layer 0 has 1 brightness block, 4 palette entries
        let (body_start, offsets) = &layers[0];
        assert_eq!(*body_start, 2);
        assert_eq!(
            offsets,
            &LayerSpeedOffsets {
                // 6 fixed + 6 block + 1 for direction_distribution
                colour_speed: 13,
                // 6 + 6 + 4 + 12 palette + 2
                selected_movement_speed: 30,
                overall_movement_speed: 33,
                brightness_speeds: vec![9],
            }
        );

        // The current speed value of layer 0's selected movement
        assert_eq!(data[body_start + offsets.selected_movement_speed], 242);
    }

    #[test]
    fn test_parse_layers_forest() {
        let data = decode(FOREST_PARAM);
        let layers = parse_layers(&data).unwrap();
        assert_eq!(layers.len(), 3);

        // layer 0 has 1 brightness block, 4 palette entries
        let (body_start, offsets) = &layers[0];
        assert_eq!(offsets.brightness_speeds, vec![9],);
        assert_eq!(data[body_start + offsets.brightness_speeds[0]], 204);

        // layer 2: the same layout
        let (body_start, offsets) = &layers[2];
        assert_eq!(data[body_start + offsets.brightness_speeds[0]], 206);
    }

    #[test]
    fn test_speed_config_parsing() {
        let speed_info: JsonValue = serde_json::from_str(RIVER_SPEED_INFO).unwrap();
        let config = scene_speed_config(&speed_info).expect("have config");
        assert_eq!(config.option_count(), 3);
        assert_eq!(config.default_index(), 2);
        assert_eq!(config.pages.len(), 1);
        assert_eq!(config.pages[0].move_in, vec![242, 249, 252]);

        let forest_info: JsonValue = serde_json::from_str(FOREST_SPEED_INFO).unwrap();
        let config = scene_speed_config(&forest_info).expect("have config");
        assert_eq!(config.option_count(), 4);
        assert_eq!(config.pages.len(), 2);
        assert_eq!(config.pages[0].brightness[0].block, 0);
        assert_eq!(
            config.pages[0].brightness[0].values,
            vec![204, 204, 216, 234]
        );
    }

    #[test]
    fn test_speed_config_no_support() {
        let speed_info = json!({
            "config": "",
            "speedIndex": 0,
            "supSpeed": false,
        });
        assert_eq!(scene_speed_config(&speed_info), None);

        // supSpeed with an empty config is not usable either
        let speed_info = json!({
            "config": "[]",
            "speedIndex": 0,
            "supSpeed": true,
        });
        assert_eq!(scene_speed_config(&speed_info), None);

        // pages without speed option lists are not usable
        let speed_info = json!({
            "config": "[{\"page\":0,\"color\":[1,2,3]}]",
            "speedIndex": 0,
            "supSpeed": true,
        });
        assert_eq!(scene_speed_config(&speed_info), None);
    }

    #[test]
    fn test_apply_speed_changes_only_speed_fields() {
        let speed_info: JsonValue = serde_json::from_str(RIVER_SPEED_INFO).unwrap();
        let config = scene_speed_config(&speed_info).unwrap();

        let original = decode(RIVER_PARAM);
        let modified = decode(&apply_scene_speed(RIVER_PARAM, &config, 2).unwrap());

        assert_eq!(original.len(), modified.len());

        let layers = parse_layers(&original).unwrap();
        let mut changed = vec![];
        for (i, (a, b)) in original.iter().zip(modified.iter()).enumerate() {
            if a != b {
                changed.push(i);
            }
        }

        // Only the selected_movement.speed field of layer 0 changes
        let (body_start, offsets) = &layers[0];
        assert_eq!(changed, vec![body_start + offsets.selected_movement_speed]);
        assert_eq!(modified[body_start + offsets.selected_movement_speed], 252);
    }

    #[test]
    fn test_apply_speed_forest() {
        let speed_info: JsonValue = serde_json::from_str(FOREST_SPEED_INFO).unwrap();
        let config = scene_speed_config(&speed_info).unwrap();

        let original = decode(FOREST_PARAM);
        let modified = decode(&apply_scene_speed(FOREST_PARAM, &config, 3).unwrap());
        assert_eq!(original.len(), modified.len());

        let layers = parse_layers(&original).unwrap();

        // Only the change_speed field of the brightness blocks in
        // layers 0 and 2 changes
        let mut changed = vec![];
        for (i, (a, b)) in original.iter().zip(modified.iter()).enumerate() {
            if a != b {
                changed.push(i);
            }
        }
        let (body_start_0, offsets_0) = &layers[0];
        let (body_start_2, offsets_2) = &layers[2];
        let mut expected = vec![
            body_start_0 + offsets_0.brightness_speeds[0],
            body_start_2 + offsets_2.brightness_speeds[0],
        ];
        expected.sort_unstable();
        assert_eq!(changed, expected);
        assert_eq!(modified[*expected.first().unwrap()], 234);
        assert_eq!(modified[*expected.last().unwrap()], 234);
    }

    #[test]
    fn test_apply_speed_index_clamping() {
        let speed_info: JsonValue = serde_json::from_str(RIVER_SPEED_INFO).unwrap();
        let config = scene_speed_config(&speed_info).unwrap();

        // An out of range index is clamped to the last option
        let modified = decode(&apply_scene_speed(RIVER_PARAM, &config, 99).unwrap());
        let layers = parse_layers(&decode(RIVER_PARAM)).unwrap();
        let (body_start, offsets) = &layers[0];
        assert_eq!(modified[body_start + offsets.selected_movement_speed], 252);
    }

    #[test]
    fn test_apply_speed_bad_param() {
        let speed_info: JsonValue = serde_json::from_str(RIVER_SPEED_INFO).unwrap();
        let config = scene_speed_config(&speed_info).unwrap();

        assert!(apply_scene_speed("not base64", &config, 0).is_err());

        // A param with a truncated layer
        let truncated = data_encoding::BASE64.encode(&[2, 30, 0, 0, 0]);
        assert!(apply_scene_speed(&truncated, &config, 0).is_err());

        // A param with non-zero padding after the layers
        let mut data = decode(RIVER_PARAM);
        data.push(1);
        let bad_padding = data_encoding::BASE64.encode(&data);
        assert!(apply_scene_speed(&bad_padding, &config, 0).is_err());

        // A config that references a missing layer
        let bad_config = SceneSpeedConfig {
            pages: vec![SceneSpeedPage {
                page: 99,
                default_index: 0,
                move_in: vec![1, 2],
                move_all: vec![],
                colour_speed: vec![],
                brightness: vec![],
            }],
        };
        assert!(apply_scene_speed(RIVER_PARAM, &bad_config, 0).is_err());
    }

    #[test]
    fn test_value_for_percent_anchors() {
        // H619Z Moonlight brightness: the default is the fastest option
        let moonlight = [20, 86, 100];
        assert_eq!(value_for_percent(&moonlight, 2, 50), 100);
        assert_eq!(value_for_percent(&moonlight, 2, 0), 0);
        assert_eq!(value_for_percent(&moonlight, 2, 25), 50);
        assert_eq!(value_for_percent(&moonlight, 2, 100), 180);

        // H619Z Party brightness: options 1 and 2 are identical, so the
        // catalog has nothing slower than the default
        let party = [20, 20, 204];
        assert_eq!(value_for_percent(&party, 1, 0), 0);
        assert_eq!(value_for_percent(&party, 1, 50), 20);
        assert_eq!(value_for_percent(&party, 1, 100), 255);

        // A narrow span still extends by at least MIN_PERCENT_EXTENSION
        let warm = [250, 252, 255];
        assert_eq!(value_for_percent(&warm, 0, 0), 186);
        assert_eq!(value_for_percent(&warm, 0, 50), 250);
        assert_eq!(value_for_percent(&warm, 0, 100), 255);
    }

    #[test]
    fn test_value_for_percent_edge_cases() {
        // Non-monotonic catalog values use the min/max for the range
        let flower_field = [204, 201, 207];
        assert_eq!(value_for_percent(&flower_field, 1, 50), 201);
        assert_eq!(value_for_percent(&flower_field, 1, 0), 137);

        // Out of range inputs are clamped
        assert_eq!(value_for_percent(&[20, 86, 100], 2, 200), 180);
        assert_eq!(value_for_percent(&[20, 86, 100], 9, 50), 100);

        // Nothing to pick from
        assert_eq!(value_for_percent(&[], 0, 50), 0);

        // Every percentage is monotonic non-decreasing
        let mut last = 0;
        for percent in 0..=100 {
            let value = value_for_percent(&[183, 214, 214], 2, percent);
            assert!(value >= last, "{percent}% went backwards");
            last = value;
        }
    }

    #[test]
    fn test_apply_speed_percent_river() {
        let speed_info: JsonValue = serde_json::from_str(RIVER_SPEED_INFO).unwrap();
        let config = scene_speed_config(&speed_info).unwrap();
        let layers = parse_layers(&decode(RIVER_PARAM)).unwrap();
        let (body_start, offsets) = &layers[0];
        let speed_at = |param: String| decode(&param)[body_start + offsets.selected_movement_speed];

        // 50% is the same as the catalog's default option
        assert_eq!(
            apply_scene_speed_percent(RIVER_PARAM, &config, 50).unwrap(),
            apply_scene_speed(RIVER_PARAM, &config, config.default_index()).unwrap()
        );
        assert_eq!(
            speed_at(apply_scene_speed_percent(RIVER_PARAM, &config, 0).unwrap()),
            178
        );
        assert_eq!(
            speed_at(apply_scene_speed_percent(RIVER_PARAM, &config, 100).unwrap()),
            255
        );

        // SceneSpeed dispatches to the matching function
        assert_eq!(
            SceneSpeed::Percent(0).apply(RIVER_PARAM, &config).unwrap(),
            apply_scene_speed_percent(RIVER_PARAM, &config, 0).unwrap()
        );
        assert_eq!(
            SceneSpeed::Option(1).apply(RIVER_PARAM, &config).unwrap(),
            apply_scene_speed(RIVER_PARAM, &config, 1).unwrap()
        );
    }
}
