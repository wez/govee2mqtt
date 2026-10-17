use crate::hass_mqtt::climate::mqtt_set_temperature;
use crate::hass_mqtt::enumerator::{enumerate_all_entites, enumerate_entities_for_device};
use crate::hass_mqtt::humidifier::{mqtt_device_set_work_mode, mqtt_humidifier_set_target};
use crate::hass_mqtt::instance::EntityList;
use crate::hass_mqtt::number::mqtt_number_command;
use crate::hass_mqtt::select::mqtt_set_mode_scene;
use crate::lan_api::DeviceColor;
use crate::opt_env_var;
use crate::platform_api::{from_json, DeviceType};
use crate::service::device::Device as ServiceDevice;
use crate::service::fade;
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
    /// or Fahrenheit respectively.
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
    /// Transition time in seconds, as sent by the Home Assistant light
    /// schema (`light.turn_on`'s `transition` parameter). Emulated
    /// bridge-side as a paced software fade of brightness; see
    /// `service::fade`.
    transition: Option<f32>,
}

impl HassLightCommand {
    /// The fade duration requested by this command, if any. `None` for
    /// absent/zero/non-finite transitions, which behave exactly as
    /// before this field existed.
    fn fade_duration(&self) -> Option<std::time::Duration> {
        self.transition
            .filter(|secs| secs.is_finite() && *secs > 0.0)
            .map(|secs| {
                std::time::Duration::from_secs_f64(
                    (secs as f64).min(fade::MAX_FADE_DURATION.as_secs_f64()),
                )
            })
    }
}

