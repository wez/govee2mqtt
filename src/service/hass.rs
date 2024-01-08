use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::hass_mqtt::button::ButtonConfig;
use crate::hass_mqtt::humidifier::HumidifierConfig;
use crate::hass_mqtt::instance::EntityList;
use crate::hass_mqtt::light::LightConfig;
use crate::hass_mqtt::scene::SceneConfig;
use crate::hass_mqtt::sensor::GlobalFixedDiagnostic;
use crate::hass_mqtt::switch::SwitchConfig;
use crate::lan_api::DeviceColor;
use crate::opt_env_var;
use crate::platform_api::{
    from_json, DeviceCapability, DeviceCapabilityKind, DeviceParameters, DeviceType, EnumOption,
};
use crate::service::device::Device as ServiceDevice;

use crate::service::state::{State as ServiceState, StateHandle};
use crate::version_info::govee_version;
use anyhow::Context;
use async_channel::Receiver;
use mosquitto_rs::router::{MqttRouter, Params, Payload, State};
use mosquitto_rs::{Client, Event, QoS};
use serde::{Deserialize, Serialize};

use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

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

enum GlobalConfig {
    Scene(SceneConfig),
    Button(ButtonConfig),
}

impl GlobalConfig {
    async fn publish(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        match self {
            Self::Scene(s) => s.publish(state, client).await,
            Self::Button(s) => s.publish(state, client).await,
        }
    }

    pub async fn notify_state(&self, client: &HassClient, value: &str) -> anyhow::Result<()> {
        match self {
            Self::Scene(s) => s.notify_state(client, value).await,
            Self::Button(_) => {
                // Buttons have no state
                Ok(())
            }
        }
    }
}

enum Config {
    Light(LightConfig),
    Switch(SwitchConfig),
    #[allow(dead_code)]
    Button(ButtonConfig),
    Humidifier(HumidifierConfig),
}

impl Config {
    async fn publish(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        match self {
            Self::Light(l) => l.publish(state, client).await,
            Self::Switch(s) => s.publish(state, client).await,
            Self::Button(s) => s.publish(state, client).await,
            Self::Humidifier(s) => s.publish(state, client).await,
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
            Self::Humidifier(s) => s.notify_state(device, client).await,
            Self::Button(_) => {
                // Buttons have no state
                Ok(())
            }
        }
    }

    async fn for_work_mode<'a>(
        _d: &'a ServiceDevice,
        _state: &ServiceState,
        cap: &DeviceCapability,
        _configs: &mut Vec<(&'a ServiceDevice, Self)>,
    ) -> anyhow::Result<()> {
        #[derive(Deserialize, PartialOrd, Ord, PartialEq, Eq)]
        struct NumericOption {
            value: i64,
        }

        fn is_contiguous_range(opt_range: &mut Vec<NumericOption>) -> Option<Range<i64>> {
            if opt_range.is_empty() {
                return None;
            }
            opt_range.sort();

            let min = opt_range.first().map(|r| r.value).expect("not empty");
            let max = opt_range.last().map(|r| r.value).expect("not empty");

            let mut expect = min;
            for item in opt_range {
                if item.value != expect {
                    return None;
                }
                expect = expect + 1;
            }

            Some(min..max + 1)
        }

        fn extract_contiguous_range(opt: &EnumOption) -> Option<Range<i64>> {
            let extra_opts = opt.extras.get("options")?;

            let mut opt_range =
                serde_json::from_value::<Vec<NumericOption>>(extra_opts.clone()).ok()?;

            is_contiguous_range(&mut opt_range)
        }

        if let Some(wm) = cap.struct_field_by_name("modeValue") {
            match &wm.field_type {
                DeviceParameters::Enum { options } => {
                    for opt in options {
                        if let Some(_range) = extract_contiguous_range(opt) {
                            log::warn!("should show this as a number slider");
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    async fn for_device<'a>(
        d: &'a ServiceDevice,
        state: &ServiceState,
        configs: &mut Vec<(&'a ServiceDevice, Self)>,
    ) -> anyhow::Result<()> {
        if !d.is_controllable() {
            return Ok(());
        }

        if d.supports_rgb() || d.get_color_temperature_range().is_some() || d.supports_brightness()
        {
            configs.push((
                d,
                Config::Light(LightConfig::for_device(&d, state, None).await?),
            ));
        }

        if d.device_type() == DeviceType::Humidifier {
            configs.push((
                d,
                Config::Humidifier(HumidifierConfig::for_device(&d, state).await?),
            ));
        }

        if let Some(info) = &d.http_device_info {
            for cap in &info.capabilities {
                match &cap.kind {
                    DeviceCapabilityKind::Toggle | DeviceCapabilityKind::OnOff => {
                        configs.push((d, Config::Switch(SwitchConfig::for_device(&d, cap).await?)));
                    }
                    DeviceCapabilityKind::ColorSetting
                    | DeviceCapabilityKind::SegmentColorSetting
                    | DeviceCapabilityKind::MusicSetting
                    | DeviceCapabilityKind::Event
                    | DeviceCapabilityKind::DynamicScene => {}

                    DeviceCapabilityKind::Range if cap.instance == "brightness" => {}
                    DeviceCapabilityKind::Range if cap.instance == "humidity" => {}
                    DeviceCapabilityKind::WorkMode => {
                        Self::for_work_mode(d, state, cap, configs).await?;
                    }

                    kind => {
                        log::warn!(
                            "Do something about {kind:?} {} for {d} {cap:?}",
                            cap.instance
                        );
                    }
                }
            }

            if let Some(segments) = info.supports_segmented_rgb() {
                for n in segments {
                    configs.push((
                        d,
                        Config::Light(LightConfig::for_device(&d, state, Some(n)).await?),
                    ));
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
        let mut entities = EntityList::new();
        entities.add(GlobalFixedDiagnostic::new("Version", govee_version()));

        let mut globals = vec![(
            GlobalConfig::Button(ButtonConfig::global_button(
                "Purge Caches",
                purge_cache_topic(),
            )),
            "".to_string(),
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

        // Register the configs
        log::trace!("register_with_hass: register entities");
        entities.publish_config(state, self).await?;
        for (s, _) in &globals {
            s.publish(state, self)
                .await
                .context("create hass config for a global item")?;
        }
        for (d, c) in &configs {
            c.publish(state, self)
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
        entities.notify_state(self).await.context("notify_state")?;
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
        if c == ':' || c == ' ' {
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
        state
            .device_light_power_on(&device, false)
            .await
            .context("mqtt_light_command: state.device_power_on")?;
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
            state
                .device_light_power_on(&device, true)
                .await
                .context("mqtt_light_command: state.device_power_on")?;
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
    let device = state
        .resolve_device(&id)
        .await
        .ok_or_else(|| anyhow::anyhow!("device '{id}' not found"))?;
    let segment: u32 = segment.parse()?;

    let command: HassLightCommand = from_json(&payload)?;
    log::info!("Command for {device} segment {segment}: {payload}");

    if let Some(client) = state.get_platform_client().await {
        let info = device
            .http_device_info
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("HTTP device info is missing"))?;

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
        anyhow::bail!("cannot set segments: platform API is not available");
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
    let client = Client::with_auto_id()?;

    let mqtt_host = args.mqtt_host()?;
    let mqtt_username = args.mqtt_username()?;
    let mqtt_password = args.mqtt_password()?;
    let mqtt_port = args.mqtt_port()?;

    client.set_last_will(availability_topic(), "offline", QoS::AtMostOnce, true)?;

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

pub fn instance_from_topic(topic: &str) -> Option<&str> {
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
