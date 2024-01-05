use crate::lan_api::DeviceColor;
use crate::opt_env_var;
use crate::platform_api::{DeviceCapability, DeviceCapabilityKind};
use crate::service::device::Device as ServiceDevice;
use crate::service::state::{State as ServiceState, StateHandle};
use crate::version_info::govee_version;
use anyhow::Context;
use async_channel::Receiver;
use mosquitto_rs::router::{MqttRouter, Params, Payload, State};
use mosquitto_rs::{Client, Message, QoS};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const MODEL: &str = "gv2mqtt";
const URL: &str = "https://github.com/wez/govee2mqtt";

#[derive(clap::Parser, Debug)]
pub struct HassArguments {
    /// The mqtt broker hostname or address.
    /// You may also set this via the GOVEE_MQTT_HOST environment variable.
    #[arg(long, global = true)]
    mqtt_host: Option<String>,

    /// The mqtt broker port
    /// You may also set this via the GOVEE_MQTT_PORT environment variable.
    /// If unspecified, uses 1883
    #[arg(long, global = true)]
    mqtt_port: Option<u16>,

    /// The username to authenticate against the broker
    /// You may also set this via the GOVEE_MQTT_USER environment variable.
    #[arg(long, global = true)]
    mqtt_username: Option<String>,

    /// The password to authenticate against the broker
    /// You may also set this via the GOVEE_MQTT_PASSWORD environment variable.
    #[arg(long, global = true)]
    mqtt_password: Option<String>,

    #[arg(long, global = true)]
    mqtt_bind_address: Option<String>,

    #[arg(long, global = true, default_value = "homeassistant")]
    hass_discovery_prefix: String,
}

impl HassArguments {
    pub fn opt_mqtt_host(&self) -> anyhow::Result<Option<String>> {
        match &self.mqtt_host {
            Some(h) => Ok(Some(h.to_string())),
            None => opt_env_var("GOVEE_MQTT_HOST"),
        }
    }

    pub fn mqtt_host(&self) -> anyhow::Result<String> {
        self.opt_mqtt_host()?.ok_or_else(|| {
            anyhow::anyhow!(
                "Please specify the mqtt broker either via the \
                --mqtt-host parameter or by setting $GOVEE_MQTT_HOST"
            )
        })
    }

    pub fn mqtt_port(&self) -> anyhow::Result<u16> {
        match self.mqtt_port {
            Some(p) => Ok(p),
            None => Ok(opt_env_var("GOVEE_MQTT_PORT")?.unwrap_or(1883)),
        }
    }

    pub fn mqtt_username(&self) -> anyhow::Result<Option<String>> {
        match self.mqtt_username.clone() {
            Some(u) => Ok(Some(u)),
            None => opt_env_var("GOVEE_MQTT_USER"),
        }
    }

