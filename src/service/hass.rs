use crate::lan_api::DeviceColor;
use crate::opt_env_var;
use crate::service::device::Device as ServiceDevice;
use crate::service::state::StateHandle;
use crate::version_info::govee_version;
use anyhow::Context;
use async_channel::Receiver;
use mosquitto_rs::router::{MqttRouter, Params, Payload, State};
use mosquitto_rs::{Client, Message, QoS};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

const MODEL: &str = "gv2mqtt";
const URL: &str = "https://github.com/wez/govee-rs";

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
            via_device: None,
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

#[derive(Serialize, Clone, Debug)]
pub struct SensorConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub state_topic: String,
    pub unit_of_measurement: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ButtonConfig {
    #[serde(flatten)]
    pub base: EntityConfig,

    pub command_topic: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_press: Option<String>,
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

    pub payload_available: String,
}

impl LightConfig {
    pub async fn for_device(device: &ServiceDevice, state: &StateHandle) -> anyhow::Result<Self> {
        let command_topic = format!("gv2mqtt/light/{id}/command", id = topic_safe_id(device));
        let state_topic = light_state_topic(device);
        let availability_topic = light_availability_topic(device);
        let unique_id = format!("gv2mqtt-{id}", id = topic_safe_id(device));

        let effect_list = state.device_list_scenes(device).await?;

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
            supported_color_modes: vec![
                "rgb".to_string(),
                "color_temp".to_string(),
                //"white".to_string(),
            ],
            color_mode: true,
            brightness: true,
            brightness_scale: 100,
            effect: true,
            effect_list,
            payload_available: "online".to_string(),
        })
    }

    pub async fn publish(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        let disco = state.get_hass_disco_prefix().await;
        let topic = format!(
            "{disco}/light/{unique_id}/config",
            unique_id = self.base.unique_id
        );

        // Delete existing version first
        client
            .client
            .publish(&topic, "", QoS::AtMostOnce, false)
            .await?;
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        client.publish_obj(topic, self).await
    }
}

#[derive(Clone)]
pub struct HassClient {
    client: Client,
}

impl HassClient {
    async fn register_with_hass(&self, state: &StateHandle) -> anyhow::Result<()> {
        let devices = state.devices().await;

        // Register the light entities
        log::trace!("register_with_hass: register entities");
        for d in &devices {
            let light = LightConfig::for_device(&d, state).await?;
            light.publish(state, &self).await?;
        }

        // Allow hass time to register the entities
        tokio::time::sleep(tokio::time::Duration::from_millis(
            (50 * devices.len()) as u64,
        ))
        .await;

        // Mark the lights as available
        log::trace!("register_with_hass: mark as online");
        for d in &devices {
            self.publish(light_availability_topic(d), "online").await?;
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(
            (50 * devices.len()) as u64,
        ))
        .await;

        // report initial state
        log::trace!("register_with_hass: reporting state");
        for d in &devices {
            self.advise_hass_of_light_state(d).await?;
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

    pub async fn advise_hass_of_light_state(&self, device: &ServiceDevice) -> anyhow::Result<()> {
        match device.device_state() {
            Some(device_state) => {
                log::trace!("advise_hass_of_light_state: state is {device_state:?}");

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
                        })
                    } else {
                        json!({
                            "state": "ON",
                            "color_mode": "color_temp",
                            "brightness": device_state.brightness,
                            "color_temp": kelvin_to_mired(device_state.kelvin),
                        })
                    }
                } else {
                    json!({"state":"OFF"})
                };

                self.publish_obj(light_state_topic(device), &light_state)
                    .await?;
            }
            None => {
                // TODO: mark as unavailable or something? Don't
                // want to prevent attempting to control it though,
                // as that could cause it to wake up.
                self.publish_obj(light_state_topic(device), &json!({"state":"OFF"}))
                    .await?;
            }
        }
        self.publish(light_availability_topic(device), "online")
            .await?;

        Ok(())
    }
}

pub fn topic_safe_id(device: &ServiceDevice) -> String {
    let mut id = device.id.to_string();
    id.retain(|c| c != ':');
    id
}

fn light_state_topic(device: &ServiceDevice) -> String {
    format!("gv2mqtt/light/{id}/state", id = topic_safe_id(device))
}

fn light_availability_topic(device: &ServiceDevice) -> String {
    format!("gv2mqtt/light/{id}/avail", id = topic_safe_id(device))
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
    log::info!("Command for {id}: {payload}");
    let device = state
        .resolve_device(&id)
        .await
        .ok_or_else(|| anyhow::anyhow!("device '{id}' not found"))?;

    let command: HassLightCommand = serde_json::from_str(&payload)?;

    if command.state == "OFF" {
        state.device_power_on(&device, false).await?;
    } else if let Some(color) = &command.color {
        state
            .device_set_color_rgb(&device, color.r, color.g, color.b)
            .await?;
    } else if let Some(brightness) = command.brightness {
        state.device_set_brightness(&device, brightness).await?;
    } else if let Some(color_temp) = command.color_temp {
        state
            .device_set_color_temperature(&device, mired_to_kelvin(color_temp))
            .await?;
    } else if let Some(effect) = &command.effect {
        state.device_set_scene(&device, effect).await?;
    } else {
        state.device_power_on(&device, true).await?;
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

    state
        .get_hass_client()
        .await
        .expect("have hass client")
        .register_with_hass(&state)
        .await?;

    while let Ok(msg) = subscriber.recv().await {
        if let Err(err) = router.dispatch(msg.clone(), state.clone()).await {
            log::error!("While dispatching {msg:?}: {err:#}");
        }
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
        }
    });

    Ok(())
}
