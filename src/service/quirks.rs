use crate::platform_api::DeviceType;
use crate::temperature::TemperatureUnits;
use once_cell::sync::Lazy;
use std::borrow::Cow;
use std::collections::HashMap;

#[allow(unused)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HumidityUnits {
    RelativePercent,
    RelativePercentTimes100,
}

impl HumidityUnits {
    pub fn from_reading_to_relative_percent(&self, value: f64) -> f64 {
        match self {
            Self::RelativePercent => value,
            Self::RelativePercentTimes100 => value / 100.,
        }
    }
}

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
    pub device_type: DeviceType,
    pub platform_temperature_sensor_units: Option<TemperatureUnits>,
    pub platform_humidity_sensor_units: Option<HumidityUnits>,
    /// If true, we can correctly parse all appropriate
    /// packets from the MQTT subscription and apply
    /// their state.
    pub iot_api_supported: bool,
    pub show_as_preset_buttons: Option<&'static [&'static str]>,
}

impl Quirk {
    pub fn device<SKU: Into<Cow<'static, str>>>(
        sku: SKU,
        device_type: DeviceType,
        icon: &'static str,
    ) -> Self {
        Self {
            sku: sku.into(),
            supports_rgb: false,
            supports_brightness: false,
            color_temp_range: None,
            avoid_platform_api: false,
            ble_only: false,
            icon: icon.into(),
            lan_api_capable: false,
            device_type,
            platform_temperature_sensor_units: None,
            platform_humidity_sensor_units: None,
            iot_api_supported: false,
            show_as_preset_buttons: None,
        }
    }

    pub fn light<SKU: Into<Cow<'static, str>>>(sku: SKU, icon: &'static str) -> Self {
        Self::device(sku, DeviceType::Light, icon)
            .with_rgb()
            .with_brightness()
            .with_color_temp()
            .with_iot_api_support(true)
    }

    pub fn ice_maker<SKU: Into<Cow<'static, str>>>(sku: SKU) -> Self {
        Self::device(sku, DeviceType::IceMaker, "mdi:snowflake")
    }

    pub fn space_heater<SKU: Into<Cow<'static, str>>>(sku: SKU) -> Self {
        Self::device(sku, DeviceType::Heater, "mdi:heat-wave")
    }

    pub fn humidifier<SKU: Into<Cow<'static, str>>>(sku: SKU) -> Self {
        Self::device(sku, DeviceType::Humidifier, "mdi:air-humidifier")
    }

    pub fn thermometer<SKU: Into<Cow<'static, str>>>(sku: SKU) -> Self {
        Self::device(sku, DeviceType::Thermometer, "mdi:thermometer")
    }

    pub fn with_rgb(mut self) -> Self {
        self.supports_rgb = true;
        self
    }

    pub fn with_brightness(mut self) -> Self {
        self.supports_brightness = true;
        self
    }

    pub fn with_platform_temperature_sensor_units(mut self, units: TemperatureUnits) -> Self {
        self.platform_temperature_sensor_units = Some(units);
        self
    }

    pub fn with_platform_humidity_sensor_units(mut self, units: HumidityUnits) -> Self {
        self.platform_humidity_sensor_units = Some(units);
        self
    }

    pub fn with_iot_api_support(mut self, supported: bool) -> Self {
        self.iot_api_supported = supported;
        self
    }

    pub fn with_color_temp(mut self) -> Self {
        self.color_temp_range = Some((2000, 9000));
        self
    }

    pub fn with_color_temp_range(mut self, min: u32, max: u32) -> Self {
        self.color_temp_range = Some((min, max));
        self
    }

    pub fn with_lan_api(mut self) -> Self {
        self.lan_api_capable = true;
        self
    }

    pub fn with_show_as_preset_modes(mut self, modes: &'static [&'static str]) -> Self {
        self.show_as_preset_buttons.replace(modes);
        self
    }

    pub fn with_broken_platform(mut self) -> Self {
        self.avoid_platform_api = true;
        self
    }

    pub fn with_ble_only(mut self, ble_only: bool) -> Self {
        self.ble_only = ble_only;
        self
    }

    pub fn lan_api_capable_light(sku: &'static str, icon: &'static str) -> Self {
        Self::light(sku, icon).with_lan_api()
    }

    pub fn should_show_mode_as_preset(&self, mode: &str) -> bool {
        self.show_as_preset_buttons
            .as_ref()
            .map(|modes| modes.contains(&mode))
            .unwrap_or(false)
    }
}

