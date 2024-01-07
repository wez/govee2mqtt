#[derive(Debug)]
pub struct Quirk {
    pub sku: &'static str,
    pub supports_rgb: bool,
    pub supports_brightness: bool,
    pub color_temp_range: Option<(u32, u32)>,
    pub avoid_platform_api: bool,
}

static QUIRKS: &[Quirk] = &[
    Quirk {
        sku: "H610A",
        supports_rgb: true,
        supports_brightness: true,
        color_temp_range: Some((2000, 9000)),
        // At the time of writing, the metadata
        // returned by Govee is completely bogus for this
        // device
        // <https://github.com/wez/govee2mqtt/issues/7>
        avoid_platform_api: true,
    },
    Quirk {
        sku: "H6141",
        supports_rgb: true,
        supports_brightness: true,
        color_temp_range: Some((2000, 9000)),
        // At the time of writing, the metadata
        // returned by Govee is completely bogus for this
        // device
        // <https://github.com/wez/govee2mqtt/issues/15>
        avoid_platform_api: true,
    },
];

pub fn resolve_quirk(sku: &str) -> Option<&'static Quirk> {
    QUIRKS.iter().find(|q| q.sku == sku)
}
