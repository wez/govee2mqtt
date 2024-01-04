use crate::cache::{cache_get, CacheGetOptions};
use crate::lan_api::boolean_int;
use crate::opt_env_var;
use crate::platform_api::json_body;
use anyhow::Context;
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::path::PathBuf;
use std::time::Duration;
use uuid::Uuid;

// <https://github.com/constructorfleet/homebridge-ultimate-govee/blob/main/src/data/clients/RestClient.ts>

const APP_VERSION: &str = "5.6.01";
const ONE_DAY: Duration = Duration::from_secs(86400);
const ONE_WEEK: Duration = Duration::from_secs(86400 * 7);

fn user_agent() -> String {
    format!(
        "GoveeHome/{APP_VERSION} (com.ihoment.GoVeeSensor; build:2; iOS 16.5.0) Alamofire/5.6.4"
    )
}

pub fn ms_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("unix epoch in the past")
        .as_millis()
        .to_string()
}

#[derive(clap::Parser, Debug)]
pub struct UndocApiArguments {
    /// The email address you registered with Govee.
    /// If not passed here, it will be read from
    /// the GOVEE_EMAIL environment variable.
    #[arg(long, global = true)]
    pub govee_email: Option<String>,

    /// The password for your Govee account.
    /// If not passed here, it will be read from
    /// the GOVEE_PASSWORD environment variable.
    #[arg(long, global = true)]
    pub govee_password: Option<String>,

    /// Where to store the AWS IoT key file.
    #[arg(long, global = true, default_value = "/dev/shm/govee.iot.key")]
    pub govee_iot_key: PathBuf,

    /// Where to store the AWS IoT certificate file.
    #[arg(long, global = true, default_value = "/dev/shm/govee.iot.cert")]
    pub govee_iot_cert: PathBuf,

    /// Where to find the AWS root CA certificate
    #[arg(long, global = true, default_value = "AmazonRootCA1.pem")]
    pub amazon_root_ca: PathBuf,
}

impl UndocApiArguments {
    pub fn opt_email(&self) -> anyhow::Result<Option<String>> {
        match &self.govee_email {
            Some(key) => Ok(Some(key.to_string())),
            None => opt_env_var("GOVEE_EMAIL"),
        }
    }

    pub fn email(&self) -> anyhow::Result<String> {
        self.opt_email()?.ok_or_else(|| {
            anyhow::anyhow!(
                "Please specify the govee account email either via the \
                --govee-email parameter or by setting $GOVEE_EMAIL"
            )
        })
    }

    pub fn opt_password(&self) -> anyhow::Result<Option<String>> {
        match &self.govee_password {
            Some(key) => Ok(Some(key.to_string())),
            None => opt_env_var("GOVEE_PASSWORD"),
        }
    }

    pub fn password(&self) -> anyhow::Result<String> {
        self.opt_password()?.ok_or_else(|| {
            anyhow::anyhow!(
                "Please specify the govee account password either via the \
                --govee-password parameter or by setting $GOVEE_PASSWORD"
            )
        })
    }

    pub fn api_client(&self) -> anyhow::Result<GoveeUndocumentedApi> {
        let email = self.email()?;
        let password = self.password()?;
        Ok(GoveeUndocumentedApi::new(email, password))
    }
}

#[derive(Clone)]
pub struct GoveeUndocumentedApi {
    email: String,
    password: String,
    client_id: String,
}