static QUIRKS: Lazy<HashMap<String, Quirk>> = Lazy::new(load_quirks);

const STRIP: &str = "mdi:led-strip-variant";
const STRIP_ALT: &str = "mdi:led-strip";
const FLOOD: &str = "mdi:light-flood-down";
const STRING: &str = "mdi:string-lights";
pub const BULB: &str = "mdi:lightbulb";
const FLOOR_LAMP: &str = "mdi:floor-lamp";
const TV_BACK: &str = "mdi:television-ambient-light";
const DESK: &str = "mdi:desk-lamp";
const HEX: &str = "mdi:hexagon-multiple";
const TRIANGLE: &str = "mdi:triangle";
const CEILING: &str = "mdi:ceiling-light";
const NIGHTLIGHT: &str = "mdi:lightbulb-night";
const WALL_SCONCE: &str = "mdi:wall-sconce";
const OUTDOOR_LAMP: &str = "mdi:outdoor-lamp";
const SPOTLIGHT: &str = "mdi:lightbulb-spot";

fn load_quirks() -> HashMap<String, Quirk> {
    let mut map = HashMap::new();
    for quirk in [
        // H60A1 Govee Ceiling Light has a color temperature range of 2200K - 6500K
        // Without this quirk, the LAN API fallback reports (2000, 9000) which causes issues
        // <https://github.com/wez/govee2mqtt/pull/502>
        Quirk::lan_api_capable_light("H60A1", CEILING).with_color_temp_range(2200, 6500),
        // Color temperature is more restrictive than the fallback range
        // <https://github.com/wez/govee2mqtt/issues/511>
        Quirk::lan_api_capable_light("H6022", BULB).with_color_temp_range(2700, 6500),
        Quirk::lan_api_capable_light("H610A", STRIP),
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
        // <https://github.com/wez/govee2mqtt/issues/152>
        Quirk::light("H6003", BULB).with_broken_platform(),
        // <https://github.com/wez/govee2mqtt/issues/40#issuecomment-1889726710>
        // indicates that this one doesn't work like the others with IoT
        Quirk::light("H6121", STRIP).with_iot_api_support(false),
        // <https://github.com/wez/govee2mqtt/issues/40>
        Quirk::light("H6154", STRIP).with_iot_api_support(false),
        // <https://github.com/wez/govee2mqtt/issues/49>
        Quirk::light("H6176", STRIP).with_iot_api_support(false),
        // Platform API probably shouldn't return this device (I suppose,
        // aside from letting us find out its name), and we need to know
        // that it is definitely BLE-only
        // <https://github.com/wez/govee2mqtt/issues/92>
        Quirk::light("H6102", STRIP)
            .with_broken_platform()
            .with_ble_only(true),
        // Another BLE-only device <https://github.com/wez/govee2mqtt/issues/77>
        Quirk::light("H6053", STRIP)
            .with_broken_platform()
            .with_ble_only(true),
        Quirk::light("H617C", STRIP)
            .with_broken_platform()
            .with_ble_only(true),
        Quirk::light("H617E", STRIP)
            .with_broken_platform()
            .with_ble_only(true),
        Quirk::light("H617F", STRIP)
            .with_broken_platform()
            .with_ble_only(true),
        Quirk::light("H6119", STRIP)
            .with_broken_platform()
            .with_ble_only(true),
        // Humidifer with mangled platform API data
        Quirk::humidifier("H7160")
            .with_broken_platform()
            .with_iot_api_support(true)
            .with_rgb()
            .with_brightness(),
        Quirk::space_heater("H7130")
            .with_platform_temperature_sensor_units(TemperatureUnits::Fahrenheit),
        Quirk::space_heater("H7131")
            .with_platform_temperature_sensor_units(TemperatureUnits::Fahrenheit)
            .with_show_as_preset_modes(&["gearMode"])
            .with_rgb()
            .with_brightness(),
        Quirk::space_heater("H713A")
            .with_platform_temperature_sensor_units(TemperatureUnits::Fahrenheit),
        Quirk::space_heater("H713B")
            .with_platform_temperature_sensor_units(TemperatureUnits::Fahrenheit),
        Quirk::space_heater("H7132")
            .with_platform_temperature_sensor_units(TemperatureUnits::Fahrenheit),
        Quirk::space_heater("H7133")
            .with_platform_temperature_sensor_units(TemperatureUnits::Fahrenheit)
            .with_show_as_preset_modes(&["gearMode"])
            .with_rgb()
            .with_brightness(),
        Quirk::space_heater("H7134")
            .with_platform_temperature_sensor_units(TemperatureUnits::Fahrenheit)
            .with_show_as_preset_modes(&["gearMode"])
            .with_color_temp()
            .with_brightness(),
        Quirk::space_heater("H7135")
            .with_platform_temperature_sensor_units(TemperatureUnits::Fahrenheit),
        // <https://github.com/wez/govee2mqtt/issues/343>
        Quirk::ice_maker("H7172").with_iot_api_support(false),
        Quirk::thermometer("H5051")
            .with_platform_temperature_sensor_units(TemperatureUnits::Fahrenheit)
            .with_platform_humidity_sensor_units(HumidityUnits::RelativePercent),
        Quirk::thermometer("H5100")
            .with_platform_temperature_sensor_units(TemperatureUnits::Fahrenheit)
            .with_platform_humidity_sensor_units(HumidityUnits::RelativePercent),
        Quirk::thermometer("H5103")
            .with_platform_temperature_sensor_units(TemperatureUnits::Fahrenheit)
            .with_platform_humidity_sensor_units(HumidityUnits::RelativePercent),
        Quirk::thermometer("H5179")
            .with_platform_temperature_sensor_units(TemperatureUnits::Fahrenheit)
            .with_platform_humidity_sensor_units(HumidityUnits::RelativePercent),
        Quirk::device("H7170", DeviceType::Kettle, "mdi:kettle")
            .with_platform_temperature_sensor_units(TemperatureUnits::Fahrenheit),
        Quirk::device("H7171", DeviceType::Kettle, "mdi:kettle")
            .with_platform_temperature_sensor_units(TemperatureUnits::Fahrenheit)
            .with_show_as_preset_modes(&["M1", "M2", "M3", "M4"]),
        Quirk::device("H7173", DeviceType::Kettle, "mdi:kettle")
            .with_platform_temperature_sensor_units(TemperatureUnits::Fahrenheit)
            .with_show_as_preset_modes(&["Tea", "Coffee", "DIY"]),
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
        Quirk::lan_api_capable_light("H66A1", TV_BACK),
        Quirk::lan_api_capable_light("H7012", STRING),
        Quirk::lan_api_capable_light("H7013", STRING),
        Quirk::lan_api_capable_light("H7021", STRING),
        Quirk::lan_api_capable_light("H7028", STRING),
        Quirk::lan_api_capable_light("H7041", STRING),
        Quirk::lan_api_capable_light("H7042", STRING),
        Quirk::lan_api_capable_light("H7050", BULB),
        Quirk::lan_api_capable_light("H7051", BULB),
        Quirk::lan_api_capable_light("H7052", STRING),
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
