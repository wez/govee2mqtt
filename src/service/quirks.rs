use once_cell::sync::Lazy;
use std::borrow::Cow;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Quirk {
    pub sku: Cow<'static, str>,
    pub icon: Cow<'static, str>,
    pub supports_rgb: bool,
    pub supports_brightness: bool,
    pub color_temp_range: Option<(u32, u32)>,
    pub avoid_platform_api: bool,
    pub ble_only: bool,
    pub lan_api_capable: bool,
}

impl Quirk {
    pub fn light<SKU: Into<Cow<'static, str>>>(sku: SKU, icon: &'static str) -> Self {
        Quirk {
            sku: sku.into(),
            supports_rgb: true,
            supports_brightness: true,
            color_temp_range: Some((2000, 9000)),
            avoid_platform_api: false,
            ble_only: false,
            icon: icon.into(),
            lan_api_capable: false,
        }
    }

    pub fn with_lan_api(mut self) -> Self {
        self.lan_api_capable = true;
        self
    }

    pub fn with_broken_platform(mut self) -> Self {
        self.avoid_platform_api = true;
        self
    }

    pub fn lan_api_capable_light(sku: &'static str, icon: &'static str) -> Self {
        Self::light(sku, icon).with_lan_api()
    }
}

static QUIRKS: Lazy<HashMap<String, Quirk>> = Lazy::new(load_quirks);

const STRIP: &str = "mdi:led-strip-variant";
const STRIP_ALT: &str = "mdi:led-strip";
const FLOOD: &str = "mdi:light-flood-down";
const STRING: &str = "mdi:string-lights";
pub const BULB: &str = "mdi:light-bulb";
const FLOOR_LAMP: &str = "mdi:floor-lamp";
const TV_BACK: &str = "mdi:television-ambient-light";
const DESK: &str = "mdi:desk-lamp";
const HEX: &str = "mdi:hexagon-multiple";
const TRIANGLE: &str = "mdi-triangle";
const NIGHTLIGHT: &str = "mdi:lightbulb-night";
const WALL_SCONCE: &str = "mdi:wall-sconce";
const OUTDOOR_LAMP: &str = "mdi:outdoor-lamp";
const SPOTLIGHT: &str = "mdi:lightbulb-spot";

fn load_quirks() -> HashMap<String, Quirk> {
    let mut map = HashMap::new();
    for quirk in [
        // At the time of writing, the metadata
        // returned by Govee is completely bogus for this
        // device
        // <https://github.com/wez/govee2mqtt/issues/7>
        Quirk::lan_api_capable_light("H610A", STRIP).with_broken_platform(),
        // At the time of writing, the metadata
        // returned by Govee is completely bogus for this
        // device
        // <https://github.com/wez/govee2mqtt/issues/15>
        Quirk::light("H6141", STRIP).with_broken_platform(),
        // At the time of writing, the metadata
        // returned by Govee is completely bogus for this
        // device
        // <https://github.com/wez/govee2mqtt/issues/14#issuecomment-1880050091>
        Quirk::light("H6159", STRIP).with_broken_platform(),
        // Lights from the list of LAN API enabled devices
        // at <https://app-h5.govee.com/user-manual/wlan-guide>
        Quirk::lan_api_capable_light("H6072", FLOOR_LAMP),
        Quirk::lan_api_capable_light("H619B", STRIP),
        Quirk::lan_api_capable_light("H619C", STRIP),
        Quirk::lan_api_capable_light("H619Z", STRIP),
        Quirk::lan_api_capable_light("H7060", FLOOD),
        Quirk::lan_api_capable_light("H6046", TV_BACK),
        Quirk::lan_api_capable_light("H6047", TV_BACK),
        Quirk::lan_api_capable_light("H6051", DESK),
        Quirk::lan_api_capable_light("H6056", STRIP_ALT),
        Quirk::lan_api_capable_light("H6059", NIGHTLIGHT),
        Quirk::lan_api_capable_light("H6061", HEX),
        Quirk::lan_api_capable_light("H6062", STRIP),
        Quirk::lan_api_capable_light("H6065", STRIP),
        Quirk::lan_api_capable_light("H6066", HEX),
        Quirk::lan_api_capable_light("H6067", TRIANGLE),
        Quirk::lan_api_capable_light("H6073", FLOOR_LAMP),
        Quirk::lan_api_capable_light("H6076", FLOOR_LAMP),
        Quirk::lan_api_capable_light("H6078", FLOOR_LAMP),
        Quirk::lan_api_capable_light("H6087", WALL_SCONCE),
        Quirk::lan_api_capable_light("H610A", STRIP),
        Quirk::lan_api_capable_light("H610B", STRIP),
        Quirk::lan_api_capable_light("H6117", STRIP),
        Quirk::lan_api_capable_light("H6159", STRIP),
        Quirk::lan_api_capable_light("H615E", STRIP),
        Quirk::lan_api_capable_light("H6163", STRIP),
        Quirk::lan_api_capable_light("H6168", TV_BACK),
        Quirk::lan_api_capable_light("H6172", STRIP),
        Quirk::lan_api_capable_light("H6173", STRIP),
        Quirk::lan_api_capable_light("H618A", STRIP),
        Quirk::lan_api_capable_light("H618C", STRIP),
        Quirk::lan_api_capable_light("H618E", STRIP),
        Quirk::lan_api_capable_light("H618F", STRIP),
        Quirk::lan_api_capable_light("H619A", STRIP),
        Quirk::lan_api_capable_light("H619D", STRIP),
        Quirk::lan_api_capable_light("H619E", STRIP),
        Quirk::lan_api_capable_light("H61A0", STRIP),
        Quirk::lan_api_capable_light("H61A1", STRIP),
        Quirk::lan_api_capable_light("H61A2", STRIP),
        Quirk::lan_api_capable_light("H61A3", STRIP),
        Quirk::lan_api_capable_light("H61A5", STRIP),
        Quirk::lan_api_capable_light("H61A8", STRIP),
        Quirk::lan_api_capable_light("H61B2", TV_BACK),
        Quirk::lan_api_capable_light("H61E1", STRIP),
        Quirk::lan_api_capable_light("H7012", STRING),
        Quirk::lan_api_capable_light("H7013", STRING),
        Quirk::lan_api_capable_light("H7021", STRING),
        Quirk::lan_api_capable_light("H7028", STRING),
        Quirk::lan_api_capable_light("H7041", STRING),
        Quirk::lan_api_capable_light("H7042", STRING),
        Quirk::lan_api_capable_light("H7050", BULB),
        Quirk::lan_api_capable_light("H7051", BULB),
        Quirk::lan_api_capable_light("H7055", BULB),
        Quirk::lan_api_capable_light("H705A", OUTDOOR_LAMP),
        Quirk::lan_api_capable_light("H705B", OUTDOOR_LAMP),
        Quirk::lan_api_capable_light("H7061", FLOOD),
        Quirk::lan_api_capable_light("H7062", FLOOD),
        Quirk::lan_api_capable_light("H7065", SPOTLIGHT),
    ] {
        map.insert(quirk.sku.to_string(), quirk);
    }

    map
}

pub fn resolve_quirk(sku: &str) -> Option<&'static Quirk> {
    QUIRKS.get(sku)
}