/// HASS is sending a command to a light
async fn mqtt_light_command(
    Payload(payload): Payload<String>,
    Params(IdParameter { id }): Params<IdParameter>,
    State(state): State<StateHandle>,
) -> anyhow::Result<()> {
    // Parse the command *before* taking the control permit: the payload
    // decides whether this command takes the fade path, and the fade path
    // must be the one entry permit that schedules no reconciling poll of
    // its own (the fade's runner's final step owns the single poll a fade
    // costs; see `start_fade`).
    let command: HassLightCommand = serde_json::from_str(&payload)?;
    let takes_fade = command.state != "OFF"
        && command.brightness.is_some()
        && command.effect.is_none()
        && command.fade_duration().is_some();

    // Capture the value of a live fade *before* resolving: acquiring the
    // control permit supersedes (cancels) the fade — including a fade
    // that just finished cleanly but whose reconciling poll has not
    // landed, whose registration is kept precisely so a retarget in that
    // window continues from what was emitted rather than from stale
    // reported state. The fades registry is keyed by the canonical
    // device id; the router's label is often not that (MQTT command
    // topics use the colon-stripped form), so resolve first.
    let fading = match state.resolve_device(&id).await {
        Some(device) => state.live_fade_value(&device.id).await,
        None => None,
    };
    let mut device = state
        .resolve_device_for_control_inner(&id, !takes_fade, true)
        .await?;

    log::info!("Command for {device}: {payload}");

    // (Any in-progress fade was already superseded by taking the control
    // permit above: latest command wins, for every entry point.)

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
            match command.fade_duration() {
                // A scene/effect command wins over a fade: scenes
                // subsume brightness control on these devices.
                Some(duration) if command.effect.is_none() => {
                    match state
                        .start_fade(&device, brightness, duration, fading)
                        .await
                    {
                        // A runner took ownership: it carries this
                        // command's single reconciling poll.
                        Ok(false) => {}
                        // Collapsed to a single immediate set: it owes
                        // the same reconciling poll any plain brightness
                        // command gets (the entry permit left it unarmed
                        // for exactly this case).
                        Ok(true) => device.arm_reconciling_poll(),
                        // The unknown-start fallback set was attempted
                        // and failed: like any plain control that failed
                        // to send, the command still leaves one poll.
                        Err(err) => {
                            device.arm_reconciling_poll();
                            return Err(err).context("mqtt_light_command: state.start_fade");
                        }
                    }
                }
                _ => {
                    state
                        .device_set_brightness(&device, brightness)
                        .await
                        .context("mqtt_light_command: state.device_set_brightness")?;
                }
            }
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
                .set_segment_brightness(info, segment, brightness)
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
                .set_segment_rgb(info, segment, color.r, color.g, color.b)
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

    iot.activate_one_click(item).await
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
            .route("gv2mqtt/humidifier/:id/set-mode", mqtt_device_set_work_mode)
            .await?;
        router
            .route("gv2mqtt/:id/set-work-mode", mqtt_device_set_work_mode)
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

        tokio::time::sleep(HASS_REGISTER_DELAY).await;
        state
            .get_hass_client()
            .await
            .expect("have hass client")
            .register_with_hass(state)
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

    let mut connected = false;
    for _ in 0..30 {
        log::info!("Attempting connection to mqtt broker {mqtt_host}:{mqtt_port}...");
        match client
            .connect(
                &mqtt_host,
                mqtt_port.into(),
                Duration::from_secs(120),
                args.mqtt_bind_address.as_deref(),
            )
            .await
        {
            Ok(status) => {
                log::info!("Connected to mqtt broker {mqtt_host}:{mqtt_port}, status={status}");
                connected = true;
                break;
            }
            Err(err) => {
                log::error!("Failed to connect to mqtt broker {mqtt_host}:{mqtt_port}: {err:#}");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }

    anyhow::ensure!(
        connected,
        "Failed to connect to mqtt broker after several attempts"
    );

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
    let mut chars = camel.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut result = first.to_ascii_uppercase().to_string();
    for c in chars {
        if c.is_uppercase() {
            result.push(' ');
        }
        result.push(c);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_command_transition_parses_per_ha_light_schema() {
        // HA's light schema sends `transition` in seconds (float).
        let cmd: HassLightCommand =
            serde_json::from_str(r#"{"state":"ON","brightness":128,"transition":4.5}"#).unwrap();
        assert_eq!(cmd.brightness, Some(128));
        assert_eq!(
            cmd.fade_duration(),
            Some(std::time::Duration::from_millis(4500))
        );

        // Absent, null, zero and negative transitions all behave exactly
        // as they did before this field existed: no fade.
        for payload in [
            r#"{"state":"ON","brightness":128}"#,
            r#"{"state":"ON","brightness":128,"transition":null}"#,
            r#"{"state":"ON","brightness":128,"transition":0}"#,
            r#"{"state":"ON","brightness":128,"transition":-1}"#,
        ] {
            let cmd: HassLightCommand = serde_json::from_str(payload).unwrap();
            assert_eq!(cmd.fade_duration(), None, "payload: {payload}");
        }
    }

    #[test]
    fn light_command_transition_is_clamped_to_max_fade_duration() {
        let cmd: HassLightCommand =
            serde_json::from_str(r#"{"state":"ON","brightness":1,"transition":86400}"#).unwrap();
        assert_eq!(cmd.fade_duration(), Some(fade::MAX_FADE_DURATION));
    }

    #[test]
    fn segment_commands_still_parse_with_transition_present() {
        // Segment commands share HassLightCommand; a transition on the
        // segment path must not break deserialization.
        let cmd: HassLightCommand =
            serde_json::from_str(r#"{"state":"ON","brightness":40,"transition":2}"#).unwrap();
        assert_eq!(cmd.brightness, Some(40));
    }

    #[test]
    fn test_camel_case_to_space_separated() {
        assert_eq!(camel_case_to_space_separated("powerSwitch"), "Power Switch");
        assert_eq!(
            camel_case_to_space_separated("oscillationToggle"),
            "Oscillation Toggle"
        );
    }

    #[test]
    fn test_camel_case_chinese_no_panic() {
        assert_eq!(
            camel_case_to_space_separated("用于三灯头中的第二个"),
            "用于三灯头中的第二个"
        );
    }

    #[test]
    fn test_camel_case_empty() {
        assert_eq!(camel_case_to_space_separated(""), "");
    }

    #[test]
    fn test_camel_case_emoji() {
        assert_eq!(camel_case_to_space_separated("🔥lightMode"), "🔥light Mode");
    }
}

/// Fade lifecycle tests against the real runtime: the real MQTT command
/// handler, the real `start_fade`/`run_fade` runner, the real control
/// semaphore, and the real reconciling-poll path. Only the leaf device
/// I/O (`State::device_set_brightness`) and the poll itself are replaced
/// by `state::test_sink`, so every interaction between fade steps and
/// competing commands exercises production code.
#[cfg(test)]
mod fade_runtime_tests {
    use super::*;
    use crate::lan_api::DeviceStatus as LanDeviceStatus;
    use crate::service::state::{test_sink, State as SvcState};
    use std::time::Instant;

    const SKU: &str = "H6008";

    fn device_id(n: u8) -> String {
        format!("AA:BB:CC:DD:EE:FF:00:{n:02X}")
    }

    /// Register a platform-controlled light whose reported state is
    /// (on, brightness) and arm the simulated device for it. Each test
    /// uses its own device id, so they can run in parallel.
    async fn setup(n: u8, on: bool, brightness: u8, delays: &[u64]) -> (String, StateHandle) {
        let id = device_id(n);
        let state: StateHandle = Arc::new(SvcState::default());
        state
            .device_mut(SKU, &id)
            .await
            .set_iot_device_status(LanDeviceStatus {
                on,
                brightness,
                color: DeviceColor::default(),
                color_temperature_kelvin: 0,
            });
        test_sink::arm(&id, delays);
        (id, state)
    }

    async fn cmd(state: &StateHandle, id: &str, payload: &str) {
        mqtt_light_command(
            Payload(payload.to_string()),
            Params(IdParameter { id: id.to_string() }),
            State(state.clone()),
        )
        .await
        .unwrap();
    }

    /// A slow first step must not let the fade continue after a newer
    /// command overtook it: the newer command takes the permit the fade's
    /// in-flight step is holding, and the runner must observe its own
    /// cancellation *under that permit* before emitting the next step.
    #[tokio::test]
    async fn latest_command_wins_while_a_step_is_in_flight() {
        let (id, state) = setup(0x01, true, 0, &[1500]).await;
        let t0 = Instant::now();
        cmd(
            &state,
            &id,
            r#"{"state":"ON","brightness":100,"transition":10}"#,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(1200)).await;
        // Retarget to 5 while step 1 is still in flight (held the permit
        // for 1.5s): the queued fade steps must never be sent afterwards.
        cmd(&state, &id, r#"{"state":"ON","brightness":5}"#).await;
        tokio::time::sleep(Duration::from_millis(2500)).await;
        let log = test_sink::log_since(&id, t0);
        assert_eq!(
            log.iter().map(|(_, b)| *b).collect::<Vec<_>>(),
            vec![10, 5],
            "stale fade traffic after the newer command: {log:?}"
        );
    }

    /// The same race on a multi-threaded runtime (production flavor).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn latest_command_wins_while_a_step_is_in_flight_multithread() {
        let (id, state) = setup(0x02, true, 0, &[1500]).await;
        cmd(
            &state,
            &id,
            r#"{"state":"ON","brightness":100,"transition":10}"#,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(1200)).await;
        cmd(&state, &id, r#"{"state":"ON","brightness":5}"#).await;
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert_eq!(
            test_sink::sent(&id),
            vec![10, 5],
            "stale fade traffic after the newer command"
        );
    }

    /// A control arriving through a different entry point — this is the
    /// verbatim body the HTTP API `/api/device/{id}/brightness/{level}`
    /// path routes through, including its own `resolve_device_for_control`
    /// — supersedes the fade exactly like an MQTT command would.
    #[tokio::test]
    async fn brightness_via_other_control_paths_supersedes_fade() {
        let (id, state) = setup(0x03, true, 0, &[]).await;
        cmd(
            &state,
            &id,
            r#"{"state":"ON","brightness":100,"transition":10}"#,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        {
            let device = state.resolve_device_for_control(&id).await.unwrap();
            state.device_set_brightness(&device, 5).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert_eq!(
            test_sink::sent(&id),
            vec![10, 5],
            "fade kept writing after a control through the shared chokepoint"
        );
    }

    /// A fade of a light that is OFF must fade up from dark, not from
    /// the brightness the light had when it was last switched off (the
    /// #376 complaint: "go ON to the last value and then transition").
    #[tokio::test]
    async fn fade_of_off_light_starts_from_zero() {
        let (id, state) = setup(0x04, false, 100, &[]).await;
        cmd(
            &state,
            &id,
            r#"{"state":"ON","brightness":30,"transition":7}"#,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(8500)).await;
        assert_eq!(
            test_sink::sent(&id),
            vec![4, 9, 13, 17, 21, 26, 30],
            "fade from off did not ramp up from zero"
        );
    }

    /// A fade costs exactly one reconciling poll (scheduled for its final
    /// step) and leaves no stale registration behind.
    #[tokio::test]
    async fn fade_produces_exactly_one_reconciling_poll() {
        let (id, state) = setup(0x05, true, 0, &[]).await;
        cmd(
            &state,
            &id,
            r#"{"state":"ON","brightness":90,"transition":9}"#,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(16_500)).await;
        assert_eq!(test_sink::polls(&id), 1, "unexpected reconciling polls");
        assert!(
            !state.has_registered_fade(&id).await,
            "fade not de-registered"
        );
        assert!(!state.fade_active(&id).await, "fade still considered live");
        assert_eq!(test_sink::sent(&id).last(), Some(&90));
    }

    /// A failing step abandons the ramp but still attempts the final
    /// target once: the device must not be left mid-ramp.
    #[tokio::test]
    async fn failed_step_still_attempts_final_target() {
        let (id, state) = setup(0x06, true, 0, &[]).await;
        test_sink::arm_fails(&id, 1);
        cmd(
            &state,
            &id,
            r#"{"state":"ON","brightness":100,"transition":3}"#,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(3000)).await;
        assert_eq!(
            test_sink::sent(&id),
            vec![33, 100],
            "failed fade did not recover with the final target"
        );
        assert!(
            !state.has_registered_fade(&id).await,
            "fade not de-registered"
        );
    }

    /// A failed command does not change the poll budget: a step that
    /// fails mid-ramp, followed by a recovery attempt that also fails,
    /// still leaves exactly one reconciling poll — carried by the
    /// recovery coordinator, like any plain control that failed to
    /// send.
    #[tokio::test]
    async fn failed_step_and_failed_recovery_leave_exactly_one_reconciling_poll() {
        let (id, state) = setup(0x0E, true, 0, &[]).await;
        test_sink::arm_fails(&id, 2); // first step + recovery attempt (NACKed after recording)
        cmd(
            &state,
            &id,
            r#"{"state":"ON","brightness":100,"transition":3}"#,
        )
        .await;
        // recovery drop + 5 s poll delay + load headroom
        tokio::time::sleep(Duration::from_millis(8000)).await;
        assert_eq!(
            test_sink::sent(&id),
            vec![33, 100],
            "expected exactly one stepped attempt plus the recovery attempt, both rejected"
        );
        assert_eq!(
            test_sink::polls(&id),
            1,
            "fully-failed fade cost more than one reconciling poll"
        );
        assert!(
            !state.has_registered_fade(&id).await,
            "failed fade not de-registered"
        );
    }

    /// The same budget for the failure shape that a review round vetoed
    /// on an earlier head: when only the final step fails, its own
    /// coordinator must not fire a poll; the recovery retry is the
    /// command's last send attempt and fires the single one — never
    /// two.
    #[tokio::test]
    async fn failed_final_step_costs_exactly_one_reconciling_poll() {
        let (id, state) = setup(0x0F, true, 0, &[]).await;
        test_sink::arm_fail_at(&id, 3); // only the final step's send is rejected; recovery succeeds
        cmd(
            &state,
            &id,
            r#"{"state":"ON","brightness":100,"transition":3}"#,
        )
        .await;
        // failed final step at ~3 s, recovery drop, + 5 s poll delay + headroom
        tokio::time::sleep(Duration::from_millis(10500)).await;
        assert_eq!(
            test_sink::sent(&id),
            vec![33, 67, 100, 100],
            "expected the three ramp steps (the last rejected) plus the recovery resend"
        );
        assert_eq!(
            test_sink::polls(&id),
            1,
            "failed final step cost two reconciling polls"
        );
    }

    /// Retargeting a live fade continues from the last value the fade
    /// actually emitted (here 50), not from the device's reported state
    /// (still 0 for a platform-only light, since sends don't update it)
    /// — so the ramp never jumps backwards when, e.g., Adaptive Lighting
    /// re-applies mid-fade.
    #[tokio::test]
    async fn retargeted_fade_continues_from_the_live_ramp() {
        let (id, state) = setup(0x07, true, 0, &[]).await;
        cmd(
            &state,
            &id,
            r#"{"state":"ON","brightness":100,"transition":10}"#,
        )
        .await;
        // By t=5.4s the fade has emitted 10,20,30,40,50.
        tokio::time::sleep(Duration::from_millis(5400)).await;
        cmd(
            &state,
            &id,
            r#"{"state":"ON","brightness":60,"transition":4}"#,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(5000)).await;
        assert_eq!(
            test_sink::sent(&id),
            vec![10, 20, 30, 40, 50, 53, 55, 58, 60],
            "retarget restarted from stale state instead of continuing the ramp"
        );
    }

    /// A fade that has just COMPLETED must not retarget from stale
    /// reported state: the finished registration keeps continuity until
    /// its reconciling poll lands. Without that, an HA slider re-move
    /// (default `transition: 2`) inside the 5 s pre-poll window flashes
    /// the light down toward the stale pre-fade value before ramping —
    /// e.g. after a 0->100 fade, `{brightness:50, transition:3}` first
    /// sent 17. The continuation from the emitted 100 must instead ramp
    /// 83, 67, 50.
    #[tokio::test]
    async fn retarget_after_a_completed_fade_continues_from_the_emitted_value() {
        let (id, state) = setup(0x08, true, 0, &[]).await;
        cmd(
            &state,
            &id,
            r#"{"state":"ON","brightness":100,"transition":3}"#,
        )
        .await;
        // Fade completed at ~3 s (emitted 33,67,100). Reported state is
        // still the stale pre-fade 0, and the reconciling poll has not
        // landed (it arrives ~5 s after the final send).
        tokio::time::sleep(Duration::from_millis(3800)).await;
        assert!(
            state.has_registered_fade(&id).await,
            "a finished fade must keep its registration until its reconciling poll retires it"
        );
        cmd(
            &state,
            &id,
            r#"{"state":"ON","brightness":50,"transition":3}"#,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(3600)).await;
        assert_eq!(
            test_sink::sent(&id),
            vec![33, 67, 100, 83, 67, 50],
            "post-completion retarget flashed down toward stale state instead of continuing from the emitted 100"
        );
    }

    /// CX-2 regression (production spelling): MQTT command topics carry
    /// the colon-stripped id, so the pre-permit continuity lookup must
    /// resolve the label to the canonical device id before consulting
    /// the fade registry — a lookup keyed by the raw label misses for
    /// every id containing a colon (every LAN-capable light), silently
    /// disabling the whole continuity feature on the production path.
    #[tokio::test]
    async fn post_completion_retarget_is_continuous_through_the_production_topic_spelling() {
        let (id, state) = setup(0x0C, true, 0, &[]).await;
        let topic_id = id.replace(':', "");
        cmd(
            &state,
            &id,
            r#"{"state":"ON","brightness":100,"transition":3}"#,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(3800)).await;
        cmd(
            &state,
            &topic_id,
            r#"{"state":"ON","brightness":50,"transition":3}"#,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(3600)).await;
        assert_eq!(
            test_sink::sent(&id),
            vec![33, 67, 100, 83, 67, 50],
            "continuity was inert when the retarget arrived with the colon-stripped id spelling"
        );
    }

    /// Same production-spelling guarantee while a fade is still running:
    /// the mid-ramp value (50), not the stale reported state (0), seeds
    /// the replacement ramp.
    #[tokio::test]
    async fn mid_fade_retarget_is_continuous_through_the_production_topic_spelling() {
        let (id, state) = setup(0x0D, true, 0, &[]).await;
        let topic_id = id.replace(':', "");
        cmd(
            &state,
            &id,
            r#"{"state":"ON","brightness":100,"transition":10}"#,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(5400)).await;
        cmd(
            &state,
            &topic_id,
            r#"{"state":"ON","brightness":60,"transition":4}"#,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(5000)).await;
        assert_eq!(
            test_sink::sent(&id),
            vec![10, 20, 30, 40, 50, 53, 55, 58, 60],
            "continuity was inert for a mid-fade retarget with the production id spelling"
        );
    }

    /// CE-2 regression: a SHORT fade — HA's default slider transition is
    /// 2 s — costs the same exactly one reconciling poll a long fade
    /// does: the fade command's entry permit schedules no poll of its
    /// own at any length. (The pre-fix shape cost two polls for any fade
    /// under the 5 s poll delay: the entry poll landed after the fade
    /// was already gone and polled, then the final-step poll polled.)
    #[tokio::test]
    async fn short_fade_still_produces_exactly_one_reconciling_poll() {
        let (id, state) = setup(0x09, true, 0, &[]).await;
        cmd(
            &state,
            &id,
            r#"{"state":"ON","brightness":100,"transition":2}"#,
        )
        .await;
        // Fade completes at ~2 s; its single poll lands ~5 s later; the
        // finished registration must be retired by that poll.
        tokio::time::sleep(Duration::from_millis(8500)).await;
        assert_eq!(
            test_sink::polls(&id),
            1,
            "short fade cost more than one reconciling poll"
        );
        assert!(
            !state.has_registered_fade(&id).await,
            "finished fade not retired by its reconciling poll"
        );
        assert_eq!(test_sink::sent(&id).last(), Some(&100));
    }

    /// The claimed scene/effect precedence carries a regression test:
    /// `transition` alongside an `effect` must produce a single
    /// immediate brightness set — never a ramp. (The simulated device
    /// has no scene capability, so the command errors at the scene step;
    /// what matters is the brightness traffic emitted before that.)
    #[tokio::test]
    async fn effect_with_transition_sets_immediately_and_does_not_ramp() {
        let (id, state) = setup(0x0A, true, 0, &[]).await;
        let _ = mqtt_light_command(
            Payload(
                r#"{"state":"ON","brightness":40,"transition":3,"effect":"Sunset"}"#.to_string(),
            ),
            Params(IdParameter { id: id.clone() }),
            State(state.clone()),
        )
        .await;
        tokio::time::sleep(Duration::from_millis(3800)).await;
        assert_eq!(
            test_sink::sent(&id),
            vec![40],
            "transition+effect ramped instead of a single immediate set"
        );
    }
}
