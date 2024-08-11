use crate::hass_mqtt::climate::mqtt_set_temperature;
use crate::hass_mqtt::enumerator::{enumerate_all_entites, enumerate_entities_for_device};
use crate::hass_mqtt::humidifier::{mqtt_humidifier_set_work_mode, mqtt_humidifier_set_target };
use crate::hass_mqtt::fan::{mqtt_fan_set_work_mode, mqtt_fan_set_speed, mqtt_fan_set_oscillation};
use crate::hass_mqtt::instance::EntityList;
use crate::hass_mqtt::number::mqtt_number_command;
use crate::hass_mqtt::select::mqtt_set_mode_scene;
use crate::lan_api::DeviceColor;
use crate::opt_env_var;
use crate::platform_api::{from_json, DeviceType};
use crate::service::device::Device as ServiceDevice;
use crate::service::state::StateHandle;
use crate::temperature::TemperatureScale;
use anyhow::Context;
use async_channel::Receiver;
use mosquitto_rs::router::{MqttRouter, Params, Payload, State};
use mosquitto_rs::{Client, Event, QoS};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

const HASS_REGISTER_DELAY: tokio::time::Duration = tokio::time::Duration::from_secs(15);

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

    /// The temperature scale to use when showing temperature values as
    /// entities in home assistant. Can be either "C" or "F" for Celsius
    /// or Farenheit respectively.
    /// You may also set this vai the GOVEE_TEMPERATURE_SCALE environment
    /// variable.
    #[arg(long, global = true)]
    temperature_scale: Option<String>,
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

    pub fn temperature_scale(&self) -> anyhow::Result<TemperatureScale> {
        match &self.temperature_scale {
            Some(s) => Ok(s.parse()?),
            None => {
                Ok(opt_env_var("GOVEE_TEMPERATURE_SCALE")?.unwrap_or(TemperatureScale::Celsius))
            }
        }
    }
}

#[derive(Clone)]
pub struct HassClient {
    client: Client,
}

impl HassClient {
    async fn register_with_hass(&self, state: &StateHandle) -> anyhow::Result<()> {
        let entities = enumerate_all_entites(state).await?;

        // Register the configs
        log::trace!("register_with_hass: register entities");
        entities.publish_config(state, self).await?;

        // Allow hass extra time to register the entities before
        // we mark them as available
        let delay = tokio::time::Duration::from_millis((10 * entities.len()) as u64);
        log::info!(
            "Wait {delay:?} for hass to settle on {} entity configs",
            entities.len()
        );
        tokio::time::sleep(delay).await;

        // Mark as available
        log::trace!("register_with_hass: mark as online");
        self.publish(availability_topic(), "online")
            .await
            .context("online -> availability_topic")?;

        // report initial state
        log::trace!("register_with_hass: reporting state");
        entities.notify_state(self).await.context("notify_state")?;

        log::trace!("register_with_hass: done");

        Ok(())
    }