    pub fn mqtt_password(&self) -> anyhow::Result<Option<String>> {
        match self.mqtt_password.clone() {
            Some(u) => Ok(Some(u)),
            None => opt_env_var("GOVEE_MQTT_PASSWORD"),
        }
    }
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct EntityConfig {
    pub availability_topic: String,
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_class: Option<String>,
    pub origin: Origin,
    pub device: Device,
    pub unique_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Origin {
    pub name: &'static str,
    pub sw_version: &'static str,
    pub url: &'static str,
}

impl Default for Origin {
    fn default() -> Self {
        Self {
            name: MODEL,
            sw_version: govee_version(),
            url: URL,
        }
    }
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Device {
    pub name: String,
    pub manufacturer: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sw_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_area: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via_device: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub identifiers: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<(String, String)>,
}

impl Device {
    pub fn for_device(device: &ServiceDevice) -> Self {
        Self {
            name: device.name(),
            manufacturer: "Govee".to_string(),
            model: device.sku.to_string(),
            sw_version: None,
            suggested_area: device.room_name().map(|s| s.to_string()),
            via_device: Some("gv2mqtt".to_string()),
            identifiers: vec![
                format!("gv2mqtt-{}", topic_safe_id(device)),
                /*
                device.computed_name(),
                device.id.to_string(),
                */
            ],
            connections: vec![],
        }
    }

    pub fn this_service() -> Self {
        Self {
            name: "Govee to MQTT".to_string(),
            manufacturer: "Wez Furlong".to_string(),
            model: "govee2mqtt".to_string(),
            sw_version: Some(govee_version().to_string()),
            suggested_area: None,
            via_device: None,
            identifiers: vec!["gv2mqtt".to_string()],
            connections: vec![],
        }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct CoverConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub state_topic: String,
    pub position_topic: String,
    pub set_position_topic: String,
    pub command_topic: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct SceneConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub command_topic: String,
    pub payload_on: String,
}

impl SceneConfig {
    pub async fn publish(
        &self,
        state: &StateHandle,
        client: &HassClient,
        remove: bool,
    ) -> anyhow::Result<()> {
        let disco = state.get_hass_disco_prefix().await;
        let topic = format!(
            "{disco}/scene/{unique_id}/config",
            unique_id = self.base.unique_id
        );

        if remove {
            client.publish(&topic, "").await
        } else {
            client.publish_obj(topic, self).await
        }
    }

    pub async fn notify_state(&self, _client: &HassClient, _: &str) -> anyhow::Result<()> {
        // Scenes have no state
        Ok(())
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct SensorConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub state_topic: String,
    pub unit_of_measurement: Option<String>,
}

impl SensorConfig {
    pub fn global_fixed_diagnostic(name: &str) -> Self {
        let unique_id = format!("global-{}", topic_safe_string(name));
        Self {
            base: EntityConfig {
                availability_topic: availability_topic(),
                name: Some(name.to_string()),
                entity_category: Some("diagnostic".to_string()),
                origin: Origin::default(),
                device: Device::this_service(),
                unique_id: unique_id.clone(),
                device_class: None,
                icon: None,
            },
            state_topic: format!("gv2mqtt/sensor/{unique_id}/state"),
            unit_of_measurement: None,
        }
    }

    pub async fn publish(
        &self,
        state: &StateHandle,
        client: &HassClient,
        remove: bool,
    ) -> anyhow::Result<()> {
        let disco = state.get_hass_disco_prefix().await;
        let topic = format!(
            "{disco}/sensor/{unique_id}/config",
            unique_id = self.base.unique_id
        );

        if remove {
            client.publish(&topic, "").await
        } else {
            client.publish_obj(topic, self).await
        }
    }

    pub async fn notify_state(&self, client: &HassClient, value: &str) -> anyhow::Result<()> {
        client.publish(&self.state_topic, value).await
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct ButtonConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub command_topic: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_press: Option<String>,
}

impl ButtonConfig {
    #[allow(dead_code)]
    pub async fn for_device(
        device: &ServiceDevice,
        instance: &DeviceCapability,
    ) -> anyhow::Result<Self> {
        let command_topic = format!(
            "gv2mqtt/switch/{id}/command/{inst}",
            id = topic_safe_id(device),
            inst = instance.instance
        );
        let availability_topic = availability_topic();
        let unique_id = format!(
            "gv2mqtt-{id}-{inst}",
            id = topic_safe_id(device),
            inst = instance.instance
        );

        Ok(Self {
            base: EntityConfig {
                availability_topic,
                name: Some(camel_case_to_space_separated(&instance.instance)),
                device_class: None,
                origin: Origin::default(),
                device: Device::for_device(device),
                unique_id,
                entity_category: None,
                icon: None,
            },
            command_topic,
            payload_press: None,
        })
    }

    pub async fn publish(
        &self,
        state: &StateHandle,
        client: &HassClient,
        remove: bool,
    ) -> anyhow::Result<()> {
        let disco = state.get_hass_disco_prefix().await;
        let topic = format!(
            "{disco}/button/{unique_id}/config",
            unique_id = self.base.unique_id
        );

        if remove {
            let legacy_topic = format!(
                "{disco}/switch/{unique_id}/config",
                unique_id = self.base.unique_id
            );
            client.publish(&legacy_topic, "").await?;

            client.publish(&topic, "").await
        } else {
            client.publish_obj(topic, self).await
        }
    }

    pub async fn notify_state(
        &self,
        _device: &ServiceDevice,
        _client: &HassClient,
    ) -> anyhow::Result<()> {
        // Buttons have no state
        Ok(())
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct SwitchConfig {
    #[serde(flatten)]
    pub base: EntityConfig,
    pub command_topic: String,
    pub state_topic: String,
}

impl SwitchConfig {
    pub async fn for_device(
        device: &ServiceDevice,
        instance: &DeviceCapability,
    ) -> anyhow::Result<Self> {
        let command_topic = format!(
            "gv2mqtt/switch/{id}/command/{inst}",
            id = topic_safe_id(device),
            inst = instance.instance
        );
        let state_topic = switch_instance_state_topic(device, &instance.instance);
        let availability_topic = availability_topic();
        let unique_id = format!(
            "gv2mqtt-{id}-{inst}",
            id = topic_safe_id(device),
            inst = instance.instance
        );

        Ok(Self {
            base: EntityConfig {
                availability_topic,
                name: Some(camel_case_to_space_separated(&instance.instance)),
                device_class: None,
                origin: Origin::default(),
                device: Device::for_device(device),
                unique_id,
                entity_category: None,
                icon: None,
            },
            command_topic,
            state_topic,
        })
    }

    pub async fn publish(
        &self,
        state: &StateHandle,
        client: &HassClient,
        remove: bool,
    ) -> anyhow::Result<()> {
        let disco = state.get_hass_disco_prefix().await;
        let topic = format!(
            "{disco}/switch/{unique_id}/config",
            unique_id = self.base.unique_id
        );

        if remove {
            if let Some("powerSwitch") = instance_from_topic(&self.command_topic) {
                let legacy_topic = topic.replace("powerSwitch", "power");
                client.publish(&legacy_topic, "").await?;
            }

            client.publish(&topic, "").await
        } else {
            client.publish_obj(topic, self).await
        }
    }

    pub async fn notify_state(
        &self,
        device: &ServiceDevice,
        client: &HassClient,
    ) -> anyhow::Result<()> {
        let instance = instance_from_topic(&self.command_topic).expect("topic to be valid");

        if instance == "powerSwitch" {
            if let Some(state) = device.device_state() {
                client
                    .publish(&self.state_topic, if state.on { "ON" } else { "OFF" })
                    .await?;
            }
            return Ok(());
        }

        // TODO: currently, Govee don't return any meaningful data on
        // additional states. When they do, we'll need to start reporting
        // it here, but we'll also need to start polling it from the
        // platform API in order for it to even be available here.
        // Until then, the switch will show in the hass UI with an
        // unknown state but provide you with separate on and off push
        // buttons so that you can at least send the commands to the device.
        // <https://developer.govee.com/discuss/6596e84c901fb900312d5968>
        if let Some(state) = &device.http_device_state {
            for cap in &state.capabilities {
                if cap.instance == instance {
                    log::warn!("SwitchConfig::notify_state: Do something with {cap:#?}");
                    return Ok(());
                }
            }
        }
        log::trace!("SelectConfig::notify_state: didn't find state for {device} {instance}");
        Ok(())
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct SelectConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub command_topic: String,
    pub options: Vec<String>,
    pub state_topic: String,
}

/// <https://www.home-assistant.io/integrations/light.mqtt/#json-schema>
#[derive(Serialize, Clone, Debug)]
pub struct LightConfig {
    #[serde(flatten)]
    pub base: EntityConfig,
    pub schema: String,

    pub command_topic: String,
    pub state_topic: String,
    pub supported_color_modes: Vec<String>,
    /// Flag that defines if the light supports color modes.
    pub color_mode: bool,
    /// Flag that defines if the light supports brightness.
    pub brightness: bool,
    /// Defines the maximum brightness value (i.e., 100%) of the MQTT device.
    pub brightness_scale: u32,

    /// Flag that defines if the light supports effects.
    pub effect: bool,
    /// The list of effects the light supports.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub effect_list: Vec<String>,

    pub min_mireds: Option<u32>,
    pub max_mireds: Option<u32>,

    pub payload_available: String,
}

impl LightConfig {
    pub async fn for_device(device: &ServiceDevice, state: &ServiceState) -> anyhow::Result<Self> {
        let command_topic = format!("gv2mqtt/light/{id}/command", id = topic_safe_id(device));
        let state_topic = light_state_topic(device);
        let availability_topic = availability_topic();
        let unique_id = format!("gv2mqtt-{id}", id = topic_safe_id(device));

        let effect_list = match state.device_list_scenes(device).await {
            Ok(scenes) => scenes,
            Err(err) => {
                log::error!("Unable to list scenes for {device}: {err:#}");
                vec![]
            }
        };

        let mut supported_color_modes = vec![];
        let mut color_mode = false;

        if device.supports_rgb() {
            supported_color_modes.push("rgb".to_string());
            color_mode = true;
        }

        let (min_mireds, max_mireds) =
            if let Some((min, max)) = device.get_color_temperature_range() {
                supported_color_modes.push("color_temp".to_string());
                color_mode = true;
                // Note that min and max are swapped by the translation
                // from kelvin to mired
                (Some(kelvin_to_mired(max)), Some(kelvin_to_mired(min)))
            } else {
                (None, None)
            };

        let brightness = device
            .http_device_info
            .as_ref()
            .map(|info| info.supports_brightness())
            .unwrap_or(false);

        Ok(Self {
            base: EntityConfig {
                availability_topic,
                name: None,
                device_class: None,
                origin: Origin::default(),
                device: Device::for_device(device),
                unique_id,
                entity_category: None,
                icon: None,
            },
            schema: "json".to_string(),
            command_topic,
            state_topic,
            supported_color_modes,
            color_mode,
            brightness,
            brightness_scale: 100,
            effect: true,
            effect_list,
            payload_available: "online".to_string(),
            max_mireds,
            min_mireds,
        })
    }

    pub async fn publish(
        &self,
        state: &StateHandle,
        client: &HassClient,
        remove: bool,
    ) -> anyhow::Result<()> {
        let disco = state.get_hass_disco_prefix().await;
        let topic = format!(
            "{disco}/light/{unique_id}/config",
            unique_id = self.base.unique_id
        );

        if remove {
            client.publish(&topic, "").await
        } else {
            client.publish_obj(topic, self).await
        }
    }

    pub async fn notify_state(
        &self,
        device: &ServiceDevice,
        client: &HassClient,
    ) -> anyhow::Result<()> {
        match device.device_state() {
            Some(device_state) => {
                log::trace!("LightConfig::notify_state: state is {device_state:?}");

                let light_state = if device_state.on {
                    if device_state.kelvin == 0 {
                        json!({
                            "state": "ON",
                            "color_mode": "rgb",
                            "color": {
                                "r": device_state.color.r,
                                "g": device_state.color.g,
                                "b": device_state.color.b,
                            },
                            "brightness": device_state.brightness,
                            "effect": device_state.scene,
                        })
                    } else {
                        json!({
                            "state": "ON",
                            "color_mode": "color_temp",
                            "brightness": device_state.brightness,
                            "color_temp": kelvin_to_mired(device_state.kelvin),
                            "effect": device_state.scene,
                        })
                    }
                } else {
                    json!({"state":"OFF"})
                };

                client.publish_obj(&self.state_topic, &light_state).await
            }
            None => {
                // TODO: mark as unavailable or something? Don't
                // want to prevent attempting to control it though,
                // as that could cause it to wake up.
                client
                    .publish_obj(&self.state_topic, &json!({"state":"OFF"}))
                    .await
            }
        }
    }
}

enum GlobalConfig {
    Sensor(SensorConfig),
    Scene(SceneConfig),
}

impl GlobalConfig {
    async fn publish(
        &self,
        state: &StateHandle,
        client: &HassClient,
        remove: bool,
    ) -> anyhow::Result<()> {
        match self {
            Self::Sensor(l) => l.publish(state, client, remove).await,
            Self::Scene(s) => s.publish(state, client, remove).await,
        }
    }

    pub async fn notify_state(&self, client: &HassClient, value: &str) -> anyhow::Result<()> {
        match self {
            Self::Sensor(l) => l.notify_state(client, value).await,
            Self::Scene(s) => s.notify_state(client, value).await,
        }
    }
}

enum Config {
    Light(LightConfig),
    Switch(SwitchConfig),
    #[allow(dead_code)]
    Button(ButtonConfig),
}

impl Config {
    async fn publish(
        &self,
        state: &StateHandle,
        client: &HassClient,
        remove: bool,
    ) -> anyhow::Result<()> {
        match self {
            Self::Light(l) => l.publish(state, client, remove).await,
            Self::Switch(s) => s.publish(state, client, remove).await,
            Self::Button(s) => s.publish(state, client, remove).await,
        }
    }

    async fn notify_state(
        &self,
        device: &ServiceDevice,
        client: &HassClient,
    ) -> anyhow::Result<()> {
        match self {
            Self::Light(l) => l.notify_state(device, client).await,
            Self::Switch(s) => s.notify_state(device, client).await,
            Self::Button(s) => s.notify_state(device, client).await,
        }
    }

    async fn for_device<'a>(
        d: &'a ServiceDevice,
        state: &ServiceState,
        configs: &mut Vec<(&'a ServiceDevice, Self)>,
    ) -> anyhow::Result<()> {
        configs.push((d, Config::Light(LightConfig::for_device(&d, state).await?)));
        if let Some(info) = &d.http_device_info {
            for cap in &info.capabilities {
                match cap.kind {
                    DeviceCapabilityKind::Toggle | DeviceCapabilityKind::OnOff => {
                        configs.push((d, Config::Switch(SwitchConfig::for_device(&d, cap).await?)));
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct HassClient {
    client: Client,
}

impl HassClient {
    async fn register_with_hass(&self, state: &StateHandle) -> anyhow::Result<()> {
        let mut globals = vec![(
            GlobalConfig::Sensor(SensorConfig::global_fixed_diagnostic("Version")),
            govee_version().to_string(),
        )];

        if let Some(undoc) = state.get_undoc_client().await {
            match undoc.parse_one_clicks().await {
                Ok(items) => {
                    for oc in items {
                        let unique_id = format!(
                            "gv2mqtt-one-click-{}",
                            Uuid::new_v5(&Uuid::NAMESPACE_DNS, oc.name.as_bytes()).simple()
                        );
                        globals.push((
                            GlobalConfig::Scene(SceneConfig {
                                base: EntityConfig {
                                    availability_topic: availability_topic(),
                                    name: Some(oc.name.to_string()),
                                    entity_category: None,
                                    origin: Origin::default(),
                                    device: Device::this_service(),
                                    unique_id: unique_id.clone(),
                                    device_class: None,
                                    icon: None,
                                },
                                command_topic: oneclick_topic(),
                                payload_on: oc.name,
                            }),
                            "".into(),
                        ));
                    }
                }
                Err(err) => {
                    log::warn!("Failed to parse one-clicks: {err:#}");
                }
            }
        }

        let devices = state.devices().await;

        let mut configs = vec![];

        for d in &devices {
            Config::for_device(d, state, &mut configs)
                .await
                .with_context(|| format!("Config::for_device({d})"))?;
        }

        // Remove existing configs first
        log::trace!("register_with_hass: Remove prior entries");
        for (s, _) in &globals {
            s.publish(state, self, true)
                .await
                .context("delete hass config for a global item")?;
        }
        for (d, c) in &configs {
            c.publish(state, self, true)
                .await
                .with_context(|| format!("delete hass config for {d}"))?;
        }

        // Allow hass time to de-register the entities
        tokio::time::sleep(tokio::time::Duration::from_millis(
            (50 * configs.len()) as u64,
        ))
        .await;

        // Register the configs
        log::trace!("register_with_hass: register entities");
        for (s, _) in &globals {
            s.publish(state, self, false)
                .await
                .context("create hass config for a global item")?;
        }
        for (d, c) in &configs {
            c.publish(state, self, false)
                .await
                .with_context(|| format!("delete hass config for {d}"))?;
        }

        // Allow hass time to register the entities
        tokio::time::sleep(tokio::time::Duration::from_millis(
            (50 * configs.len()) as u64,
        ))
        .await;

        // Mark as available
        log::trace!("register_with_hass: mark as online");
        self.publish(availability_topic(), "online")
            .await
            .context("online -> availability_topic")?;

        // report initial state
        log::trace!("register_with_hass: reporting state");
        for (s, v) in &globals {
            s.notify_state(self, v)
                .await
                .context("publish state for a global item")?;
        }
        for (d, c) in &configs {
            c.notify_state(d, self)
                .await
                .with_context(|| format!("publish state for {d}"))?;
        }

        log::trace!("register_with_hass: done");

        Ok(())
    }

    async fn publish<T: AsRef<str> + std::fmt::Display, P: AsRef<[u8]> + std::fmt::Display>(
        &self,
        topic: T,
        payload: P,
    ) -> anyhow::Result<()> {
        log::trace!("{topic} -> {payload}");
        self.client
            .publish(topic, payload, QoS::AtMostOnce, false)
            .await?;
        Ok(())
    }

    async fn publish_obj<T: AsRef<str> + std::fmt::Display, P: Serialize>(
        &self,
        topic: T,
        payload: P,
    ) -> anyhow::Result<()> {
        let payload = serde_json::to_string(&payload)?;
        log::trace!("{topic} -> {payload}");
        self.client
            .publish(topic, payload, QoS::AtMostOnce, false)
            .await?;
        Ok(())
    }

    pub async fn advise_hass_of_light_state(
        &self,
        device: &ServiceDevice,
        state: &ServiceState,
    ) -> anyhow::Result<()> {
        let mut configs = vec![];
        Config::for_device(device, state, &mut configs).await?;
        for (d, c) in configs {
            c.notify_state(d, self).await?;
        }

        Ok(())
    }
}

pub fn topic_safe_string(s: &str) -> String {
    let mut result = String::new();
    for c in s.chars() {
        if c == ':' {
            result.push('_');
        } else {
            result.push(c.to_ascii_lowercase());
        }
    }
    result
}

pub fn topic_safe_id(device: &ServiceDevice) -> String {
    let mut id = device.id.to_string();
    id.retain(|c| c != ':');
    id
}

fn switch_instance_state_topic(device: &ServiceDevice, instance: &str) -> String {
    format!(
        "gv2mqtt/switch/{id}/{instance}/state",
        id = topic_safe_id(device)
    )
}

fn light_state_topic(device: &ServiceDevice) -> String {
    format!("gv2mqtt/light/{id}/state", id = topic_safe_id(device))
}

/// All entities use the same topic so that we can mark unavailable
/// via last-will
fn availability_topic() -> String {
    "gv2mqtt/availability".to_string()
}

fn oneclick_topic() -> String {
    "gv2mqtt/oneclick".to_string()
}

#[derive(Deserialize)]
struct IdParameter {
    id: String,
}

#[derive(Deserialize, Debug, Clone)]
struct HassLightCommand {
    state: String,
    color_temp: Option<u32>,
    color: Option<DeviceColor>,
    effect: Option<String>,
    brightness: Option<u8>,
}

/// HASS is sending a command to a light
async fn mqtt_light_command(
    Payload(payload): Payload<String>,
    Params(IdParameter { id }): Params<IdParameter>,
    State(state): State<StateHandle>,
) -> anyhow::Result<()> {
    let device = state
        .resolve_device(&id)
        .await
        .ok_or_else(|| anyhow::anyhow!("device '{id}' not found"))?;

    let command: HassLightCommand = serde_json::from_str(&payload)?;
    log::info!("Command for {device}: {payload}");

    if command.state == "OFF" {
        state.device_power_on(&device, false).await?;
    } else {
        let mut power_on = true;

        if let Some(brightness) = command.brightness {
            state.device_set_brightness(&device, brightness).await?;
            power_on = false;
        }

        if let Some(effect) = &command.effect {
            state.device_set_scene(&device, effect).await?;
            // It doesn't make sense to vary color properties
            // at the same time as the scene properties, so
            // ignore those.
            // Brightness, set above, is ok.
            return Ok(());
        }

        if let Some(color) = &command.color {
            state
                .device_set_color_rgb(&device, color.r, color.g, color.b)
                .await?;
            power_on = false;
        }
        if let Some(color_temp) = command.color_temp {
            state
                .device_set_color_temperature(&device, mired_to_kelvin(color_temp))
                .await?;
            power_on = false;
        }
        if power_on {
            state.device_power_on(&device, true).await?;
        }
    }

    Ok(())
}

async fn mqtt_oneclick(
    Payload(name): Payload<String>,
    State(state): State<StateHandle>,
) -> anyhow::Result<()> {
    log::info!("mqtt_oneclick: {name}");

    let undoc = state
        .get_undoc_client()
        .await
        .ok_or_else(|| anyhow::anyhow!("Undoc API client is not available"))?;
    let items = undoc.parse_one_clicks().await?;
    let item = items
        .iter()
        .find(|item| item.name == name)
        .ok_or_else(|| anyhow::anyhow!("didn't find item {name}"))?;

    let iot = state
        .get_iot_client()
        .await
        .ok_or_else(|| anyhow::anyhow!("AWS IoT client is not available"))?;

    iot.activate_one_click(&item).await
}

#[derive(Deserialize)]
struct IdAndInst {
    id: String,
    instance: String,
}

async fn mqtt_switch_command(
    Payload(command): Payload<String>,
    Params(IdAndInst { id, instance }): Params<IdAndInst>,
    State(state): State<StateHandle>,
) -> anyhow::Result<()> {
    log::info!("{instance} for {id}: {command}");
    let device = state
        .resolve_device(&id)
        .await
        .ok_or_else(|| anyhow::anyhow!("device '{id}' not found"))?;

    let on = match command.as_str() {
        "ON" | "on" => true,
        "OFF" | "off" => false,
        _ => anyhow::bail!("invalid {command} for {id}"),
    };

    if instance == "powerSwitch" {
        state.device_power_on(&device, on).await?;
    } else if let Some(client) = state.get_platform_client().await {
        if let Some(http_dev) = &device.http_device_info {
            client.set_toggle_state(http_dev, &instance, on).await?;
        } else {
            anyhow::bail!("No platform state available to set {id} {instance} to {on}");
        }
    } else {
        anyhow::bail!("Don't know how to {command} for {id} {instance}!");
    }

    Ok(())
}

pub fn mired_to_kelvin(mired: u32) -> u32 {
    if mired == 0 {
        0
    } else {
        1000000 / mired
    }
}

pub fn kelvin_to_mired(kelvin: u32) -> u32 {
    if kelvin == 0 {
        0
    } else {
        1000000 / kelvin
    }
}

/// HASS is advising us that its status has changed
async fn mqtt_homeassitant_status(
    Payload(status): Payload<String>,
    State(state): State<StateHandle>,
) -> anyhow::Result<()> {
    log::info!("Home Assistant status changed: {status}");

    let client = state
        .get_hass_client()
        .await
        .expect("hass client to be present");

    client.register_with_hass(&state).await?;

    Ok(())
}

async fn run_mqtt_loop(
    state: StateHandle,
    subscriber: Receiver<Message>,
    client: Client,
) -> anyhow::Result<()> {
    // Give LAN disco a chance to get current state before
    // we register with hass
    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

    let mut router: MqttRouter<StateHandle> = MqttRouter::new(client);

    let disco_prefix = state.get_hass_disco_prefix().await;
    router
        .route(format!("{disco_prefix}/status"), mqtt_homeassitant_status)
        .await?;

    router
        .route("gv2mqtt/light/:id/command", mqtt_light_command)
        .await?;
    router
        .route("gv2mqtt/switch/:id/command/:instance", mqtt_switch_command)
        .await?;

    router.route(oneclick_topic(), mqtt_oneclick).await?;

    state
        .get_hass_client()
        .await
        .expect("have hass client")
        .register_with_hass(&state)
        .await
        .context("register_with_hass")?;

    let router = Arc::new(router);

    while let Ok(msg) = subscriber.recv().await {
        let router = router.clone();
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(err) = router.dispatch(msg.clone(), state.clone()).await {
                log::error!("While dispatching {msg:?}: {err:#}");
            }
        });
    }
    Ok(())
}

pub async fn spawn_hass_integration(
    state: StateHandle,
    args: &HassArguments,
) -> anyhow::Result<()> {
    let client = Client::with_auto_id()?;

    let mqtt_host = args.mqtt_host()?;
    let mqtt_username = args.mqtt_username()?;
    let mqtt_password = args.mqtt_password()?;
    let mqtt_port = args.mqtt_port()?;

    client.set_last_will(availability_topic(), "offline", QoS::AtMostOnce, true)?;

    client.set_username_and_password(mqtt_username.as_deref(), mqtt_password.as_deref())?;
    client
        .connect(
            &mqtt_host,
            mqtt_port.into(),
            Duration::from_secs(10),
            args.mqtt_bind_address.as_deref(),
        )
        .await
        .with_context(|| format!("connecting to mqtt broker {mqtt_host}:{mqtt_port}"))?;
    let subscriber = client.subscriber().expect("to own the subscriber");

    state
        .set_hass_client(HassClient {
            client: client.clone(),
        })
        .await;

    let disco_prefix = args.hass_discovery_prefix.clone();
    state.set_hass_disco_prefix(disco_prefix).await;

    tokio::spawn(async move {
        let res = run_mqtt_loop(state, subscriber, client).await;
        if let Err(err) = res {
            log::error!("run_mqtt_loop: {err:#}");
            log::error!("FATAL: hass integration will not function.");
            log::error!("Pausing for 30 seconds before terminating.");
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
            std::process::exit(1);
        }
    });

    Ok(())
}

fn camel_case_to_space_separated(camel: &str) -> String {
    let mut result = camel[..1].to_ascii_uppercase();
    for c in camel.chars().skip(1) {
        if c.is_uppercase() {
            result.push(' ');
        }
        result.push(c);
    }
    result
}

#[cfg(test)]
#[test]
fn test_camel_case_to_space_separated() {
    assert_eq!(camel_case_to_space_separated("powerSwitch"), "Power Switch");
    assert_eq!(
        camel_case_to_space_separated("oscillationToggle"),
        "Oscillation Toggle"
    );
}

fn instance_from_topic(topic: &str) -> Option<&str> {
    topic.rsplit_once('/').map(|(_, instance)| instance)
}

#[cfg(test)]
#[test]
fn test_instance_from_topic() {
    assert_eq!(
        instance_from_topic("hello/there/powerSwitch").unwrap(),
        "powerSwitch"
    );
}
