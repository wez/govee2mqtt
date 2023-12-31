use crate::cache::{cache_get, CacheGetOptions};
use crate::opt_env_var;
use anyhow::Context;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::time::Duration;

// <https://developer.govee.com/reference/get-you-devices>
const SERVER: &str = "https://openapi.api.govee.com";
const ONE_WEEK: Duration = Duration::from_secs(86400 * 7);

fn endpoint(url: &str) -> String {
    format!("{SERVER}{url}")
}

#[derive(clap::Parser, Debug)]
pub struct GoveeApiArguments {
    /// The Govee API Key. If not passed here, it will be read from
    /// the GOVEE_API_KEY environment variable.
    #[arg(long, global = true)]
    pub api_key: Option<String>,
}

impl GoveeApiArguments {
    pub fn opt_api_key(&self) -> anyhow::Result<Option<String>> {
        match &self.api_key {
            Some(key) => Ok(Some(key.to_string())),
            None => opt_env_var("GOVEE_API_KEY"),
        }
    }

    pub fn api_key(&self) -> anyhow::Result<String> {
        self.opt_api_key()?.ok_or_else(|| {
            anyhow::anyhow!(
                "Please specify the api key either via the \
                --api-key parameter or by setting $GOVEE_API_KEY"
            )
        })
    }

    pub fn api_client(&self) -> anyhow::Result<GoveeApiClient> {
        let key = self.api_key()?;
        Ok(GoveeApiClient::new(key))
    }
}

pub struct GoveeApiClient {
    key: String,
}

impl GoveeApiClient {
    pub fn new<K: Into<String>>(key: K) -> Self {
        Self { key: key.into() }
    }

    pub async fn get_devices(&self) -> anyhow::Result<Vec<HttpDeviceInfo>> {
        cache_get(
            CacheGetOptions {
                topic: "http-api",
                key: "device-list",
                soft_ttl: Duration::from_secs(900),
                hard_ttl: ONE_WEEK,
                negative_ttl: Duration::from_secs(60),
            },
            async {
                let url = endpoint("/router/api/v1/user/devices");
                let resp: GetDevicesResponse = self.get_request_with_json_response(url).await?;
                Ok(resp.data)
            },
        )
        .await
    }

    pub async fn get_device_by_id<I: AsRef<str>>(&self, id: I) -> anyhow::Result<HttpDeviceInfo> {
        let id = id.as_ref();
        let devices = self.get_devices().await?;
        for d in devices {
            if d.device == id {
                return Ok(d);
            }
        }
        anyhow::bail!("device {id} not found");
    }

    pub async fn control_device<V: Into<JsonValue>>(
        &self,
        device: &HttpDeviceInfo,
        capability: &DeviceCapability,
        value: V,
    ) -> anyhow::Result<ControlDeviceResponseCapability> {
        let url = endpoint("/router/api/v1/device/control");
        let request = ControlDeviceRequest {
            request_id: "uuid".to_string(),
            payload: ControlDevicePayload {
                sku: device.sku.to_string(),
                device: device.device.to_string(),
                capability: ControlDeviceCapability {
                    kind: capability.kind,
                    instance: capability.instance.to_string(),
                    value: value.into(),
                },
            },
        };

        let resp: ControlDeviceResponse = self
            .request_with_json_response(Method::POST, url, &request)
            .await?;

        Ok(resp.capability)
    }

    #[allow(unused)]
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