    pub async fn publish<T: AsRef<str> + std::fmt::Display, P: AsRef<[u8]> + std::fmt::Display>(
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

    pub async fn publish_obj<T: AsRef<str> + std::fmt::Display, P: Serialize>(
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
        state: &StateHandle,
    ) -> anyhow::Result<()> {
        let mut entities = EntityList::new();
        enumerate_entities_for_device(device, state, &mut entities).await?;
        entities.notify_state(self).await?;

        Ok(())
    }
}

pub fn topic_safe_string(s: &str) -> String {
    let mut result = String::new();
    for c in s.chars() {
        if c == ':' || c == ' ' || c == '\\' || c == '/' || c == '\'' || c == '"' {
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
    id.retain(|c| c != ' ');
    id
}

pub fn switch_instance_state_topic(device: &ServiceDevice, instance: &str) -> String {
    format!(
        "gv2mqtt/switch/{id}/{instance}/state",
        id = topic_safe_id(device)
    )
}

pub fn light_state_topic(device: &ServiceDevice) -> String {
    format!("gv2mqtt/light/{id}/state", id = topic_safe_id(device))
}

pub fn light_segment_state_topic(device: &ServiceDevice, segment: u32) -> String {
    format!(
        "gv2mqtt/light/{id}/state/{segment}",
        id = topic_safe_id(device)
    )
}

/// All entities use the same topic so that we can mark unavailable
/// via last-will
pub fn availability_topic() -> String {
    "gv2mqtt/availability".to_string()
}

pub fn oneclick_topic() -> String {
    "gv2mqtt/oneclick".to_string()
}

pub fn purge_cache_topic() -> String {
    "gv2mqtt/purge-caches".to_string()
}

#[derive(Deserialize)]
pub struct IdParameter {
    pub id: String,
}

/// Someone clicked the "Request Platform API State" button
async fn mqtt_request_platform_data(
    Params(IdParameter { id }): Params<IdParameter>,
    State(state): State<StateHandle>,
) -> anyhow::Result<()> {
    let device = state.resolve_device_read_only(&id).await?;
    log::info!("Request Platform API State for {device}");
    if !state.poll_platform_api(&device).await? {
        log::warn!("Unable to poll platform API for {device}");
    }
    Ok(())
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
    let device = state.resolve_device_for_control(&id).await?;

    let command: HassLightCommand = serde_json::from_str(&payload)?;
    log::info!("Command for {device}: {payload}");

    let is_light = device.device_type() == DeviceType::Light;

    if command.state == "OFF" {
        if is_light {
            state
                .device_light_power_on(&device, false)
                .await
                .context("mqtt_light_command: state.device_power_on")?;
        } else {
            state
                .device_set_brightness(&device, 0)
                .await
                .context("mqtt_light_command: state.device_set_brightness")?;
        }
    } else {
        let mut power_on = true;

        if let Some(brightness) = command.brightness {
            state
                .device_set_brightness(&device, brightness)
                .await
                .context("mqtt_light_command: state.device_set_brightness")?;
            power_on = false;
        }

        if let Some(effect) = &command.effect {
            state
                .device_set_scene(&device, effect)
                .await
                .context("mqtt_light_command: state.device_set_scene")?;
            // It doesn't make sense to vary color properties
            // at the same time as the scene properties, so
            // ignore those.
            // Brightness, set above, is ok.
            return Ok(());
        }

        if let Some(color) = &command.color {
            state
                .device_set_color_rgb(&device, color.r, color.g, color.b)
                .await
                .context("mqtt_light_command: state.device_set_color_rgb")?;
            power_on = false;
        }
        if let Some(color_temp) = command.color_temp {
            state
                .device_set_color_temperature(&device, mired_to_kelvin(color_temp))
                .await
                .context("mqtt_light_command: state.device_set_color_temperature")?;
            power_on = false;
        }

        if power_on {
            if is_light {
                state
                    .device_light_power_on(&device, true)
                    .await
                    .context("mqtt_light_command: state.device_power_on")?;
            } else if command.brightness.is_none() {
                // The device is not primarily a light and we don't have
                // a guaranteed way to power it on without setting the
                // brightness to something, and we know we didn't set
                // the brightness just now, so let's turn it on 100%
                state
                    .device_set_brightness(&device, 100)
                    .await
                    .context("mqtt_light_command: state.device_set_brightness")?;
            }
        }
    }

    Ok(())
}

#[derive(Deserialize)]
struct IdAndSeg {
    id: String,
    segment: String,
}

async fn mqtt_light_segment_command(
    Payload(payload): Payload<String>,
    Params(IdAndSeg { id, segment }): Params<IdAndSeg>,
    State(state): State<StateHandle>,
) -> anyhow::Result<()> {
    let device = state.resolve_device_for_control(&id).await?;
    let segment: u32 = segment.parse()?;

    let command: HassLightCommand = from_json(&payload)?;
    log::info!("Command for {device} segment {segment}: {payload}");

    if let Some(client) = state.get_platform_client().await {
        let info = device
            .http_device_info
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("HTTP device info is missing"))?;

        log::info!("Using Platform API to control {device} segment");

        if let Some(brightness) = command.brightness {
            client
                .set_segment_brightness(&info, segment, brightness)
                .await?;
        } else if command.state == "OFF" {
            // Do nothing here. We used to set brightness to zero,
            // but it is problematic:
            // * Some devices don't have a 0
            // * Setting it to 0 will power up the rest of the device,
            //   so if HASS is turning off all lights in an area, the
            //   effect is that they will turn off and then immediate
            //   on again when there are segments involved
            // client.set_segment_brightness(&info, segment, 0).await?;
        }
        if let Some(color) = &command.color {
            client
                .set_segment_rgb(&info, segment, color.r, color.g, color.b)
                .await?;
        }
    } else {
        anyhow::bail!("set segments for {device}: Platform API is not available");
    }

    Ok(())
}

async fn mqtt_purge_caches(State(state): State<StateHandle>) -> anyhow::Result<()> {
    log::info!("mqtt_purge_caches");
    crate::cache::purge_cache()?;
    state
        .get_hass_client()
        .await
        .expect("have hass client")
        .register_with_hass(&state)
        .await
        .context("register_with_hass")
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
    let device = state.resolve_device_for_control(&id).await?;

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
    let client = state
        .get_hass_client()
        .await
        .expect("hass client to be present");

    log::info!("Home Assistant status changed: {status}, waiting {HASS_REGISTER_DELAY:?} before re-registering entities");
    tokio::time::sleep(HASS_REGISTER_DELAY).await;

    client.register_with_hass(&state).await?;

    Ok(())
}

async fn run_mqtt_loop(
    state: StateHandle,
    subscriber: Receiver<Event>,
    client: Client,
) -> anyhow::Result<()> {
    // Give LAN disco a chance to get current state before
    // we register with hass
    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

    async fn rebuild_router(
        client: &Client,
        state: &StateHandle,
    ) -> anyhow::Result<Arc<MqttRouter<StateHandle>>> {
        let disco_prefix = state.get_hass_disco_prefix().await;
        let mut router: MqttRouter<StateHandle> = MqttRouter::new(client.clone());

        router
            .route(format!("{disco_prefix}/status"), mqtt_homeassitant_status)
            .await?;

        router
            .route("gv2mqtt/light/:id/command", mqtt_light_command)
            .await?;
        router
            .route(
                "gv2mqtt/light/:id/command/:segment",
                mqtt_light_segment_command,
            )
            .await?;
        router
            .route("gv2mqtt/switch/:id/command/:instance", mqtt_switch_command)
            .await?;

        router.route(oneclick_topic(), mqtt_oneclick).await?;
        router.route(purge_cache_topic(), mqtt_purge_caches).await?;
        router
            .route(
                "gv2mqtt/:id/request-platform-data",
                mqtt_request_platform_data,
            )
            .await?;
        router
            .route(
                "gv2mqtt/number/:id/command/:mode_name/:work_mode",
                mqtt_number_command,
            )
            .await?;
        router
            .route("gv2mqtt/humidifier/:id/set-mode", mqtt_humidifier_set_work_mode)
            .await?;
        router
            // TODO Determine if humidifier or fan...
            .route("gv2mqtt/:id/set-work-mode", mqtt_humidifier_set_work_mode)
            .await?;
        router
            .route(
                "gv2mqtt/humidifier/:id/set-target",
                mqtt_humidifier_set_target,
            )
            .await?;
        router
            .route(
                "gv2mqtt/humidifier/:id/set-target",
                mqtt_humidifier_set_target,
            )
            .await?;
        router
            .route(
                "gv2mqtt/:id/set-temperature/:instance/:units",
                mqtt_set_temperature,
            )
            .await?;
        router
            .route("gv2mqtt/:id/set-mode-scene", mqtt_set_mode_scene)
            .await?;

        router
            .route("gv2mqtt/fan/:id/set-mode", mqtt_fan_set_work_mode)
            .await?;
        router
            .route(
                "gv2mqtt/fan/:id/set-speed",
                mqtt_fan_set_speed,
            )
            .await?;
        router
            .route(
                "gv2mqtt/fan/:id/set-oscillation",
                mqtt_fan_set_oscillation,
            )
            .await?;

        tokio::time::sleep(HASS_REGISTER_DELAY).await;
        state
            .get_hass_client()
            .await
            .expect("have hass client")
            .register_with_hass(&state)
            .await
            .context("register_with_hass")?;

        Ok(Arc::new(router))
    }

    let mut router = rebuild_router(&client, &state).await?;
    let mut need_rebuild = false;

    while let Ok(event) = subscriber.recv().await {
        match event {
            Event::Message(msg) => {
                let router = router.clone();
                let state = state.clone();
                tokio::spawn(async move {
                    if let Err(err) = router.dispatch(msg.clone(), state.clone()).await {
                        log::error!("While dispatching {msg:?}: {err:#}");
                    }
                });
            }
            Event::Disconnected(reason) => {
                log::warn!("MQTT disconnected with reason={reason}");
                need_rebuild = true;
            }
            Event::Connected(status) => {
                log::info!("MQTT connected with status={status}");
                if need_rebuild {
                    router = rebuild_router(&client, &state).await?;
                }
            }
        }
    }

    log::info!("subscriber.recv loop terminated");

    Ok(())
}

pub async fn spawn_hass_integration(
    state: StateHandle,
    args: &HassArguments,
) -> anyhow::Result<()> {
    let client = Client::with_id(
        &format!("govee2mqtt/{}", uuid::Uuid::new_v4().simple()),
        true,
    )?;

    state.set_temperature_scale(args.temperature_scale()?).await;

    let mqtt_host = args.mqtt_host()?;
    let mqtt_username = args.mqtt_username()?;
    let mqtt_password = args.mqtt_password()?;
    let mqtt_port = args.mqtt_port()?;

    client.set_last_will(availability_topic(), "offline", QoS::AtMostOnce, false)?;

    if mqtt_username.is_some() != mqtt_password.is_some() {
        log::error!(
            "MQTT username and password either both need to be set, or both need to be unset"
        );
    }
    client.set_username_and_password(mqtt_username.as_deref(), mqtt_password.as_deref())?;
    client
        .connect(
            &mqtt_host,
            mqtt_port.into(),
            Duration::from_secs(120),
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
        } else {
            log::info!("run_mqtt_loop exited. We should do something to shutdown gracefully here");
            std::process::exit(0);
        }
    });

    Ok(())
}

pub fn camel_case_to_space_separated(camel: &str) -> String {
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
