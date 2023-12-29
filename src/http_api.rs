use anyhow::Context;
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlite_cache::{Cache, CacheConfig};
use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

// <https://developer.govee.com/reference/get-you-devices>
const SERVER: &str = "https://openapi.api.govee.com";

fn endpoint(url: &str) -> String {
    format!("{SERVER}{url}")
}

pub struct GoveeApiClient {
    key: String,
    cache: Cache,
}

async fn cache_get<T, Fut>(
    cache: &Cache,
    key: &str,
    ttl: Duration,
    future: Fut,
) -> anyhow::Result<T>
where
    T: Serialize + DeserializeOwned + std::fmt::Debug,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let topic = cache.topic("http-api")?;
    let (updater, current_value) = topic.get_for_update(key).await?;
    if let Some(current) = current_value {
        let result: T = serde_json::from_slice(&current.data)?;
        return Ok(result);
    }

    let value: T = future.await?;
    let data = serde_json::to_string_pretty(&value)?;
    updater.write(data.as_bytes(), ttl)?;

    Ok(value)
}

impl GoveeApiClient {
    pub fn new<K: Into<String>>(key: K) -> anyhow::Result<Self> {
        let cache_dir = std::env::var("GOVEE_CACHE_DIR")
            .ok()
            .map(PathBuf::from)
            .or_else(|| dirs_next::cache_dir())
            .ok_or_else(|| anyhow::anyhow!("failed to resolve cache dir"))?;

        let cache_file = cache_dir.join("govee-rs-cache.sqlite");
        let cache = Cache::new(
            CacheConfig::default(),
            sqlite_cache::rusqlite::Connection::open(cache_file)?,
        )?;

        Ok(Self {
            key: key.into(),
            cache,
        })
    }

    pub async fn get_devices(&self) -> anyhow::Result<Vec<HttpDeviceInfo>> {
        cache_get(
            &self.cache,
            "device-list",
            Duration::from_secs(900),
            async {
                let url = endpoint("/router/api/v1/user/devices");
                let resp: GetDevicesResponse = self.get_request_with_json_response(url).await?;
                Ok(resp.data)
            },
        )
        .await
    }

    pub async fn get_device_state(
        &self,
        device: &HttpDeviceInfo,
    ) -> anyhow::Result<HttpDeviceState> {
        let url = endpoint("/router/api/v1/device/state");
        let request = GetDeviceStateRequest {
            request_id: "uuid".to_string(),
            payload: GetDeviceStateRequestPayload {
                sku: device.sku.to_string(),
                device: device.device.to_string(),
            },
        };

        let resp: GetDeviceStateResponse = self
            .request_with_json_response(Method::POST, url, &request)
            .await?;

        Ok(resp.payload)
    }
}

#[derive(Serialize, Debug)]
struct GetDeviceStateRequest {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub payload: GetDeviceStateRequestPayload,
}

#[derive(Serialize, Debug)]
struct GetDeviceStateRequestPayload {
    pub sku: String,
    pub device: String,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
struct GetDeviceStateResponse {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub code: u32,
    #[serde(rename = "msg")]
    pub message: String,
    pub payload: HttpDeviceState,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct HttpDeviceState {
    pub sku: String,
    pub device: String,
    pub capabilities: Vec<DeviceCapabilityState>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(tag = "type")]
#[serde(deny_unknown_fields)]
pub struct DeviceCapabilityState {
    #[serde(rename = "type")]
    pub kind: DeviceCapabilityKind,
    pub instance: String,
    pub state: JsonValue,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
struct GetDevicesResponse {
    pub code: u32,
    pub message: String,
    pub data: Vec<HttpDeviceInfo>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct HttpDeviceInfo {
    pub sku: String,
    pub device: String,
    #[serde(default, rename = "deviceName")]
    pub device_name: String,
    #[serde(default, rename = "type")]
    pub device_type: DeviceType,
    pub capabilities: Vec<DeviceCapability>,
}

#[derive(Deserialize, Serialize, Debug, Default)]
pub enum DeviceType {
    #[serde(rename = "devices.types.light")]
    #[default]
    Light,
    #[serde(rename = "devices.types.air_purifier")]
    AirPurifier,
    #[serde(rename = "devices.types.thermometer")]
    Thermometer,
    #[serde(rename = "devices.types.socket")]
    Socket,
    #[serde(rename = "devices.types.sensor")]
    Sensor,
    #[serde(rename = "devices.types.heater")]
    Heater,
    #[serde(rename = "devices.types.humidifier")]
    Humidifer,
    #[serde(rename = "devices.types.dehumidifer")]
    Dehumidifer,
    #[serde(rename = "devices.types.ice_maker")]
    IceMaker,
    #[serde(rename = "devices.types.aroma_diffuser")]
    AromaDiffuser,
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Serialize, Debug)]
pub enum DeviceCapabilityKind {
    #[serde(rename = "devices.capabilities.on_off")]
    OnOff,
    #[serde(rename = "devices.capabilities.toggle")]
    Toggle,
    #[serde(rename = "devices.capabilities.range")]
    Range,
    #[serde(rename = "devices.capabilities.mode")]
    Mode,
    #[serde(rename = "devices.capabilities.color_setting")]
    ColorSetting,
    #[serde(rename = "devices.capabilities.segment_color_setting")]
    SegmentColorSetting,
    #[serde(rename = "devices.capabilities.music_setting")]
    MusicSetting,
    #[serde(rename = "devices.capabilities.dynamic_scene")]
    DynamicScene,
    #[serde(rename = "devices.capabilities.work_mode")]
    WorkMode,
    #[serde(rename = "devices.capabilities.dynamic_setting")]
    DynamicSetting,
    #[serde(rename = "devices.capabilities.temperature_setting")]
    TemperatureSetting,
    #[serde(rename = "devices.capabilities.online")]
    Online,
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct DeviceCapability {
    #[serde(rename = "type")]
    pub kind: DeviceCapabilityKind,
    pub instance: String,
    pub parameters: DeviceParameters,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(tag = "dataType")]
#[serde(deny_unknown_fields)]
pub enum DeviceParameters {
    #[serde(rename = "ENUM")]
    Enum { options: Vec<EnumOption> },
    #[serde(rename = "INTEGER")]
    Integer {
        unit: Option<String>,
        range: IntegerRange,
    },
    #[serde(rename = "STRUCT")]
    Struct { fields: Vec<StructField> },
    #[serde(rename = "Array")]
    Array {
        size: Option<ArraySize>,
        #[serde(rename = "elementRange")]
        element_range: Option<ElementRange>,
        #[serde(rename = "elementType")]
        element_type: Option<String>,
        #[serde(default)]
        options: Vec<ArrayOption>,
    },
}

#[derive(Deserialize, Serialize, Debug)]
// No deny_unknown_fields here, because we embed via flatten
pub struct StructField {
    #[serde(rename = "fieldName")]
    pub field_name: String,

