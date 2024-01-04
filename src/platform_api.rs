use crate::cache::{cache_get, CacheGetOptions};
use crate::opt_env_var;
use anyhow::Context;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::time::Duration;

// This file implements the Govee Platform API V1 as described at:
// <https://developer.govee.com/reference/get-you-devices>
//
// It is NOT the same thing as the older, but confusingly versioned
// with a higher number, Govee HTTP API v2 that is described at
// <https://govee.readme.io/reference/getlightdeviceinfo>

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

#[derive(Clone)]
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
                allow_stale: true,
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

    pub async fn get_device_diy_scenes(
        &self,
        device: &HttpDeviceInfo,
    ) -> anyhow::Result<Vec<DeviceCapability>> {
        let key = format!("scene-list-diy-{}-{}", device.sku, device.device);
        cache_get(
            CacheGetOptions {
                topic: "http-api",
                key: &key,
                soft_ttl: Duration::from_secs(300),
                hard_ttl: ONE_WEEK,
                negative_ttl: Duration::from_secs(60),
                allow_stale: true,
            },
            async {
                let url = endpoint("/router/api/v1/device/diy-scenes");
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

    pub async fn get_device_scenes(
        &self,
        device: &HttpDeviceInfo,
    ) -> anyhow::Result<Vec<DeviceCapability>> {
        let key = format!("scene-list-{}-{}", device.sku, device.device);
        cache_get(
            CacheGetOptions {
                topic: "http-api",
                key: &key,
                soft_ttl: Duration::from_secs(300),
                hard_ttl: ONE_WEEK,
                negative_ttl: Duration::from_secs(60),
                allow_stale: true,
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

    pub async fn list_scene_names(&self, device: &HttpDeviceInfo) -> anyhow::Result<Vec<String>> {
        let mut result = vec![];

        let scene_caps = self.get_device_scenes(&device).await?;
        let diy_caps = self.get_device_diy_scenes(&device).await?;

        for caps in [&device.capabilities, &scene_caps, &diy_caps] {
            for cap in caps {
                let is_scene = matches!(
                    cap.kind,
                    DeviceCapabilityKind::DynamicScene | DeviceCapabilityKind::DynamicSetting
                );
                if !is_scene {
                    continue;
                }
                match &cap.parameters {
                    DeviceParameters::Enum { options } => {
                        for opt in options {
                            result.push(opt.name.to_string());
                        }
                    }
                    _ => anyhow::bail!("unexpected type {cap:#?}"),
                }
            }
        }

        Ok(result)
    }

    pub async fn set_scene_by_name(
        &self,
        device: &HttpDeviceInfo,
        scene: &str,
    ) -> anyhow::Result<ControlDeviceResponseCapability> {
        let scene_caps = self.get_device_scenes(&device).await?;
        let diy_caps = self.get_device_diy_scenes(&device).await?;

        for caps in [&device.capabilities, &scene_caps, &diy_caps] {
            for cap in caps {
                let is_scene = matches!(
                    cap.kind,
                    DeviceCapabilityKind::DynamicScene | DeviceCapabilityKind::DynamicSetting
                );
                if !is_scene {
                    continue;
                }
                match &cap.parameters {
                    DeviceParameters::Enum { options } => {
                        for opt in options {
                            if scene.eq_ignore_ascii_case(&opt.name) {
                                return self.control_device(&device, &cap, opt.value.clone()).await;
                            }
                        }
                    }
                    _ => anyhow::bail!("unexpected type {cap:#?}"),
                }
            }
        }
        anyhow::bail!("Scene '{scene}' is not available for this device");
    }

    pub async fn set_power_state(
        &self,
        device: &HttpDeviceInfo,
        on: bool,
    ) -> anyhow::Result<ControlDeviceResponseCapability> {
        let cap = device
            .capability_by_instance("powerSwitch")
            .ok_or_else(|| anyhow::anyhow!("device has no powerSwitch"))?;

        let value = cap
            .enum_parameter_by_name(if on { "on" } else { "off" })
            .ok_or_else(|| anyhow::anyhow!("powerSwitch has no on/off!?"))?;

        self.control_device(&device, &cap, value).await
    }

    pub async fn set_brightness(
        &self,
        device: &HttpDeviceInfo,
        percent: u8,
    ) -> anyhow::Result<ControlDeviceResponseCapability> {
        let cap = device
            .capability_by_instance("brightness")
            .ok_or_else(|| anyhow::anyhow!("device has no brightness"))?;
        let value = match &cap.parameters {
            DeviceParameters::Integer {
                range: IntegerRange { min, max, .. },
                ..
            } => (percent as u32).max(*min).min(*max),
            _ => anyhow::bail!("unexpected parameter type for brightness"),
        };
        self.control_device(&device, &cap, value).await
    }

    pub async fn set_color_temperature(
        &self,
        device: &HttpDeviceInfo,
        kelvin: u32,
    ) -> anyhow::Result<ControlDeviceResponseCapability> {
        let cap = device
            .capability_by_instance("colorTemperatureK")
            .ok_or_else(|| anyhow::anyhow!("device has no colorTemperatureK"))?;
        let value = match &cap.parameters {
            DeviceParameters::Integer {
                range: IntegerRange { min, max, .. },
                ..
            } => (kelvin).max(*min).min(*max),
            _ => anyhow::bail!("unexpected parameter type for colorTemperatureK"),
        };
        self.control_device(&device, &cap, value).await
    }

    pub async fn set_color_rgb(
        &self,
        device: &HttpDeviceInfo,
        r: u8,
        g: u8,
        b: u8,
    ) -> anyhow::Result<ControlDeviceResponseCapability> {
        let cap = device
            .capability_by_instance("colorRgb")
            .ok_or_else(|| anyhow::anyhow!("device has no colorRgb"))?;
        let value = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
        self.control_device(&device, &cap, value).await
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
struct GetDeviceScenesResponse {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub code: u32,
    #[serde(rename = "msg")]
    pub message: String,
    pub payload: GetDeviceScenesResponsePayload,
}

#[derive(Deserialize, Serialize, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
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
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
struct GetDeviceStateResponse {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub code: u32,
    #[serde(rename = "msg")]
    pub message: String,
    pub payload: HttpDeviceState,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct HttpDeviceState {
    pub sku: String,
    pub device: String,
    pub capabilities: Vec<DeviceCapabilityState>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "type")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct DeviceCapabilityState {
    #[serde(rename = "type")]
    pub kind: DeviceCapabilityKind,
    pub instance: String,
    pub state: JsonValue,
}

#[derive(Deserialize, Serialize, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
struct GetDevicesResponse {
    pub code: u32,
    pub message: String,
    pub data: Vec<HttpDeviceInfo>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
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

    pub fn supports_rgb(&self) -> bool {
        self.capability_by_instance("colorRgb").is_some()
    }

    pub fn supports_brightness(&self) -> bool {
        self.capability_by_instance("brightness").is_some()
    }

    pub fn get_color_temperature_range(&self) -> Option<(u32, u32)> {
        let cap = self.capability_by_instance("colorTemperatureK")?;

        match cap.parameters {
            DeviceParameters::Integer {
                range: IntegerRange { min, max, .. },
                ..
            } => Some((min, max)),
            _ => None,
        }
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
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
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
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
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
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct ElementRange {
    pub min: u32,
    pub max: u32,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct ArraySize {
    pub min: u32,
    pub max: u32,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct IntegerRange {
    pub min: u32,
    pub max: u32,
    pub precision: u32,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct EnumOption {
    pub name: String,
    pub value: JsonValue,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct ArrayOption {
    pub value: u32,
}

pub async fn json_body<T: serde::de::DeserializeOwned>(
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
        k9::assert_matches_snapshot!(format!("{resp:#?}"));
    }

    const GET_DEVICE_STATE_EXAMPLE: &str = include_str!("../test-data/get_device_state.json");

    #[test]
    fn get_device_state() {
        let resp: GetDeviceStateResponse = serde_json::from_str(&GET_DEVICE_STATE_EXAMPLE).unwrap();
        k9::assert_matches_snapshot!(format!("{resp:#?}"));
    }

    const LIST_DEVICES_EXAMPLE: &str = include_str!("../test-data/list_devices.json");
    const LIST_DEVICES_EXAMPLE2: &str = include_str!("../test-data/list_devices_2.json");

    #[test]
    fn list_devices_2() {
        let resp: GetDevicesResponse = serde_json::from_str(&LIST_DEVICES_EXAMPLE2).unwrap();
        k9::assert_matches_snapshot!(format!("{resp:#?}"));
    }

    #[test]
    fn list_devices() {
        let resp: GetDevicesResponse = serde_json::from_str(&LIST_DEVICES_EXAMPLE).unwrap();
        k9::assert_matches_snapshot!(format!("{resp:#?}"));
    }
}