    pub async fn get_device_scenes(
        &self,
        device: &HttpDeviceInfo,
    ) -> anyhow::Result<Vec<DeviceCapability>> {
        let key = format!("scene-list-{}-{}", device.sku, device.device);
        cache_get(
            CacheGetOptions {
                topic: "http-api",
                key: &key,
                soft_ttl: Duration::from_secs(120),
                hard_ttl: ONE_WEEK,
                negative_ttl: Duration::from_secs(60),
            },
            async {
                let url = endpoint("/router/api/v1/device/scenes");
                let request = GetDeviceScenesRequest {
                    request_id: "uuid".to_string(),
                    payload: GetDeviceScenesPayload {
                        sku: device.sku.to_string(),
                        device: device.device.to_string(),
                    },
                };

                let resp: GetDeviceScenesResponse = self
                    .request_with_json_response(Method::POST, url, &request)
                    .await?;

                Ok(resp.payload.capabilities)
            },
        )
        .await
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
struct GetDeviceScenesResponse {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub code: u32,
    #[serde(rename = "msg")]
    pub message: String,
    pub payload: GetDeviceScenesResponsePayload,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
struct GetDeviceScenesResponsePayload {
    pub sku: String,
    pub device: String,
    pub capabilities: Vec<DeviceCapability>,
}

#[derive(Serialize, Debug)]
struct GetDeviceScenesRequest {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub payload: GetDeviceScenesPayload,
}

#[derive(Serialize, Debug)]
struct GetDeviceScenesPayload {
    pub sku: String,
    pub device: String,
}

#[derive(Serialize, Debug)]
struct ControlDeviceRequest {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub payload: ControlDevicePayload,
}

#[derive(Serialize, Debug)]
struct ControlDevicePayload {
    pub sku: String,
    pub device: String,
    pub capability: ControlDeviceCapability,
}

#[derive(Serialize, Debug)]
struct ControlDeviceCapability {
    #[serde(rename = "type")]
    pub kind: DeviceCapabilityKind,
    pub instance: String,
    pub value: JsonValue,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct ControlDeviceResponse {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub code: u32,
    #[serde(rename = "msg")]
    pub message: String,

    pub capability: ControlDeviceResponseCapability,
}

#[derive(Deserialize, Debug)]
pub struct ControlDeviceResponseCapability {
    #[serde(rename = "type")]
    pub kind: DeviceCapabilityKind,
    pub instance: String,
    pub value: JsonValue,
    pub state: JsonValue,
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

#[derive(Deserialize, Serialize, Debug, Clone)]
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

impl HttpDeviceInfo {
    pub fn capability_by_instance(&self, instance: &str) -> Option<&DeviceCapability> {
        self.capabilities.iter().find(|c| c.instance == instance)
    }
}

#[derive(Deserialize, Serialize, Debug, Default, Clone, Copy)]
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

#[derive(Deserialize, Serialize, Debug, Clone, Copy)]
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

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct DeviceCapability {
    #[serde(rename = "type")]
    pub kind: DeviceCapabilityKind,
    pub instance: String,
    pub parameters: DeviceParameters,
}

impl DeviceCapability {
    pub fn enum_parameter_by_name(&self, name: &str) -> Option<u32> {
        match &self.parameters {
            DeviceParameters::Enum { options } => options
                .iter()
                .find(|e| e.name == name && e.value.is_i64())
                .map(|e| e.value.as_i64().expect("i64") as u32),
            _ => None,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
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

#[derive(Deserialize, Serialize, Debug, Clone)]
// No deny_unknown_fields here, because we embed via flatten
pub struct StructField {
    #[serde(rename = "fieldName")]
    pub field_name: String,

    #[serde(flatten)]
    pub field_type: DeviceParameters,

    pub required: bool,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ElementRange {
    pub min: u32,
    pub max: u32,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ArraySize {
    pub min: u32,
    pub max: u32,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct IntegerRange {
    pub min: u32,
    pub max: u32,
    pub precision: u32,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct EnumOption {
    pub name: String,
    pub value: JsonValue,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
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

    #[allow(unused)]
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

    const SCENE_LIST: &str = include_str!("../test-data/scenes.json");

    #[test]
    fn get_device_scenes() {
        let resp: GetDeviceScenesResponse = serde_json::from_str(&SCENE_LIST).unwrap();
        k9::snapshot!(
            resp,
            r#"
GetDeviceScenesResponse {
    request_id: "uuid",
    code: 200,
    message: "success",
    payload: GetDeviceScenesResponsePayload {
        sku: "H6072",
        device: "AA:BB:CC:DD:EE:AA:BB:FF",
        capabilities: [
            DeviceCapability {
                kind: DynamicScene,
                instance: "lightScene",
                parameters: Enum {
                    options: [
                        EnumOption {
                            name: "rainbow B",
                            value: Object {
                                "id": Number(7691),
                                "paramId": Number(11837),
                            },
                        },
                        EnumOption {
                            name: "Sunrise",
                            value: Object {
                                "id": Number(1606),
                                "paramId": Number(1681),
                            },
                        },
                        EnumOption {
                            name: "Sunset",
                            value: Object {
                                "id": Number(1607),
                                "paramId": Number(1682),
                            },
                        },
                        EnumOption {
                            name: "Ocean",
                            value: Object {
                                "id": Number(1608),
                                "paramId": Number(1683),
                            },
                        },
                        EnumOption {
                            name: "Forest",
                            value: Object {
                                "id": Number(1609),
                                "paramId": Number(1684),
                            },
                        },
                        EnumOption {
                            name: "Sunset Glow",
                            value: Object {
                                "id": Number(1610),
                                "paramId": Number(1685),
                            },
                        },
                        EnumOption {
                            name: "Ripple",
                            value: Object {
                                "id": Number(1611),
                                "paramId": Number(1686),
                            },
                        },
                        EnumOption {
                            name: "Rainbow",
                            value: Object {
                                "id": Number(1612),
                                "paramId": Number(1687),
                            },
                        },
                        EnumOption {
                            name: "Meteor",
                            value: Object {
                                "id": Number(1613),
                                "paramId": Number(1688),
                            },
                        },
                        EnumOption {
                            name: "Aurora",
                            value: Object {
                                "id": Number(1614),
                                "paramId": Number(1689),
                            },
                        },
                        EnumOption {
                            name: "Karst Cave",
                            value: Object {
                                "id": Number(1615),
                                "paramId": Number(1690),
                            },
                        },
                        EnumOption {
                            name: "Glacier",
                            value: Object {
                                "id": Number(1616),
                                "paramId": Number(1691),
                            },
                        },
                        EnumOption {
                            name: "Lake",
                            value: Object {
                                "id": Number(1617),
                                "paramId": Number(1692),
                            },
                        },
                        EnumOption {
                            name: "Fire",
                            value: Object {
                                "id": Number(1618),
                                "paramId": Number(1693),
                            },
                        },
                        EnumOption {
                            name: "Journey of Flowers",
                            value: Object {
                                "id": Number(1619),
                                "paramId": Number(1694),
                            },
                        },
                        EnumOption {
                            name: "Downpour",
                            value: Object {
                                "id": Number(1620),
                                "paramId": Number(1695),
                            },
                        },
                        EnumOption {
                            name: "Rustling leaves",
                            value: Object {
                                "id": Number(1621),
                                "paramId": Number(1696),
                            },
                        },
                        EnumOption {
                            name: "Wave",
                            value: Object {
                                "id": Number(1622),
                                "paramId": Number(1697),
                            },
                        },
                        EnumOption {
                            name: "Morning",
                            value: Object {
                                "id": Number(1623),
                                "paramId": Number(1698),
                            },
                        },
                        EnumOption {
                            name: "Night",
                            value: Object {
                                "id": Number(1624),
                                "paramId": Number(1699),
                            },
                        },
                        EnumOption {
                            name: "Cherry blossoms",
                            value: Object {
                                "id": Number(1625),
                                "paramId": Number(1700),
                            },
                        },
                        EnumOption {
                            name: "Movie",
                            value: Object {
                                "id": Number(1626),
                                "paramId": Number(1701),
                            },
                        },
                        EnumOption {
                            name: "Leisure",
                            value: Object {
                                "id": Number(1627),
                                "paramId": Number(1702),
                            },
                        },
                        EnumOption {
                            name: "Night Light",
                            value: Object {
                                "id": Number(1628),
                                "paramId": Number(1703),
                            },
                        },
                        EnumOption {
                            name: "Romantic",
                            value: Object {
                                "id": Number(1629),
                                "paramId": Number(1704),
                            },
                        },
                        EnumOption {
                            name: "Fireworks",
                            value: Object {
                                "id": Number(1630),
                                "paramId": Number(1705),
                            },
                        },
                        EnumOption {
                            name: "Tunnel",
                            value: Object {
                                "id": Number(1631),
                                "paramId": Number(1706),
                            },
                        },
                        EnumOption {
                            name: "Drinks",
                            value: Object {
                                "id": Number(1632),
                                "paramId": Number(1707),
                            },
                        },
                        EnumOption {
                            name: "Work",
                            value: Object {
                                "id": Number(1633),
                                "paramId": Number(1708),
                            },
                        },
                        EnumOption {
                            name: "Study",
                            value: Object {
                                "id": Number(1634),
                                "paramId": Number(1709),
                            },
                        },
                        EnumOption {
                            name: "Candy",
                            value: Object {
                                "id": Number(1635),
                                "paramId": Number(1710),
                            },
                        },
                        EnumOption {
                            name: "Breathe",
                            value: Object {
                                "id": Number(1636),
                                "paramId": Number(1711),
                            },
                        },
                        EnumOption {
                            name: "Gradient",
                            value: Object {
                                "id": Number(1637),
                                "paramId": Number(1712),
                            },
                        },
                        EnumOption {
                            name: "Energetic",
                            value: Object {
                                "id": Number(1638),
                                "paramId": Number(1713),
                            },
                        },
                        EnumOption {
                            name: "Dreamlike",
                            value: Object {
                                "id": Number(1639),
                                "paramId": Number(1714),
                            },
                        },
                        EnumOption {
                            name: "Dreamland",
                            value: Object {
                                "id": Number(1640),
                                "paramId": Number(1715),
                            },
                        },
                        EnumOption {
                            name: "Fight",
                            value: Object {
                                "id": Number(1641),
                                "paramId": Number(1716),
                            },
                        },
                        EnumOption {
                            name: "Light",
                            value: Object {
                                "id": Number(1642),
                                "paramId": Number(1717),
                            },
                        },
                        EnumOption {
                            name: "Tenderness",
                            value: Object {
                                "id": Number(1643),
                                "paramId": Number(1718),
                            },
                        },
                        EnumOption {
                            name: "Warm",
                            value: Object {
                                "id": Number(1644),
                                "paramId": Number(1719),
                            },
                        },
                        EnumOption {
                            name: "Cheerful",
                            value: Object {
                                "id": Number(1645),
                                "paramId": Number(1720),
                            },
                        },
                        EnumOption {
                            name: "Rush",
                            value: Object {
                                "id": Number(1646),
                                "paramId": Number(1721),
                            },
                        },
                        EnumOption {
                            name: "Profound",
                            value: Object {
                                "id": Number(1647),
                                "paramId": Number(1722),
                            },
                        },
                        EnumOption {
                            name: "Daze",
                            value: Object {
                                "id": Number(1648),
                                "paramId": Number(1723),
                            },
                        },
                        EnumOption {
                            name: "Halloween",
                            value: Object {
                                "id": Number(1649),
                                "paramId": Number(1724),
                            },
                        },
                        EnumOption {
                            name: "Christmas",
                            value: Object {
                                "id": Number(1650),
                                "paramId": Number(1725),
                            },
                        },
                        EnumOption {
                            name: "Party",
                            value: Object {
                                "id": Number(1651),
                                "paramId": Number(1726),
                            },
                        },
                        EnumOption {
                            name: "Celebration",
                            value: Object {
                                "id": Number(1652),
                                "paramId": Number(1727),
                            },
                        },
                        EnumOption {
                            name: "Ghost",
                            value: Object {
                                "id": Number(1653),
                                "paramId": Number(1728),
                            },
                        },
                        EnumOption {
                            name: "Poppin",
                            value: Object {
                                "id": Number(1664),
                                "paramId": Number(1739),
                            },
                        },
                        EnumOption {
                            name: "Swing",
                            value: Object {
                                "id": Number(1665),
                                "paramId": Number(1740),
                            },
                        },
                        EnumOption {
                            name: "Racing",
                            value: Object {
                                "id": Number(1666),
                                "paramId": Number(1741),
                            },
                        },
                        EnumOption {
                            name: "Flash",
                            value: Object {
                                "id": Number(1667),
                                "paramId": Number(1742),
                            },
                        },
                        EnumOption {
                            name: "Marbles",
                            value: Object {
                                "id": Number(1668),
                                "paramId": Number(1743),
                            },
                        },
                        EnumOption {
                            name: "Split",
                            value: Object {
                                "id": Number(1669),
                                "paramId": Number(1744),
                            },
                        },
                        EnumOption {
                            name: "Stacking",
                            value: Object {
                                "id": Number(1654),
                                "paramId": Number(1729),
                            },
                        },
                        EnumOption {
                            name: "Greedy Snake",
                            value: Object {
                                "id": Number(1655),
                                "paramId": Number(1730),
                            },
                        },
                        EnumOption {
                            name: "Bouncing Ball",
                            value: Object {
                                "id": Number(1656),
                                "paramId": Number(1731),
                            },
                        },
                        EnumOption {
                            name: "Strike",
                            value: Object {
                                "id": Number(1657),
                                "paramId": Number(1732),
                            },
                        },
                        EnumOption {
                            name: "Bubble",
                            value: Object {
                                "id": Number(1658),
                                "paramId": Number(1733),
                            },
                        },
                        EnumOption {
                            name: "Crossing",
                            value: Object {
                                "id": Number(1659),
                                "paramId": Number(1734),
                            },
                        },
                        EnumOption {
                            name: "Electro Dance",
                            value: Object {
                                "id": Number(1660),
                                "paramId": Number(1735),
                            },
                        },
                        EnumOption {
                            name: "Flow",
                            value: Object {
                                "id": Number(1661),
                                "paramId": Number(1736),
                            },
                        },
                        EnumOption {
                            name: "Accumulation",
                            value: Object {
                                "id": Number(1662),
                                "paramId": Number(1737),
                            },
                        },
                        EnumOption {
                            name: "Release",
                            value: Object {
                                "id": Number(1663),
                                "paramId": Number(1738),
                            },
                        },
                    ],
                },
            },
        ],
    },
}
"#
        );
    }

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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: Number(2),
                                        },
                                        EnumOption {
                                            name: "Bounce",
                                            value: Number(3),
                                        },
                                        EnumOption {
                                            name: "Hopping",
                                            value: Number(4),
                                        },
                                        EnumOption {
                                            name: "Strike",
                                            value: Number(5),
                                        },
                                        EnumOption {
                                            name: "Vibrate",
                                            value: Number(6),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: Number(2),
                                        },
                                        EnumOption {
                                            name: "Bounce",
                                            value: Number(3),
                                        },
                                        EnumOption {
                                            name: "Hopping",
                                            value: Number(4),
                                        },
                                        EnumOption {
                                            name: "Strike",
                                            value: Number(5),
                                        },
                                        EnumOption {
                                            name: "Vibrate",
                                            value: Number(6),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: Number(2),
                                        },
                                        EnumOption {
                                            name: "Spectrum",
                                            value: Number(3),
                                        },
                                        EnumOption {
                                            name: "Rolling",
                                            value: Number(4),
                                        },
                                        EnumOption {
                                            name: "Separation",
                                            value: Number(5),
                                        },
                                        EnumOption {
                                            name: "Hopping",
                                            value: Number(6),
                                        },
                                        EnumOption {
                                            name: "PianoKeys",
                                            value: Number(7),
                                        },
                                        EnumOption {
                                            name: "Fountain",
                                            value: Number(8),
                                        },
                                        EnumOption {
                                            name: "DayAndNight",
                                            value: Number(9),
                                        },
                                        EnumOption {
                                            name: "Sprouting",
                                            value: Number(10),
                                        },
                                        EnumOption {
                                            name: "Shiny",
                                            value: Number(11),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: Number(2),
                                        },
                                        EnumOption {
                                            name: "Spectrum",
                                            value: Number(3),
                                        },
                                        EnumOption {
                                            name: "Rolling",
                                            value: Number(4),
                                        },
                                        EnumOption {
                                            name: "Separation",
                                            value: Number(5),
                                        },
                                        EnumOption {
                                            name: "Hopping",
                                            value: Number(6),
                                        },
                                        EnumOption {
                                            name: "PianoKeys",
                                            value: Number(7),
                                        },
                                        EnumOption {
                                            name: "Fountain",
                                            value: Number(8),
                                        },
                                        EnumOption {
                                            name: "DayAndNight",
                                            value: Number(9),
                                        },
                                        EnumOption {
                                            name: "Sprouting",
                                            value: Number(10),
                                        },
                                        EnumOption {
                                            name: "Shiny",
                                            value: Number(11),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: Number(2),
                                        },
                                        EnumOption {
                                            name: "Spectrum",
                                            value: Number(3),
                                        },
                                        EnumOption {
                                            name: "Rolling",
                                            value: Number(4),
                                        },
                                        EnumOption {
                                            name: "Separation",
                                            value: Number(5),
                                        },
                                        EnumOption {
                                            name: "Hopping",
                                            value: Number(6),
                                        },
                                        EnumOption {
                                            name: "PianoKeys",
                                            value: Number(7),
                                        },
                                        EnumOption {
                                            name: "Fountain",
                                            value: Number(8),
                                        },
                                        EnumOption {
                                            name: "DayandNight",
                                            value: Number(9),
                                        },
                                        EnumOption {
                                            name: "Sprouting",
                                            value: Number(10),
                                        },
                                        EnumOption {
                                            name: "Shiny",
                                            value: Number(11),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "Calm",
                                            value: Number(2),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: Number(2),
                                        },
                                        EnumOption {
                                            name: "Bounce",
                                            value: Number(3),
                                        },
                                        EnumOption {
                                            name: "Hopping",
                                            value: Number(4),
                                        },
                                        EnumOption {
                                            name: "Strike",
                                            value: Number(5),
                                        },
                                        EnumOption {
                                            name: "Vibrate",
                                            value: Number(6),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: Number(2),
                                        },
                                        EnumOption {
                                            name: "Bounce",
                                            value: Number(3),
                                        },
                                        EnumOption {
                                            name: "Hopping",
                                            value: Number(4),
                                        },
                                        EnumOption {
                                            name: "Strike",
                                            value: Number(5),
                                        },
                                        EnumOption {
                                            name: "Vibrate",
                                            value: Number(6),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: Number(2),
                                        },
                                        EnumOption {
                                            name: "Bounce",
                                            value: Number(3),
                                        },
                                        EnumOption {
                                            name: "Hopping",
                                            value: Number(4),
                                        },
                                        EnumOption {
                                            name: "Strike",
                                            value: Number(5),
                                        },
                                        EnumOption {
                                            name: "Vibrate",
                                            value: Number(6),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                value: Number(3054),
                            },
                            EnumOption {
                                name: "Party",
                                value: Number(3055),
                            },
                            EnumOption {
                                name: "Dance Party",
                                value: Number(3056),
                            },
                            EnumOption {
                                name: "Dine Together",
                                value: Number(3057),
                            },
                            EnumOption {
                                name: "Dating",
                                value: Number(3058),
                            },
                            EnumOption {
                                name: "Adventure",
                                value: Number(3059),
                            },
                            EnumOption {
                                name: "Technology",
                                value: Number(3060),
                            },
                            EnumOption {
                                name: "Sports",
                                value: Number(3061),
                            },
                            EnumOption {
                                name: "Dreamlike",
                                value: Number(3062),
                            },
                            EnumOption {
                                name: "Dynamic",
                                value: Number(3063),
                            },
                            EnumOption {
                                name: "Blossom",
                                value: Number(3064),
                            },
                            EnumOption {
                                name: "Christmas",
                                value: Number(3065),
                            },
                            EnumOption {
                                name: "Halloween",
                                value: Number(3066),
                            },
                            EnumOption {
                                name: "Fireworks",
                                value: Number(3067),
                            },
                            EnumOption {
                                name: "Ghost",
                                value: Number(3068),
                            },
                            EnumOption {
                                name: "Easter",
                                value: Number(3069),
                            },
                            EnumOption {
                                name: "Valentine's Day",
                                value: Number(3070),
                            },
                            EnumOption {
                                name: "Spin",
                                value: Number(3071),
                            },
                            EnumOption {
                                name: "Stacking",
                                value: Number(3072),
                            },
                            EnumOption {
                                name: "Shoot",
                                value: Number(3073),
                            },
                            EnumOption {
                                name: "Racing",
                                value: Number(3074),
                            },
                            EnumOption {
                                name: "Poker",
                                value: Number(3075),
                            },
                            EnumOption {
                                name: "Crossing",
                                value: Number(3076),
                            },
                            EnumOption {
                                name: "Fight",
                                value: Number(3077),
                            },
                            EnumOption {
                                name: "Electro Dance",
                                value: Number(3078),
                            },
                            EnumOption {
                                name: "Swing",
                                value: Number(3079),
                            },
                            EnumOption {
                                name: "Candy Crush",
                                value: Number(3080),
                            },
                            EnumOption {
                                name: "Portal",
                                value: Number(3081),
                            },
                            EnumOption {
                                name: "Freeze",
                                value: Number(3082),
                            },
                            EnumOption {
                                name: "Excited",
                                value: Number(3083),
                            },
                            EnumOption {
                                name: "Tension",
                                value: Number(3084),
                            },
                            EnumOption {
                                name: "Fright",
                                value: Number(3085),
                            },
                            EnumOption {
                                name: "Energetic",
                                value: Number(3086),
                            },
                            EnumOption {
                                name: "Doubt",
                                value: Number(3087),
                            },
                            EnumOption {
                                name: "Meditation",
                                value: Number(3088),
                            },
                            EnumOption {
                                name: "Daze",
                                value: Number(3089),
                            },
                            EnumOption {
                                name: "Action",
                                value: Number(3090),
                            },
                            EnumOption {
                                name: "Rivalry",
                                value: Number(3091),
                            },
                            EnumOption {
                                name: "Puzzle Game",
                                value: Number(3092),
                            },
                            EnumOption {
                                name: "Shooting Game",
                                value: Number(3093),
                            },
                            EnumOption {
                                name: "Racing Game",
                                value: Number(3094),
                            },
                            EnumOption {
                                name: "Card Playing",
                                value: Number(3095),
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
                                            value: Number(5),
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: Number(3),
                                        },
                                        EnumOption {
                                            name: "Spectrum",
                                            value: Number(6),
                                        },
                                        EnumOption {
                                            name: "Rolling",
                                            value: Number(4),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: Number(0),
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
                                value: Number(8216567),
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
                                value: Number(0),
                            },
                            EnumOption {
                                name: "Sunset",
                                value: Number(1),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                value: Number(3054),
                            },
                            EnumOption {
                                name: "Party",
                                value: Number(3055),
                            },
                            EnumOption {
                                name: "Dance Party",
                                value: Number(3056),
                            },
                            EnumOption {
                                name: "Dine Together",
                                value: Number(3057),
                            },
                            EnumOption {
                                name: "Dating",
                                value: Number(3058),
                            },
                            EnumOption {
                                name: "Adventure",
                                value: Number(3059),
                            },
                            EnumOption {
                                name: "Technology",
                                value: Number(3060),
                            },
                            EnumOption {
                                name: "Sports",
                                value: Number(3061),
                            },
                            EnumOption {
                                name: "Dreamlike",
                                value: Number(3062),
                            },
                            EnumOption {
                                name: "Dynamic",
                                value: Number(3063),
                            },
                            EnumOption {
                                name: "Blossom",
                                value: Number(3064),
                            },
                            EnumOption {
                                name: "Christmas",
                                value: Number(3065),
                            },
                            EnumOption {
                                name: "Halloween",
                                value: Number(3066),
                            },
                            EnumOption {
                                name: "Fireworks",
                                value: Number(3067),
                            },
                            EnumOption {
                                name: "Ghost",
                                value: Number(3068),
                            },
                            EnumOption {
                                name: "Easter",
                                value: Number(3069),
                            },
                            EnumOption {
                                name: "Valentine's Day",
                                value: Number(3070),
                            },
                            EnumOption {
                                name: "Spin",
                                value: Number(3071),
                            },
                            EnumOption {
                                name: "Stacking",
                                value: Number(3072),
                            },
                            EnumOption {
                                name: "Shoot",
                                value: Number(3073),
                            },
                            EnumOption {
                                name: "Racing",
                                value: Number(3074),
                            },
                            EnumOption {
                                name: "Poker",
                                value: Number(3075),
                            },
                            EnumOption {
                                name: "Crossing",
                                value: Number(3076),
                            },
                            EnumOption {
                                name: "Fight",
                                value: Number(3077),
                            },
                            EnumOption {
                                name: "Electro Dance",
                                value: Number(3078),
                            },
                            EnumOption {
                                name: "Swing",
                                value: Number(3079),
                            },
                            EnumOption {
                                name: "Candy Crush",
                                value: Number(3080),
                            },
                            EnumOption {
                                name: "Portal",
                                value: Number(3081),
                            },
                            EnumOption {
                                name: "Freeze",
                                value: Number(3082),
                            },
                            EnumOption {
                                name: "Excited",
                                value: Number(3083),
                            },
                            EnumOption {
                                name: "Tension",
                                value: Number(3084),
                            },
                            EnumOption {
                                name: "Fright",
                                value: Number(3085),
                            },
                            EnumOption {
                                name: "Energetic",
                                value: Number(3086),
                            },
                            EnumOption {
                                name: "Doubt",
                                value: Number(3087),
                            },
                            EnumOption {
                                name: "Meditation",
                                value: Number(3088),
                            },
                            EnumOption {
                                name: "Daze",
                                value: Number(3089),
                            },
                            EnumOption {
                                name: "Action",
                                value: Number(3090),
                            },
                            EnumOption {
                                name: "Rivalry",
                                value: Number(3091),
                            },
                            EnumOption {
                                name: "Puzzle Game",
                                value: Number(3092),
                            },
                            EnumOption {
                                name: "Shooting Game",
                                value: Number(3093),
                            },
                            EnumOption {
                                name: "Racing Game",
                                value: Number(3094),
                            },
                            EnumOption {
                                name: "Card Playing",
                                value: Number(3095),
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
                                            value: Number(5),
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: Number(3),
                                        },
                                        EnumOption {
                                            name: "Spectrum",
                                            value: Number(4),
                                        },
                                        EnumOption {
                                            name: "Rolling",
                                            value: Number(6),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: Number(0),
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
                                value: Number(8216567),
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
                                value: Number(465503),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                value: Number(1),
                            },
                            EnumOption {
                                name: "off",
                                value: Number(0),
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
                                value: Number(3054),
                            },
                            EnumOption {
                                name: "Party",
                                value: Number(3055),
                            },
                            EnumOption {
                                name: "Dance Party",
                                value: Number(3056),
                            },
                            EnumOption {
                                name: "Dine Together",
                                value: Number(3057),
                            },
                            EnumOption {
                                name: "Dating",
                                value: Number(3058),
                            },
                            EnumOption {
                                name: "Adventure",
                                value: Number(3059),
                            },
                            EnumOption {
                                name: "Technology",
                                value: Number(3060),
                            },
                            EnumOption {
                                name: "Sports",
                                value: Number(3061),
                            },
                            EnumOption {
                                name: "Dreamlike",
                                value: Number(3062),
                            },
                            EnumOption {
                                name: "Dynamic",
                                value: Number(3063),
                            },
                            EnumOption {
                                name: "Blossom",
                                value: Number(3064),
                            },
                            EnumOption {
                                name: "Christmas",
                                value: Number(3065),
                            },
                            EnumOption {
                                name: "Halloween",
                                value: Number(3066),
                            },
                            EnumOption {
                                name: "Fireworks",
                                value: Number(3067),
                            },
                            EnumOption {
                                name: "Ghost",
                                value: Number(3068),
                            },
                            EnumOption {
                                name: "Easter",
                                value: Number(3069),
                            },
                            EnumOption {
                                name: "Valentine's Day",
                                value: Number(3070),
                            },
                            EnumOption {
                                name: "Spin",
                                value: Number(3071),
                            },
                            EnumOption {
                                name: "Stacking",
                                value: Number(3072),
                            },
                            EnumOption {
                                name: "Shoot",
                                value: Number(3073),
                            },
                            EnumOption {
                                name: "Racing",
                                value: Number(3074),
                            },
                            EnumOption {
                                name: "Poker",
                                value: Number(3075),
                            },
                            EnumOption {
                                name: "Crossing",
                                value: Number(3076),
                            },
                            EnumOption {
                                name: "Fight",
                                value: Number(3077),
                            },
                            EnumOption {
                                name: "Electro Dance",
                                value: Number(3078),
                            },
                            EnumOption {
                                name: "Swing",
                                value: Number(3079),
                            },
                            EnumOption {
                                name: "Candy Crush",
                                value: Number(3080),
                            },
                            EnumOption {
                                name: "Portal",
                                value: Number(3081),
                            },
                            EnumOption {
                                name: "Freeze",
                                value: Number(3082),
                            },
                            EnumOption {
                                name: "Excited",
                                value: Number(3083),
                            },
                            EnumOption {
                                name: "Tension",
                                value: Number(3084),
                            },
                            EnumOption {
                                name: "Fright",
                                value: Number(3085),
                            },
                            EnumOption {
                                name: "Energetic",
                                value: Number(3086),
                            },
                            EnumOption {
                                name: "Doubt",
                                value: Number(3087),
                            },
                            EnumOption {
                                name: "Meditation",
                                value: Number(3088),
                            },
                            EnumOption {
                                name: "Daze",
                                value: Number(3089),
                            },
                            EnumOption {
                                name: "Action",
                                value: Number(3090),
                            },
                            EnumOption {
                                name: "Rivalry",
                                value: Number(3091),
                            },
                            EnumOption {
                                name: "Puzzle Game",
                                value: Number(3092),
                            },
                            EnumOption {
                                name: "Shooting Game",
                                value: Number(3093),
                            },
                            EnumOption {
                                name: "Racing Game",
                                value: Number(3094),
                            },
                            EnumOption {
                                name: "Card Playing",
                                value: Number(3095),
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
                                            value: Number(5),
                                        },
                                        EnumOption {
                                            name: "Rhythm",
                                            value: Number(3),
                                        },
                                        EnumOption {
                                            name: "Spectrum",
                                            value: Number(6),
                                        },
                                        EnumOption {
                                            name: "Rolling",
                                            value: Number(4),
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
                                            value: Number(1),
                                        },
                                        EnumOption {
                                            name: "off",
                                            value: Number(0),
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
                                value: Number(8216567),
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
                                value: Number(0),
                            },
                            EnumOption {
                                name: "Sunset",
                                value: Number(1),
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