    #[serde(flatten)]
    pub field_type: DeviceParameters,

    pub required: bool,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ElementRange {
    pub min: u32,
    pub max: u32,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ArraySize {
    pub min: u32,
    pub max: u32,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct IntegerRange {
    pub min: u32,
    pub max: u32,
    pub precision: u32,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct EnumOption {
    pub name: String,
    pub value: u32,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ArrayOption {
    pub value: u32,
}

async fn json_body<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> anyhow::Result<T> {
    let data = response.bytes().await.context("ready response body")?;
    serde_json::from_slice(&data).with_context(|| {
        format!(
            "parsing response as json: {}",
            String::from_utf8_lossy(&data)
        )
    })
}

impl GoveeApiClient {
    async fn get_request_with_json_response<T: reqwest::IntoUrl, R: serde::de::DeserializeOwned>(
        &self,
        url: T,
    ) -> anyhow::Result<R> {
        let response = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()?
            .request(Method::GET, url)
            .header("Govee-API-Key", &self.key)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let url = response.url().clone();
            let body_bytes = response.bytes().await.with_context(|| {
                format!(
                    "request status {}: {}, and failed to read response body",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("")
                )
            })?;

            anyhow::bail!(
                "request status {}: {}. Response body: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or(""),
                String::from_utf8_lossy(&body_bytes)
            );
        }
        json_body(response).await.with_context(|| {
            format!(
                "request status {}: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            )
        })
    }

    async fn request_with_json_response<
        T: reqwest::IntoUrl,
        B: serde::Serialize,
        R: serde::de::DeserializeOwned,
    >(
        &self,
        method: Method,
        url: T,
        body: &B,
    ) -> anyhow::Result<R> {
        let response = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()?
            .request(method, url)
            .header("Govee-API-Key", &self.key)
            .json(body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body_bytes = response.bytes().await.with_context(|| {
                format!(
                    "request status {}: {}, and failed to read response body",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("")
                )
            })?;
            anyhow::bail!(
                "request status {}: {}. Response body: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or(""),
                String::from_utf8_lossy(&body_bytes)
            );
        }
        json_body(response).await.with_context(|| {
            format!(
                "request status {}: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            )
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;

    const GET_DEVICE_STATE_EXAMPLE: &str = include_str!("../test-data/get_device_state.json");

    #[test]
    fn get_device_state() {
        let resp: GetDeviceStateResponse = serde_json::from_str(&GET_DEVICE_STATE_EXAMPLE).unwrap();
        k9::snapshot!(
            resp,
            r#"
GetDeviceStateResponse {
    request_id: "uuid",
    code: 200,
    message: "success",
    payload: HttpDeviceState {
        sku: "H7143",
        device: "52:8B:D4:AD:FC:45:5D:FE",
        capabilities: [
            DeviceCapabilityState {
                kind: Online,
                instance: "online",
                state: Object {
                    "value": Bool(false),
                },
            },
            DeviceCapabilityState {
                kind: OnOff,
                instance: "powerSwitch",
                state: Object {
                    "value": Number(0),
                },
            },
            DeviceCapabilityState {
                kind: Toggle,
                instance: "warmMistToggle",
                state: Object {
                    "value": Number(0),
                },
            },
            DeviceCapabilityState {
                kind: WorkMode,
                instance: "workMode",
                state: Object {
                    "value": Object {
                        "modeValue": Number(9),
                        "workMode": Number(3),
                    },
                },
            },
            DeviceCapabilityState {
                kind: Range,
                instance: "humidity",
                state: Object {
                    "value": String(""),
                },
            },
            DeviceCapabilityState {
                kind: Toggle,
                instance: "nightlightToggle",
                state: Object {
                    "value": Number(1),
                },
            },
            DeviceCapabilityState {
                kind: Range,
                instance: "brightness",
                state: Object {
                    "value": Number(5),
                },
            },
            DeviceCapabilityState {
                kind: ColorSetting,
                instance: "colorRgb",
                state: Object {
                    "value": Number(16777215),
                },
            },
            DeviceCapabilityState {
                kind: Mode,
                instance: "nightlightScene",
                state: Object {
                    "value": Number(5),
                },
            },
        ],
    },
}
"#
        );
    }

    const LIST_DEVICES_EXAMPLE: &str = include_str!("../test-data/list_devices.json");
    const LIST_DEVICES_EXAMPLE2: &str = include_str!("../test-data/list_devices_2.json");

    #[test]
    fn list_devices_2() {
        let resp: GetDevicesResponse = serde_json::from_str(&LIST_DEVICES_EXAMPLE2).unwrap();
        k9::snapshot!(
            resp,
            r#"
GetDevicesResponse {
    code: 200,
    message: "success",
    data: [
        HttpDeviceInfo {
            sku: "H6072",
            device: "AA:BB:CC:DD:AA:BB:CC:DD",
            device_name: "Floor Lamp",
            device_type: Light,
            capabilities: [
                DeviceCapability {
                    kind: OnOff,
                    instance: "powerSwitch",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Toggle,
                    instance: "gradientToggle",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Range,
                    instance: "brightness",
                    parameters: Integer {
                        unit: Some(
                            "unit.percent",
                        ),
                        range: IntegerRange {
                            min: 1,
                            max: 100,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedBrightness",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: Some(
                                        ArraySize {
                                            min: 1,
                                            max: 8,
                                        },
                                    ),
                                    element_range: Some(
                                        ElementRange {
                                            min: 0,
                                            max: 14,
                                        },
                                    ),
                                    element_type: Some(
                                        "INTEGER",
                                    ),
                                    options: [],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "brightness",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedColorRgb",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: Some(
                                        ArraySize {
                                            min: 1,
                                            max: 8,
                                        },
                                    ),
                                    element_range: Some(
                                        ElementRange {
                                            min: 0,
                                            max: 14,
                                        },
                                    ),
                                    element_type: Some(
                                        "INTEGER",
                                    ),
                                    options: [],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorRgb",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 0,
                            max: 16777215,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorTemperatureK",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 2000,
                            max: 9000,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "lightScene",
                    parameters: Enum {
                        options: [],
                    },
                },
                DeviceCapability {
                    kind: MusicSetting,
                    instance: "musicMode",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "musicMode",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "Energic",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: 2,
                                        },
                                        EnumOption {
                                            name: "Bounce",
                                            value: 3,
                                        },
                                        EnumOption {
                                            name: "Hopping",
                                            value: 4,
                                        },
                                        EnumOption {
                                            name: "Strike",
                                            value: 5,
                                        },
                                        EnumOption {
                                            name: "Vibrate",
                                            value: 6,
                                        },
                                    ],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "sensitivity",
                                field_type: Integer {
                                    unit: Some(
                                        "unit.percent",
                                    ),
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "autoColor",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "on",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: 0,
                                        },
                                    ],
                                },
                                required: false,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: false,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "diyScene",
                    parameters: Enum {
                        options: [],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "snapshot",
                    parameters: Enum {
                        options: [],
                    },
                },
            ],
        },
        HttpDeviceInfo {
            sku: "H6072",
            device: "AA:BB:CC:DD:AA:BB:CC:DD",
            device_name: "Floor Lamp",
            device_type: Light,
            capabilities: [
                DeviceCapability {
                    kind: OnOff,
                    instance: "powerSwitch",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Toggle,
                    instance: "gradientToggle",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Range,
                    instance: "brightness",
                    parameters: Integer {
                        unit: Some(
                            "unit.percent",
                        ),
                        range: IntegerRange {
                            min: 1,
                            max: 100,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedBrightness",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: Some(
                                        ArraySize {
                                            min: 1,
                                            max: 8,
                                        },
                                    ),
                                    element_range: Some(
                                        ElementRange {
                                            min: 0,
                                            max: 14,
                                        },
                                    ),
                                    element_type: Some(
                                        "INTEGER",
                                    ),
                                    options: [],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "brightness",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedColorRgb",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: Some(
                                        ArraySize {
                                            min: 1,
                                            max: 8,
                                        },
                                    ),
                                    element_range: Some(
                                        ElementRange {
                                            min: 0,
                                            max: 14,
                                        },
                                    ),
                                    element_type: Some(
                                        "INTEGER",
                                    ),
                                    options: [],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorRgb",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 0,
                            max: 16777215,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorTemperatureK",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 2000,
                            max: 9000,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "lightScene",
                    parameters: Enum {
                        options: [],
                    },
                },
                DeviceCapability {
                    kind: MusicSetting,
                    instance: "musicMode",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "musicMode",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "Energic",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: 2,
                                        },
                                        EnumOption {
                                            name: "Bounce",
                                            value: 3,
                                        },
                                        EnumOption {
                                            name: "Hopping",
                                            value: 4,
                                        },
                                        EnumOption {
                                            name: "Strike",
                                            value: 5,
                                        },
                                        EnumOption {
                                            name: "Vibrate",
                                            value: 6,
                                        },
                                    ],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "sensitivity",
                                field_type: Integer {
                                    unit: Some(
                                        "unit.percent",
                                    ),
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "autoColor",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "on",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: 0,
                                        },
                                    ],
                                },
                                required: false,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: false,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "diyScene",
                    parameters: Enum {
                        options: [],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "snapshot",
                    parameters: Enum {
                        options: [],
                    },
                },
            ],
        },
        HttpDeviceInfo {
            sku: "H619A",
            device: "AA:BB:CC:DD:AA:BB:CC:DD",
            device_name: "H619A_CDF5",
            device_type: Light,
            capabilities: [
                DeviceCapability {
                    kind: OnOff,
                    instance: "powerSwitch",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Toggle,
                    instance: "gradientToggle",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Range,
                    instance: "brightness",
                    parameters: Integer {
                        unit: Some(
                            "unit.percent",
                        ),
                        range: IntegerRange {
                            min: 1,
                            max: 100,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedBrightness",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: Some(
                                        ArraySize {
                                            min: 1,
                                            max: 15,
                                        },
                                    ),
                                    element_range: Some(
                                        ElementRange {
                                            min: 0,
                                            max: 14,
                                        },
                                    ),
                                    element_type: Some(
                                        "INTEGER",
                                    ),
                                    options: [],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "brightness",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedColorRgb",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: Some(
                                        ArraySize {
                                            min: 1,
                                            max: 15,
                                        },
                                    ),
                                    element_range: Some(
                                        ElementRange {
                                            min: 0,
                                            max: 14,
                                        },
                                    ),
                                    element_type: Some(
                                        "INTEGER",
                                    ),
                                    options: [],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorRgb",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 0,
                            max: 16777215,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorTemperatureK",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 2000,
                            max: 9000,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "lightScene",
                    parameters: Enum {
                        options: [],
                    },
                },
                DeviceCapability {
                    kind: MusicSetting,
                    instance: "musicMode",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "musicMode",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "Energic",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: 2,
                                        },
                                        EnumOption {
                                            name: "Spectrum",
                                            value: 3,
                                        },
                                        EnumOption {
                                            name: "Rolling",
                                            value: 4,
                                        },
                                        EnumOption {
                                            name: "Separation",
                                            value: 5,
                                        },
                                        EnumOption {
                                            name: "Hopping",
                                            value: 6,
                                        },
                                        EnumOption {
                                            name: "PianoKeys",
                                            value: 7,
                                        },
                                        EnumOption {
                                            name: "Fountain",
                                            value: 8,
                                        },
                                        EnumOption {
                                            name: "DayAndNight",
                                            value: 9,
                                        },
                                        EnumOption {
                                            name: "Sprouting",
                                            value: 10,
                                        },
                                        EnumOption {
                                            name: "Shiny",
                                            value: 11,
                                        },
                                    ],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "sensitivity",
                                field_type: Integer {
                                    unit: Some(
                                        "unit.percent",
                                    ),
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "autoColor",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "on",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: 0,
                                        },
                                    ],
                                },
                                required: false,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: false,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "diyScene",
                    parameters: Enum {
                        options: [],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "snapshot",
                    parameters: Enum {
                        options: [],
                    },
                },
            ],
        },
        HttpDeviceInfo {
            sku: "H619A",
            device: "AA:BB:CC:DD:AA:BB:CC:DD",
            device_name: "Strip",
            device_type: Light,
            capabilities: [
                DeviceCapability {
                    kind: OnOff,
                    instance: "powerSwitch",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Toggle,
                    instance: "gradientToggle",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Range,
                    instance: "brightness",
                    parameters: Integer {
                        unit: Some(
                            "unit.percent",
                        ),
                        range: IntegerRange {
                            min: 1,
                            max: 100,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedBrightness",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: Some(
                                        ArraySize {
                                            min: 1,
                                            max: 15,
                                        },
                                    ),
                                    element_range: Some(
                                        ElementRange {
                                            min: 0,
                                            max: 14,
                                        },
                                    ),
                                    element_type: Some(
                                        "INTEGER",
                                    ),
                                    options: [],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "brightness",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedColorRgb",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: Some(
                                        ArraySize {
                                            min: 1,
                                            max: 15,
                                        },
                                    ),
                                    element_range: Some(
                                        ElementRange {
                                            min: 0,
                                            max: 14,
                                        },
                                    ),
                                    element_type: Some(
                                        "INTEGER",
                                    ),
                                    options: [],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorRgb",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 0,
                            max: 16777215,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorTemperatureK",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 2000,
                            max: 9000,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "lightScene",
                    parameters: Enum {
                        options: [],
                    },
                },
                DeviceCapability {
                    kind: MusicSetting,
                    instance: "musicMode",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "musicMode",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "Energic",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: 2,
                                        },
                                        EnumOption {
                                            name: "Spectrum",
                                            value: 3,
                                        },
                                        EnumOption {
                                            name: "Rolling",
                                            value: 4,
                                        },
                                        EnumOption {
                                            name: "Separation",
                                            value: 5,
                                        },
                                        EnumOption {
                                            name: "Hopping",
                                            value: 6,
                                        },
                                        EnumOption {
                                            name: "PianoKeys",
                                            value: 7,
                                        },
                                        EnumOption {
                                            name: "Fountain",
                                            value: 8,
                                        },
                                        EnumOption {
                                            name: "DayAndNight",
                                            value: 9,
                                        },
                                        EnumOption {
                                            name: "Sprouting",
                                            value: 10,
                                        },
                                        EnumOption {
                                            name: "Shiny",
                                            value: 11,
                                        },
                                    ],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "sensitivity",
                                field_type: Integer {
                                    unit: Some(
                                        "unit.percent",
                                    ),
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "autoColor",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "on",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: 0,
                                        },
                                    ],
                                },
                                required: false,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: false,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "diyScene",
                    parameters: Enum {
                        options: [],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "snapshot",
                    parameters: Enum {
                        options: [],
                    },
                },
            ],
        },
        HttpDeviceInfo {
            sku: "H61A2",
            device: "AA:BB:CC:DD:AA:BB:CC:DD",
            device_name: "Neon",
            device_type: Light,
            capabilities: [
                DeviceCapability {
                    kind: OnOff,
                    instance: "powerSwitch",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Toggle,
                    instance: "gradientToggle",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Range,
                    instance: "brightness",
                    parameters: Integer {
                        unit: Some(
                            "unit.percent",
                        ),
                        range: IntegerRange {
                            min: 1,
                            max: 100,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedBrightness",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: Some(
                                        ArraySize {
                                            min: 1,
                                            max: 15,
                                        },
                                    ),
                                    element_range: Some(
                                        ElementRange {
                                            min: 0,
                                            max: 14,
                                        },
                                    ),
                                    element_type: Some(
                                        "INTEGER",
                                    ),
                                    options: [],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "brightness",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedColorRgb",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: Some(
                                        ArraySize {
                                            min: 1,
                                            max: 15,
                                        },
                                    ),
                                    element_range: Some(
                                        ElementRange {
                                            min: 0,
                                            max: 14,
                                        },
                                    ),
                                    element_type: None,
                                    options: [],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorRgb",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 0,
                            max: 16777215,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorTemperatureK",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 2000,
                            max: 9000,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "lightScene",
                    parameters: Enum {
                        options: [],
                    },
                },
                DeviceCapability {
                    kind: MusicSetting,
                    instance: "musicMode",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "musicMode",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "Energic",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: 2,
                                        },
                                        EnumOption {
                                            name: "Spectrum",
                                            value: 3,
                                        },
                                        EnumOption {
                                            name: "Rolling",
                                            value: 4,
                                        },
                                        EnumOption {
                                            name: "Separation",
                                            value: 5,
                                        },
                                        EnumOption {
                                            name: "Hopping",
                                            value: 6,
                                        },
                                        EnumOption {
                                            name: "PianoKeys",
                                            value: 7,
                                        },
                                        EnumOption {
                                            name: "Fountain",
                                            value: 8,
                                        },
                                        EnumOption {
                                            name: "DayandNight",
                                            value: 9,
                                        },
                                        EnumOption {
                                            name: "Sprouting",
                                            value: 10,
                                        },
                                        EnumOption {
                                            name: "Shiny",
                                            value: 11,
                                        },
                                    ],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "sensitivity",
                                field_type: Integer {
                                    unit: Some(
                                        "unit.percent",
                                    ),
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "autoColor",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "on",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: 0,
                                        },
                                    ],
                                },
                                required: false,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: false,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "diyScene",
                    parameters: Enum {
                        options: [],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "snapshot",
                    parameters: Enum {
                        options: [],
                    },
                },
            ],
        },
        HttpDeviceInfo {
            sku: "H610A",
            device: "AA:BB:CC:DD:AA:BB:CC:DD",
            device_name: "Govee Glide Lively 1",
            device_type: Light,
            capabilities: [
                DeviceCapability {
                    kind: OnOff,
                    instance: "powerSwitch",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
            ],
        },
        HttpDeviceInfo {
            sku: "H6058",
            device: "AA:BB:CC:DD:AA:BB:CC:DD",
            device_name: "Portable Table Lamp",
            device_type: Light,
            capabilities: [
                DeviceCapability {
                    kind: OnOff,
                    instance: "powerSwitch",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Range,
                    instance: "brightness",
                    parameters: Integer {
                        unit: Some(
                            "unit.percent",
                        ),
                        range: IntegerRange {
                            min: 1,
                            max: 100,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedBrightness",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: Some(
                                        ArraySize {
                                            min: 1,
                                            max: 15,
                                        },
                                    ),
                                    element_range: Some(
                                        ElementRange {
                                            min: 0,
                                            max: 14,
                                        },
                                    ),
                                    element_type: Some(
                                        "INTEGER",
                                    ),
                                    options: [],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "brightness",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorRgb",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 0,
                            max: 16777215,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorTemperatureK",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 2000,
                            max: 9000,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "lightScene",
                    parameters: Enum {
                        options: [],
                    },
                },
                DeviceCapability {
                    kind: MusicSetting,
                    instance: "musicMode",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "musicMode",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "Dynamic",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "Calm",
                                            value: 2,
                                        },
                                    ],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "sensitivity",
                                field_type: Integer {
                                    unit: Some(
                                        "unit.percent",
                                    ),
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "autoColor",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "on",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: 0,
                                        },
                                    ],
                                },
                                required: false,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: false,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "diyScene",
                    parameters: Enum {
                        options: [],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "snapshot",
                    parameters: Enum {
                        options: [],
                    },
                },
            ],
        },
        HttpDeviceInfo {
            sku: "H6072",
            device: "AA:BB:CC:DD:AA:BB:CC:DD",
            device_name: "Floor Lamp",
            device_type: Light,
            capabilities: [
                DeviceCapability {
                    kind: OnOff,
                    instance: "powerSwitch",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Toggle,
                    instance: "gradientToggle",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Range,
                    instance: "brightness",
                    parameters: Integer {
                        unit: Some(
                            "unit.percent",
                        ),
                        range: IntegerRange {
                            min: 1,
                            max: 100,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedBrightness",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: Some(
                                        ArraySize {
                                            min: 1,
                                            max: 8,
                                        },
                                    ),
                                    element_range: Some(
                                        ElementRange {
                                            min: 0,
                                            max: 14,
                                        },
                                    ),
                                    element_type: Some(
                                        "INTEGER",
                                    ),
                                    options: [],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "brightness",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedColorRgb",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: Some(
                                        ArraySize {
                                            min: 1,
                                            max: 8,
                                        },
                                    ),
                                    element_range: Some(
                                        ElementRange {
                                            min: 0,
                                            max: 14,
                                        },
                                    ),
                                    element_type: Some(
                                        "INTEGER",
                                    ),
                                    options: [],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorRgb",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 0,
                            max: 16777215,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorTemperatureK",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 2000,
                            max: 9000,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "lightScene",
                    parameters: Enum {
                        options: [],
                    },
                },
                DeviceCapability {
                    kind: MusicSetting,
                    instance: "musicMode",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "musicMode",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "Energic",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: 2,
                                        },
                                        EnumOption {
                                            name: "Bounce",
                                            value: 3,
                                        },
                                        EnumOption {
                                            name: "Hopping",
                                            value: 4,
                                        },
                                        EnumOption {
                                            name: "Strike",
                                            value: 5,
                                        },
                                        EnumOption {
                                            name: "Vibrate",
                                            value: 6,
                                        },
                                    ],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "sensitivity",
                                field_type: Integer {
                                    unit: Some(
                                        "unit.percent",
                                    ),
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "autoColor",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "on",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: 0,
                                        },
                                    ],
                                },
                                required: false,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: false,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "diyScene",
                    parameters: Enum {
                        options: [],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "snapshot",
                    parameters: Enum {
                        options: [],
                    },
                },
            ],
        },
        HttpDeviceInfo {
            sku: "H6072",
            device: "AA:BB:CC:DD:AA:BB:CC:DD",
            device_name: "Light",
            device_type: Light,
            capabilities: [
                DeviceCapability {
                    kind: OnOff,
                    instance: "powerSwitch",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Toggle,
                    instance: "gradientToggle",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Range,
                    instance: "brightness",
                    parameters: Integer {
                        unit: Some(
                            "unit.percent",
                        ),
                        range: IntegerRange {
                            min: 1,
                            max: 100,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedBrightness",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: Some(
                                        ArraySize {
                                            min: 1,
                                            max: 8,
                                        },
                                    ),
                                    element_range: Some(
                                        ElementRange {
                                            min: 0,
                                            max: 14,
                                        },
                                    ),
                                    element_type: Some(
                                        "INTEGER",
                                    ),
                                    options: [],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "brightness",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedColorRgb",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: Some(
                                        ArraySize {
                                            min: 1,
                                            max: 8,
                                        },
                                    ),
                                    element_range: Some(
                                        ElementRange {
                                            min: 0,
                                            max: 14,
                                        },
                                    ),
                                    element_type: Some(
                                        "INTEGER",
                                    ),
                                    options: [],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorRgb",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 0,
                            max: 16777215,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorTemperatureK",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 2000,
                            max: 9000,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "lightScene",
                    parameters: Enum {
                        options: [],
                    },
                },
                DeviceCapability {
                    kind: MusicSetting,
                    instance: "musicMode",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "musicMode",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "Energic",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: 2,
                                        },
                                        EnumOption {
                                            name: "Bounce",
                                            value: 3,
                                        },
                                        EnumOption {
                                            name: "Hopping",
                                            value: 4,
                                        },
                                        EnumOption {
                                            name: "Strike",
                                            value: 5,
                                        },
                                        EnumOption {
                                            name: "Vibrate",
                                            value: 6,
                                        },
                                    ],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "sensitivity",
                                field_type: Integer {
                                    unit: Some(
                                        "unit.percent",
                                    ),
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "autoColor",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "on",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: 0,
                                        },
                                    ],
                                },
                                required: false,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: false,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "diyScene",
                    parameters: Enum {
                        options: [],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "snapshot",
                    parameters: Enum {
                        options: [],
                    },
                },
            ],
        },
        HttpDeviceInfo {
            sku: "H6072",
            device: "AA:BB:CC:DD:AA:BB:CC:DD",
            device_name: "Lamp",
            device_type: Light,
            capabilities: [
                DeviceCapability {
                    kind: OnOff,
                    instance: "powerSwitch",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Toggle,
                    instance: "gradientToggle",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Range,
                    instance: "brightness",
                    parameters: Integer {
                        unit: Some(
                            "unit.percent",
                        ),
                        range: IntegerRange {
                            min: 1,
                            max: 100,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedBrightness",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: Some(
                                        ArraySize {
                                            min: 1,
                                            max: 8,
                                        },
                                    ),
                                    element_range: Some(
                                        ElementRange {
                                            min: 0,
                                            max: 14,
                                        },
                                    ),
                                    element_type: Some(
                                        "INTEGER",
                                    ),
                                    options: [],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "brightness",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedColorRgb",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: Some(
                                        ArraySize {
                                            min: 1,
                                            max: 8,
                                        },
                                    ),
                                    element_range: Some(
                                        ElementRange {
                                            min: 0,
                                            max: 14,
                                        },
                                    ),
                                    element_type: Some(
                                        "INTEGER",
                                    ),
                                    options: [],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorRgb",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 0,
                            max: 16777215,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorTemperatureK",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 2000,
                            max: 9000,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "lightScene",
                    parameters: Enum {
                        options: [],
                    },
                },
                DeviceCapability {
                    kind: MusicSetting,
                    instance: "musicMode",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "musicMode",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "Energic",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: 2,
                                        },
                                        EnumOption {
                                            name: "Bounce",
                                            value: 3,
                                        },
                                        EnumOption {
                                            name: "Hopping",
                                            value: 4,
                                        },
                                        EnumOption {
                                            name: "Strike",
                                            value: 5,
                                        },
                                        EnumOption {
                                            name: "Vibrate",
                                            value: 6,
                                        },
                                    ],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "sensitivity",
                                field_type: Integer {
                                    unit: Some(
                                        "unit.percent",
                                    ),
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "autoColor",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "on",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: 0,
                                        },
                                    ],
                                },
                                required: false,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: false,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "diyScene",
                    parameters: Enum {
                        options: [],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "snapshot",
                    parameters: Enum {
                        options: [],
                    },
                },
            ],
        },
    ],
}
"#
        );
    }

    #[test]
    fn list_devices() {
        let resp: GetDevicesResponse = serde_json::from_str(&LIST_DEVICES_EXAMPLE).unwrap();
        k9::snapshot!(
            resp,
            r#"
GetDevicesResponse {
    code: 200,
    message: "success",
    data: [
        HttpDeviceInfo {
            sku: "H6601",
            device: "9D:FA:85:EB:D3:00:8B:FF",
            device_name: "",
            device_type: Light,
            capabilities: [
                DeviceCapability {
                    kind: OnOff,
                    instance: "powerSwitch",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Toggle,
                    instance: "gradientToggle",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Range,
                    instance: "brightness",
                    parameters: Integer {
                        unit: Some(
                            "unit.percent",
                        ),
                        range: IntegerRange {
                            min: 1,
                            max: 100,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedColorRgb",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: None,
                                    element_range: None,
                                    element_type: None,
                                    options: [
                                        ArrayOption {
                                            value: 0,
                                        },
                                        ArrayOption {
                                            value: 1,
                                        },
                                        ArrayOption {
                                            value: 2,
                                        },
                                        ArrayOption {
                                            value: 3,
                                        },
                                        ArrayOption {
                                            value: 4,
                                        },
                                        ArrayOption {
                                            value: 5,
                                        },
                                        ArrayOption {
                                            value: 6,
                                        },
                                        ArrayOption {
                                            value: 7,
                                        },
                                        ArrayOption {
                                            value: 8,
                                        },
                                        ArrayOption {
                                            value: 9,
                                        },
                                        ArrayOption {
                                            value: 10,
                                        },
                                        ArrayOption {
                                            value: 11,
                                        },
                                        ArrayOption {
                                            value: 12,
                                        },
                                        ArrayOption {
                                            value: 13,
                                        },
                                        ArrayOption {
                                            value: 14,
                                        },
                                    ],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorRgb",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 0,
                            max: 16777215,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorTemperatureK",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 2000,
                            max: 9000,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "lightScene",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "Tudum",
                                value: 3054,
                            },
                            EnumOption {
                                name: "Party",
                                value: 3055,
                            },
                            EnumOption {
                                name: "Dance Party",
                                value: 3056,
                            },
                            EnumOption {
                                name: "Dine Together",
                                value: 3057,
                            },
                            EnumOption {
                                name: "Dating",
                                value: 3058,
                            },
                            EnumOption {
                                name: "Adventure",
                                value: 3059,
                            },
                            EnumOption {
                                name: "Technology",
                                value: 3060,
                            },
                            EnumOption {
                                name: "Sports",
                                value: 3061,
                            },
                            EnumOption {
                                name: "Dreamlike",
                                value: 3062,
                            },
                            EnumOption {
                                name: "Dynamic",
                                value: 3063,
                            },
                            EnumOption {
                                name: "Blossom",
                                value: 3064,
                            },
                            EnumOption {
                                name: "Christmas",
                                value: 3065,
                            },
                            EnumOption {
                                name: "Halloween",
                                value: 3066,
                            },
                            EnumOption {
                                name: "Fireworks",
                                value: 3067,
                            },
                            EnumOption {
                                name: "Ghost",
                                value: 3068,
                            },
                            EnumOption {
                                name: "Easter",
                                value: 3069,
                            },
                            EnumOption {
                                name: "Valentine's Day",
                                value: 3070,
                            },
                            EnumOption {
                                name: "Spin",
                                value: 3071,
                            },
                            EnumOption {
                                name: "Stacking",
                                value: 3072,
                            },
                            EnumOption {
                                name: "Shoot",
                                value: 3073,
                            },
                            EnumOption {
                                name: "Racing",
                                value: 3074,
                            },
                            EnumOption {
                                name: "Poker",
                                value: 3075,
                            },
                            EnumOption {
                                name: "Crossing",
                                value: 3076,
                            },
                            EnumOption {
                                name: "Fight",
                                value: 3077,
                            },
                            EnumOption {
                                name: "Electro Dance",
                                value: 3078,
                            },
                            EnumOption {
                                name: "Swing",
                                value: 3079,
                            },
                            EnumOption {
                                name: "Candy Crush",
                                value: 3080,
                            },
                            EnumOption {
                                name: "Portal",
                                value: 3081,
                            },
                            EnumOption {
                                name: "Freeze",
                                value: 3082,
                            },
                            EnumOption {
                                name: "Excited",
                                value: 3083,
                            },
                            EnumOption {
                                name: "Tension",
                                value: 3084,
                            },
                            EnumOption {
                                name: "Fright",
                                value: 3085,
                            },
                            EnumOption {
                                name: "Energetic",
                                value: 3086,
                            },
                            EnumOption {
                                name: "Doubt",
                                value: 3087,
                            },
                            EnumOption {
                                name: "Meditation",
                                value: 3088,
                            },
                            EnumOption {
                                name: "Daze",
                                value: 3089,
                            },
                            EnumOption {
                                name: "Action",
                                value: 3090,
                            },
                            EnumOption {
                                name: "Rivalry",
                                value: 3091,
                            },
                            EnumOption {
                                name: "Puzzle Game",
                                value: 3092,
                            },
                            EnumOption {
                                name: "Shooting Game",
                                value: 3093,
                            },
                            EnumOption {
                                name: "Racing Game",
                                value: 3094,
                            },
                            EnumOption {
                                name: "Card Playing",
                                value: 3095,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: MusicSetting,
                    instance: "musicMode",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "musicMode",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "Energic",
                                            value: 5,
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: 3,
                                        },
                                        EnumOption {
                                            name: "Spectrum",
                                            value: 6,
                                        },
                                        EnumOption {
                                            name: "Rolling",
                                            value: 4,
                                        },
                                    ],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "sensitivity",
                                field_type: Integer {
                                    unit: Some(
                                        "unit.percent",
                                    ),
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "autoColor",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "on",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: 0,
                                        },
                                    ],
                                },
                                required: false,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: Some(
                                        "unit.percent",
                                    ),
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "diyScene",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "Fade",
                                value: 8216567,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "snapshot",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "Sunrise",
                                value: 0,
                            },
                            EnumOption {
                                name: "Sunset",
                                value: 1,
                            },
                        ],
                    },
                },
            ],
        },
        HttpDeviceInfo {
            sku: "H605C",
            device: "69:EC:D1:37:36:39:24:4B",
            device_name: "",
            device_type: Light,
            capabilities: [
                DeviceCapability {
                    kind: OnOff,
                    instance: "powerSwitch",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Toggle,
                    instance: "gradientToggle",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Range,
                    instance: "brightness",
                    parameters: Integer {
                        unit: Some(
                            "unit.percent",
                        ),
                        range: IntegerRange {
                            min: 1,
                            max: 100,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedColorRgb",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: None,
                                    element_range: None,
                                    element_type: None,
                                    options: [
                                        ArrayOption {
                                            value: 0,
                                        },
                                        ArrayOption {
                                            value: 1,
                                        },
                                        ArrayOption {
                                            value: 2,
                                        },
                                        ArrayOption {
                                            value: 3,
                                        },
                                        ArrayOption {
                                            value: 4,
                                        },
                                        ArrayOption {
                                            value: 5,
                                        },
                                        ArrayOption {
                                            value: 6,
                                        },
                                        ArrayOption {
                                            value: 7,
                                        },
                                        ArrayOption {
                                            value: 8,
                                        },
                                        ArrayOption {
                                            value: 9,
                                        },
                                        ArrayOption {
                                            value: 10,
                                        },
                                        ArrayOption {
                                            value: 11,
                                        },
                                        ArrayOption {
                                            value: 12,
                                        },
                                        ArrayOption {
                                            value: 13,
                                        },
                                        ArrayOption {
                                            value: 14,
                                        },
                                    ],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorRgb",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 0,
                            max: 16777215,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorTemperatureK",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 2000,
                            max: 9000,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "lightScene",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "Tudum",
                                value: 3054,
                            },
                            EnumOption {
                                name: "Party",
                                value: 3055,
                            },
                            EnumOption {
                                name: "Dance Party",
                                value: 3056,
                            },
                            EnumOption {
                                name: "Dine Together",
                                value: 3057,
                            },
                            EnumOption {
                                name: "Dating",
                                value: 3058,
                            },
                            EnumOption {
                                name: "Adventure",
                                value: 3059,
                            },
                            EnumOption {
                                name: "Technology",
                                value: 3060,
                            },
                            EnumOption {
                                name: "Sports",
                                value: 3061,
                            },
                            EnumOption {
                                name: "Dreamlike",
                                value: 3062,
                            },
                            EnumOption {
                                name: "Dynamic",
                                value: 3063,
                            },
                            EnumOption {
                                name: "Blossom",
                                value: 3064,
                            },
                            EnumOption {
                                name: "Christmas",
                                value: 3065,
                            },
                            EnumOption {
                                name: "Halloween",
                                value: 3066,
                            },
                            EnumOption {
                                name: "Fireworks",
                                value: 3067,
                            },
                            EnumOption {
                                name: "Ghost",
                                value: 3068,
                            },
                            EnumOption {
                                name: "Easter",
                                value: 3069,
                            },
                            EnumOption {
                                name: "Valentine's Day",
                                value: 3070,
                            },
                            EnumOption {
                                name: "Spin",
                                value: 3071,
                            },
                            EnumOption {
                                name: "Stacking",
                                value: 3072,
                            },
                            EnumOption {
                                name: "Shoot",
                                value: 3073,
                            },
                            EnumOption {
                                name: "Racing",
                                value: 3074,
                            },
                            EnumOption {
                                name: "Poker",
                                value: 3075,
                            },
                            EnumOption {
                                name: "Crossing",
                                value: 3076,
                            },
                            EnumOption {
                                name: "Fight",
                                value: 3077,
                            },
                            EnumOption {
                                name: "Electro Dance",
                                value: 3078,
                            },
                            EnumOption {
                                name: "Swing",
                                value: 3079,
                            },
                            EnumOption {
                                name: "Candy Crush",
                                value: 3080,
                            },
                            EnumOption {
                                name: "Portal",
                                value: 3081,
                            },
                            EnumOption {
                                name: "Freeze",
                                value: 3082,
                            },
                            EnumOption {
                                name: "Excited",
                                value: 3083,
                            },
                            EnumOption {
                                name: "Tension",
                                value: 3084,
                            },
                            EnumOption {
                                name: "Fright",
                                value: 3085,
                            },
                            EnumOption {
                                name: "Energetic",
                                value: 3086,
                            },
                            EnumOption {
                                name: "Doubt",
                                value: 3087,
                            },
                            EnumOption {
                                name: "Meditation",
                                value: 3088,
                            },
                            EnumOption {
                                name: "Daze",
                                value: 3089,
                            },
                            EnumOption {
                                name: "Action",
                                value: 3090,
                            },
                            EnumOption {
                                name: "Rivalry",
                                value: 3091,
                            },
                            EnumOption {
                                name: "Puzzle Game",
                                value: 3092,
                            },
                            EnumOption {
                                name: "Shooting Game",
                                value: 3093,
                            },
                            EnumOption {
                                name: "Racing Game",
                                value: 3094,
                            },
                            EnumOption {
                                name: "Card Playing",
                                value: 3095,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: MusicSetting,
                    instance: "musicMode",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "musicMode",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "Energic",
                                            value: 5,
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: 3,
                                        },
                                        EnumOption {
                                            name: "Spectrum",
                                            value: 4,
                                        },
                                        EnumOption {
                                            name: "Rolling",
                                            value: 6,
                                        },
                                    ],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "sensitivity",
                                field_type: Integer {
                                    unit: Some(
                                        "unit.percent",
                                    ),
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "autoColor",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "on",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: 0,
                                        },
                                    ],
                                },
                                required: false,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: DynamicSetting,
                    instance: "diyScene",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "fade",
                                value: 8216567,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "snapshot",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "color scene",
                                value: 465503,
                            },
                        ],
                    },
                },
            ],
        },
        HttpDeviceInfo {
            sku: "H7055",
            device: "B6:21:C3:37:34:32:33:86",
            device_name: "",
            device_type: Light,
            capabilities: [
                DeviceCapability {
                    kind: OnOff,
                    instance: "powerSwitch",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Toggle,
                    instance: "gradientToggle",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "on",
                                value: 1,
                            },
                            EnumOption {
                                name: "off",
                                value: 0,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: Range,
                    instance: "brightness",
                    parameters: Integer {
                        unit: Some(
                            "unit.percent",
                        ),
                        range: IntegerRange {
                            min: 1,
                            max: 100,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: SegmentColorSetting,
                    instance: "segmentedColorRgb",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "segment",
                                field_type: Array {
                                    size: None,
                                    element_range: None,
                                    element_type: None,
                                    options: [
                                        ArrayOption {
                                            value: 0,
                                        },
                                        ArrayOption {
                                            value: 1,
                                        },
                                        ArrayOption {
                                            value: 2,
                                        },
                                        ArrayOption {
                                            value: 3,
                                        },
                                        ArrayOption {
                                            value: 4,
                                        },
                                        ArrayOption {
                                            value: 5,
                                        },
                                        ArrayOption {
                                            value: 6,
                                        },
                                        ArrayOption {
                                            value: 7,
                                        },
                                        ArrayOption {
                                            value: 8,
                                        },
                                        ArrayOption {
                                            value: 9,
                                        },
                                        ArrayOption {
                                            value: 10,
                                        },
                                        ArrayOption {
                                            value: 11,
                                        },
                                        ArrayOption {
                                            value: 12,
                                        },
                                        ArrayOption {
                                            value: 13,
                                        },
                                        ArrayOption {
                                            value: 14,
                                        },
                                    ],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: None,
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorRgb",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 0,
                            max: 16777215,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: ColorSetting,
                    instance: "colorTemperatureK",
                    parameters: Integer {
                        unit: None,
                        range: IntegerRange {
                            min: 2000,
                            max: 9000,
                            precision: 1,
                        },
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "lightScene",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "Tudum",
                                value: 3054,
                            },
                            EnumOption {
                                name: "Party",
                                value: 3055,
                            },
                            EnumOption {
                                name: "Dance Party",
                                value: 3056,
                            },
                            EnumOption {
                                name: "Dine Together",
                                value: 3057,
                            },
                            EnumOption {
                                name: "Dating",
                                value: 3058,
                            },
                            EnumOption {
                                name: "Adventure",
                                value: 3059,
                            },
                            EnumOption {
                                name: "Technology",
                                value: 3060,
                            },
                            EnumOption {
                                name: "Sports",
                                value: 3061,
                            },
                            EnumOption {
                                name: "Dreamlike",
                                value: 3062,
                            },
                            EnumOption {
                                name: "Dynamic",
                                value: 3063,
                            },
                            EnumOption {
                                name: "Blossom",
                                value: 3064,
                            },
                            EnumOption {
                                name: "Christmas",
                                value: 3065,
                            },
                            EnumOption {
                                name: "Halloween",
                                value: 3066,
                            },
                            EnumOption {
                                name: "Fireworks",
                                value: 3067,
                            },
                            EnumOption {
                                name: "Ghost",
                                value: 3068,
                            },
                            EnumOption {
                                name: "Easter",
                                value: 3069,
                            },
                            EnumOption {
                                name: "Valentine's Day",
                                value: 3070,
                            },
                            EnumOption {
                                name: "Spin",
                                value: 3071,
                            },
                            EnumOption {
                                name: "Stacking",
                                value: 3072,
                            },
                            EnumOption {
                                name: "Shoot",
                                value: 3073,
                            },
                            EnumOption {
                                name: "Racing",
                                value: 3074,
                            },
                            EnumOption {
                                name: "Poker",
                                value: 3075,
                            },
                            EnumOption {
                                name: "Crossing",
                                value: 3076,
                            },
                            EnumOption {
                                name: "Fight",
                                value: 3077,
                            },
                            EnumOption {
                                name: "Electro Dance",
                                value: 3078,
                            },
                            EnumOption {
                                name: "Swing",
                                value: 3079,
                            },
                            EnumOption {
                                name: "Candy Crush",
                                value: 3080,
                            },
                            EnumOption {
                                name: "Portal",
                                value: 3081,
                            },
                            EnumOption {
                                name: "Freeze",
                                value: 3082,
                            },
                            EnumOption {
                                name: "Excited",
                                value: 3083,
                            },
                            EnumOption {
                                name: "Tension",
                                value: 3084,
                            },
                            EnumOption {
                                name: "Fright",
                                value: 3085,
                            },
                            EnumOption {
                                name: "Energetic",
                                value: 3086,
                            },
                            EnumOption {
                                name: "Doubt",
                                value: 3087,
                            },
                            EnumOption {
                                name: "Meditation",
                                value: 3088,
                            },
                            EnumOption {
                                name: "Daze",
                                value: 3089,
                            },
                            EnumOption {
                                name: "Action",
                                value: 3090,
                            },
                            EnumOption {
                                name: "Rivalry",
                                value: 3091,
                            },
                            EnumOption {
                                name: "Puzzle Game",
                                value: 3092,
                            },
                            EnumOption {
                                name: "Shooting Game",
                                value: 3093,
                            },
                            EnumOption {
                                name: "Racing Game",
                                value: 3094,
                            },
                            EnumOption {
                                name: "Card Playing",
                                value: 3095,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: MusicSetting,
                    instance: "musicMode",
                    parameters: Struct {
                        fields: [
                            StructField {
                                field_name: "musicMode",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "Energic",
                                            value: 5,
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: 3,
                                        },
                                        EnumOption {
                                            name: "Spectrum",
                                            value: 6,
                                        },
                                        EnumOption {
                                            name: "Rolling",
                                            value: 4,
                                        },
                                    ],
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "sensitivity",
                                field_type: Integer {
                                    unit: Some(
                                        "unit.percent",
                                    ),
                                    range: IntegerRange {
                                        min: 0,
                                        max: 100,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                            StructField {
                                field_name: "autoColor",
                                field_type: Enum {
                                    options: [
                                        EnumOption {
                                            name: "on",
                                            value: 1,
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: 0,
                                        },
                                    ],
                                },
                                required: false,
                            },
                            StructField {
                                field_name: "rgb",
                                field_type: Integer {
                                    unit: Some(
                                        "unit.percent",
                                    ),
                                    range: IntegerRange {
                                        min: 0,
                                        max: 16777215,
                                        precision: 1,
                                    },
                                },
                                required: true,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "diyScene",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "Fade",
                                value: 8216567,
                            },
                        ],
                    },
                },
                DeviceCapability {
                    kind: DynamicScene,
                    instance: "snapshot",
                    parameters: Enum {
                        options: [
                            EnumOption {
                                name: "Sunrise",
                                value: 0,
                            },
                            EnumOption {
                                name: "Sunset",
                                value: 1,
                            },
                        ],
                    },
                },
            ],
        },
    ],
}
"#
        );
    }
}