impl GoveeUndocumentedApi {
    pub fn new<E: Into<String>, P: Into<String>>(email: E, password: P) -> Self {
        let email = email.into();
        let password = password.into();
        let client_id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, email.as_bytes());
        let client_id = format!("{}", client_id.simple());
        Self {
            email,
            password,
            client_id,
        }
    }

    #[allow(unused)]
    pub async fn get_iot_key(&self, token: &str) -> anyhow::Result<IotKey> {
        cache_get(
            CacheGetOptions {
                topic: "undoc-api",
                key: "iot-key",
                soft_ttl: ONE_DAY,
                hard_ttl: ONE_WEEK,
                negative_ttl: Duration::from_secs(10),
            },
            async {
                let response = reqwest::Client::builder()
                    .timeout(Duration::from_secs(30))
                    .build()?
                    .request(Method::GET, "https://app2.govee.com/app/v1/account/iot/key")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("appVersion", APP_VERSION)
                    .header("clientId", &self.client_id)
                    .header("clientType", "1")
                    .header("iotVersion", "0")
                    .header("timestamp", ms_timestamp())
                    .header("User-Agent", user_agent())
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

                #[derive(Deserialize, Debug)]
                #[allow(non_snake_case, dead_code)]
                struct Response {
                    data: IotKey,
                    message: String,
                    status: u64,
                }

                let resp: Response = json_body(response).await.with_context(|| {
                    format!(
                        "request status {}: {}",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or("")
                    )
                })?;

                Ok(resp.data)
            },
        )
        .await
    }

    pub async fn login_account(&self) -> anyhow::Result<LoginAccountResponse> {
        cache_get(
            CacheGetOptions {
                topic: "undoc-api",
                key: "account-info",
                soft_ttl: ONE_DAY,
                hard_ttl: ONE_WEEK,
                negative_ttl: Duration::from_secs(10),
            },
            async {
                let response = reqwest::Client::builder()
                    .timeout(Duration::from_secs(30))
                    .build()?
                    .request(
                        Method::POST,
                        "https://app2.govee.com/account/rest/account/v1/login",
                    )
                    .json(&serde_json::json!({
                        "email": self.email,
                        "password": self.password,
                        "client": &self.client_id,
                    }))
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

                let resp: Response = json_body(response).await.with_context(|| {
                    format!(
                        "request status {}: {}",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or("")
                    )
                })?;

                #[derive(Deserialize, Serialize, Debug)]
                #[allow(non_snake_case, dead_code)]
                struct Response {
                    client: LoginAccountResponse,
                    message: String,
                    status: u64,
                }

                Ok(resp.client)
            },
        )
        .await
    }

    pub async fn get_device_list(&self, token: &str) -> anyhow::Result<DevicesResponse> {
        let response = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?
            .request(
                Method::POST,
                "https://app2.govee.com/device/rest/devices/v1/list",
            )
            .header("Authorization", format!("Bearer {token}"))
            .header("appVersion", APP_VERSION)
            .header("clientId", &self.client_id)
            .header("clientType", "1")
            .header("iotVersion", "0")
            .header("timestamp", ms_timestamp())
            .header("User-Agent", user_agent())
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

        let resp: DevicesResponse = json_body(response).await.with_context(|| {
            format!(
                "request status {}: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            )
        })?;

        Ok(resp)
    }

    /// Login to community-api.govee.com and return the bearer token
    pub async fn login_community(&self) -> anyhow::Result<String> {
        let response = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()?
            .request(Method::POST, "https://community-api.govee.com/os/v1/login")
            .json(&serde_json::json!({
                "email": self.email,
                "password": self.password,
            }))
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

        #[derive(Deserialize, Debug)]
        #[allow(non_snake_case, dead_code)]
        struct Response {
            data: ResponseData,
            message: String,
            status: u64,
        }

        #[derive(Deserialize, Debug)]
        #[allow(non_snake_case, dead_code)]
        struct ResponseData {
            email: String,
            expiredAt: u64,
            headerUrl: String,
            id: u64,
            nickName: String,
            token: String,
        }

        let resp: Response = json_body(response).await.with_context(|| {
            format!(
                "request status {}: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            )
        })?;

        Ok(resp.data.token)
    }

    pub async fn get_scenes_for_device(sku: &str) -> anyhow::Result<Vec<LightEffectCategory>> {
        let key = format!("scenes-{sku}");

        cache_get(
            CacheGetOptions {
                topic: "undoc-api",
                key: &key,
                soft_ttl: ONE_DAY,
                hard_ttl: ONE_WEEK,
                negative_ttl: Duration::from_secs(1),
            },
            async {
                let response = reqwest::Client::builder()
                    .timeout(Duration::from_secs(10))
                    .build()?
                    .request(
                        Method::GET,
                        format!(
                            "https://app2.govee.com/appsku/v1/light-effect-libraries?sku={sku}"
                        ),
                    )
                    .header("AppVersion", APP_VERSION)
                    .header("User-Agent", user_agent())
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

                let resp: LightEffectLibraryResponse =
                    json_body(response).await.with_context(|| {
                        format!(
                            "request status {}: {}",
                            status.as_u16(),
                            status.canonical_reason().unwrap_or("")
                        )
                    })?;

                Ok(resp.data.categories)
            },
        )
        .await
    }

    pub async fn get_saved_one_click_shortcuts(
        &self,
        community_token: &str,
    ) -> anyhow::Result<Vec<OneClickComponent>> {
        let response = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?
            .request(
                Method::GET,
                "https://app2.govee.com/bff-app/v1/exec-plat/home",
            )
            .header("Authorization", format!("Bearer {community_token}"))
            .header("appVersion", APP_VERSION)
            .header("clientId", &self.client_id)
            .header("clientType", "1")
            .header("iotVersion", "0")
            .header("timestamp", ms_timestamp())
            .header("User-Agent", user_agent())
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

        let resp: OneClickResponse = json_body(response).await.with_context(|| {
            format!(
                "request status {}: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            )
        })?;

        Ok(resp.data.components)
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct IotKey {
    pub endpoint: String,
    pub log: String,
    pub p12: String,
    pub p12_pass: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct LightEffectLibraryResponse {
    pub data: LightEffectLibraryCategoryList,
    pub message: String,
    pub status: u32,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct LightEffectLibraryCategoryList {
    pub categories: Vec<LightEffectCategory>,
    pub support_speed: u8,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct LightEffectCategory {
    pub category_id: u32,
    pub category_name: String,
    pub scenes: Vec<LightEffectScene>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct LightEffectScene {
    pub scene_id: u32,
    pub icon_urls: Vec<String>,
    pub scene_name: String,
    pub analytic_name: String,
    pub scene_type: u32,
    pub scene_code: u32,
    pub scence_category_id: u32,
    pub pop_up_prompt: u32,
    pub scenes_hint: String,
    /// Eg: min/max applicable device version constraints
    pub rule: JsonValue,
    pub light_effects: Vec<LightEffectEntry>,
    pub voice_url: String,
    pub create_time: u64,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct LightEffectEntry {
    pub scence_param_id: u32,
    pub scence_name: String,
    /// base64 encoded
    pub scence_param: String,
    pub scene_code: u16,
    pub special_effect: Vec<JsonValue>,
    pub cmd_version: u32,
    pub scene_type: u32,
    pub diy_effect_code: Vec<JsonValue>,
    pub diy_effect_str: String,
    pub rules: Vec<JsonValue>,
    pub speed_info: JsonValue,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct OneClickResponse {
    pub data: OneClickComponentList,
    pub message: String,
    pub status: u32,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct OneClickComponentList {
    pub components: Vec<OneClickComponent>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct OneClickComponent {
    pub can_disable: Option<u8>,
    #[serde(deserialize_with = "boolean_int")]
    pub can_manage: bool,

    pub feast_type: Option<u64>,
    #[serde(default)]
    pub feasts: Vec<JsonValue>,

    #[serde(default)]
    pub groups: Vec<JsonValue>,

    pub main_device: Option<JsonValue>,

    pub component_id: u64,
    #[serde(default)]
    pub environments: Vec<JsonValue>,
    pub name: String,
    #[serde(rename = "type")]
    pub component_type: u64,

    pub guide_url: Option<String>,
    pub h5_url: Option<String>,
    pub video_url: Option<String>,

    #[serde(default)]
    pub one_clicks: Vec<OneClick>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct OneClick {
    pub name: String,
    pub plan_type: u32,
    pub preset_id: u32,
    pub preset_state: u32,
    pub siri_engine_id: u32,
    #[serde(rename = "type")]
    pub rule_type: u32,
    pub desc: String,
    #[serde(default)]
    pub exec_rules: Vec<JsonValue>,
    pub group_id: u64,
    pub group_name: String,
    #[serde(default)]
    pub iot_rules: Vec<OneClickIotRule>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct OneClickIotRule {
    pub device_obj: OneClickIotRuleDevice,
    pub rule: Vec<OneClickIotRuleEntry>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct OneClickIotRuleEntry {
    #[serde(deserialize_with = "embedded_json")]
    pub blue_msg: JsonValue,
    pub cmd_type: u64,
    #[serde(deserialize_with = "embedded_json")]
    pub cmd_val: OneClickIotRuleEntryCmd,
    pub device_type: u32,
    #[serde(deserialize_with = "embedded_json")]
    pub iot_msg: JsonValue,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct OneClickIotRuleEntryCmd {
    pub open: Option<u32>,
    pub scenes_code: Option<u16>,
    pub scence_id: Option<u16>,
    pub scenes_str: Option<String>,
    pub scence_param_id: Option<u16>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct OneClickIotRuleDevice {
    pub name: String,
    pub device: String,
    pub sku: String,

    pub topic: String,

    pub ble_address: String,
    pub ble_name: String,
    pub device_splicing_status: u32,
    pub feast_id: u64,
    pub feast_name: String,
    pub feast_type: u64,
    pub goods_type: u64,
    pub ic: Option<u32>,
    #[serde(rename = "ic_sub_1")]
    pub ic_sub_1: Option<u32>,
    #[serde(rename = "ic_sub_2")]
    pub ic_sub_2: Option<u32>,
    #[serde(deserialize_with = "boolean_int")]
    pub is_feast: bool,
    pub pact_type: u32,
    pub pact_code: u32,

    pub settings: Option<JsonValue>,
    pub spec: String,
    pub sub_device: String,
    pub sub_device_num: u64,
    pub sub_devices: Option<JsonValue>,

    pub version_hard: String,
    pub version_soft: String,
    pub wifi_soft_version: String,
    pub wifi_hard_version: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LoginAccountResponse {
    #[serde(rename = "A")]
    pub a: String,
    #[serde(rename = "B")]
    pub b: String,
    pub account_id: u64,
    /// this is the client id that we passed in
    pub client: String,
    pub is_savvy_user: bool,
    pub refresh_token: Option<String>,
    pub client_name: Option<String>,
    pub push_token: Option<String>,
    pub version_code: Option<String>,
    pub version_name: Option<String>,
    pub sys_version: Option<String>,
    pub token: String,
    pub token_expire_cycle: u32,
    pub topic: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DevicesResponse {
    pub devices: Vec<DeviceEntry>,
    pub groups: Vec<GroupEntry>,
    pub message: String,
    pub status: u32,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GroupEntry {
    pub group_id: u64,
    pub group_name: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct DeviceEntry {
    pub attributes_id: u32,
    pub device_id: Option<u32>,
    pub device: String,
    pub device_ext: DeviceEntryExt,
    pub device_name: String,
    pub goods_type: u32,
    pub group_id: u64,
    pub pact_code: u32,
    pub pact_type: u32,
    pub share: u32,
    pub sku: String,
    pub spec: String,
    #[serde(deserialize_with = "boolean_int")]
    pub support_scene: bool,
    pub version_hard: String,
    pub version_soft: String,
}
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct DeviceEntryExt {
    #[serde(deserialize_with = "embedded_json")]
    pub device_settings: DeviceSettings,
    #[serde(deserialize_with = "embedded_json")]
    pub ext_resources: ExtResources,
    #[serde(deserialize_with = "embedded_json")]
    pub last_device_data: LastDeviceData,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct DeviceSettings {
    pub wifi_name: String,
    pub address: String,
    pub ble_name: String,
    pub topic: String,
    pub wifi_mac: String,
    pub pact_type: u32,
    pub pact_code: u32,
    pub wifi_soft_version: String,
    pub wifi_hard_version: String,
    pub ic: Option<u32>,
    #[serde(rename = "ic_sub_1")]
    pub ic_sub_1: Option<u32>,
    #[serde(rename = "ic_sub_2")]
    pub ic_sub_2: Option<u32>,
    pub secret_code: Option<String>,
    #[serde(deserialize_with = "boolean_int")]
    pub boil_water_completed_noti_on_off: bool,
    #[serde(deserialize_with = "boolean_int")]
    pub completion_noti_on_off: bool,
    #[serde(deserialize_with = "boolean_int")]
    pub auto_shut_down_on_off: bool,
    pub sku: String,
    pub device: String,
    pub device_name: String,
    pub version_hard: String,
    pub version_soft: String,
    pub play_state: bool,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct ExtResources {
    pub sku_url: String,
    pub head_on_img_new: Option<String>,
    pub head_on_img: String,
    pub head_off_img: String,
    pub head_off_img_new: Option<String>,
    pub ext: String,
    pub ic: u32,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct LastDeviceData {
    pub online: bool,
}

pub fn embedded_json<'de, T: DeserializeOwned, D: serde::de::Deserializer<'de>>(
    deserializer: D,
) -> Result<T, D::Error> {
    use serde::de::Error as _;
    let s = String::deserialize(deserializer)?;
    serde_json::from_str(&s).map_err(|e| D::Error::custom(format!("{e:#}")))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn get_device_scenes() {
        let resp: DevicesResponse =
            serde_json::from_str(include_str!("../test-data/undoc-device-list.json")).unwrap();
        k9::assert_matches_snapshot!(format!("{resp:#?}"));
    }

    #[test]
    fn get_one_click() {
        let resp: OneClickResponse =
            serde_json::from_str(include_str!("../test-data/undoc-one-click.json")).unwrap();
        k9::assert_matches_snapshot!(format!("{resp:#?}"));
    }

    #[test]
    fn light_effect_library() {
        let resp: LightEffectLibraryResponse =
            serde_json::from_str(include_str!("../test-data/light-effect-library-h6072.json"))
                .unwrap();
        k9::assert_matches_snapshot!(format!("{resp:#?}"));
    }
}
