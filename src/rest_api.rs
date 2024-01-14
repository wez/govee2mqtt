use crate::cache::{cache_get, CacheComputeResult, CacheGetOptions};
use crate::platform_api::{http_response_body, ONE_WEEK};
use reqwest::Method;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Value as JsonValue};
use tokio::time::Duration;

// This file im0plements the older Govee REST API as described in:
// <https://govee-public.s3.amazonaws.com/developer-docs/GoveeDeveloperAPIReference.pdf>

const SERVER: &str = "https://developer-api.govee.com";

fn endpoint(url: &str) -> String {
    format!("{SERVER}{url}")
}

#[derive(Clone)]
pub struct RestApiClient {
    key: String,
}

#[allow(unused)]
impl RestApiClient {
    pub fn new<K: Into<String>>(key: K) -> Self {
        Self { key: key.into() }
    }

    pub async fn list_devices(&self) -> anyhow::Result<Vec<RestDeviceInfo>> {
        cache_get(
            CacheGetOptions {
                topic: "rest-api",
                key: "device-list",
                soft_ttl: Duration::from_secs(900),
                hard_ttl: ONE_WEEK,
                negative_ttl: Duration::from_secs(60),
                allow_stale: true,
            },
            async {
                let url = endpoint("/v1/devices");
                let resp: GetDevicesResponse = self.get_request_with_json_response(url).await?;
                Ok(CacheComputeResult::Value(resp.data.devices))
            },
        )
        .await
    }

    pub async fn control_turn(&self, device: &RestDeviceInfo, on: bool) -> anyhow::Result<()> {
        let request = json!({
            "device": device.device,
            "model": device.sku,
            "cmd":{
                "name": "turn",
                "value": if on {"on"} else {"off"},
            }
        });

        let resp: JsonValue = self
            .request_with_json_response(Method::PUT, endpoint("/v1/devices/control"), &request)
            .await?;

        log::trace!("turn response: {resp:?}");

        Ok(())
    }

    pub async fn control_brightness(
        &self,
        device: &RestDeviceInfo,
        percent: u8,
    ) -> anyhow::Result<()> {
        let request = json!({
            "device": device.device,
            "model": device.sku,
            "cmd":{
                "name": "brightness",
                "value": percent,
            }
        });

        let resp: JsonValue = self
            .request_with_json_response(Method::PUT, endpoint("/v1/devices/control"), &request)
            .await?;

        log::trace!("brightness response: {resp:?}");

        Ok(())
    }

    pub async fn control_color(
        &self,
        device: &RestDeviceInfo,
        r: u8,
        g: u8,
        b: u8,
    ) -> anyhow::Result<()> {
        let request = json!({
            "device": device.device,
            "model": device.sku,
            "cmd":{
                "name": "color",
                "value": {
                    "r":r,
                    "g":g,
                    "b":b,
                },
            }
        });

        let resp: JsonValue = self
            .request_with_json_response(Method::PUT, endpoint("/v1/devices/control"), &request)
            .await?;

        log::trace!("color response: {resp:?}");

        Ok(())
    }

    pub async fn control_color_temperature(
        &self,
        device: &RestDeviceInfo,
        kelvin: u16,
    ) -> anyhow::Result<()> {
        let request = json!({
            "device": device.device,
            "model": device.sku,
            "cmd":{
                "name": "colorTem",
                "value": kelvin,
            }
        });

        let resp: JsonValue = self
            .request_with_json_response(Method::PUT, endpoint("/v1/devices/control"), &request)
            .await?;

        log::trace!("colorTem response: {resp:?}");

        Ok(())
    }

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

        http_response_body(response).await
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

        http_response_body(response).await
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
struct GetDevicesResponse {
    code: u32,
    message: String,
    data: GetDevicesDeviceList,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
struct GetDevicesDeviceList {
    devices: Vec<RestDeviceInfo>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct RestDeviceInfo {
    #[serde(rename = "model")]
    pub sku: String,
    pub device: String,
    #[serde(rename = "deviceName", default)]
    pub device_name: String,
    #[serde(default)]
    pub controllable: bool,
    #[serde(default)]
    pub properties: RestDeviceProperties,
    #[serde(default)]
    pub retrievable: bool,
    #[serde(rename = "supportCmds", default)]
    pub supported_commands: Vec<SupportedCommand>,
}

#[derive(Default, Deserialize, Serialize, Debug, Clone)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct RestDeviceProperties {
    #[serde(rename = "colorTem", default)]
    pub color_temperature: Option<ColorTemperatureProperties>,

    pub mode: Option<JsonValue>,
    pub gear: Option<JsonValue>,
}

#[derive(Default, Deserialize, Serialize, Debug, Clone)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct ColorTemperatureProperties {
    pub range: RestRange,
}

#[derive(Default, Deserialize, Serialize, Debug, Clone, Copy)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct RestRange {
    pub min: i64,
    pub max: i64,
}

enum_string! {
pub enum SupportedCommand {
    Turn = "turn",
    Brightness = "brightness",
    Color = "color",
    ColorTemperature = "colorTem",
    Mode = "mode",
    Gear = "gear",
}
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::platform_api::from_json;

    #[test]
    fn list_devices() {
        let resp: GetDevicesResponse =
            from_json(&include_str!("../test-data/rest-list-devices.json")).unwrap();
        k9::assert_matches_snapshot!(format!("{resp:#?}"));
    }

    #[test]
    fn list_appliances() {
        let resp: GetDevicesResponse =
            from_json(&include_str!("../test-data/rest-appliances.json")).unwrap();
        k9::assert_matches_snapshot!(format!("{resp:#?}"));
    }
}
