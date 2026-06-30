//! Subscribes to Govee's cloud MQTT broker to receive device events such as
//! ice-maker-full, water-empty and presence detection.
//!
//! This replaces the need to run a separate Mosquitto bridge as described in
//! <https://github.com/wez/govee2mqtt/issues/343>. The broker, authentication
//! and topic are documented at
//! <https://developer.govee.com/reference/subscribe-device-event>:
//!
//! * Host: `mqtt.openapi.govee.com`, port 8883 (TLS)
//! * Username and password are both the Govee API key
//! * Topic is `GA/<api-key>`
//!
//! The connection is supervised so that it reconnects robustly: mosquitto's
//! built-in auto-reconnect handles transient drops (re-subscribing on each
//! reconnect), while an outer supervisor rebuilds the client from scratch,
//! with exponential backoff, if the connection cannot be (re)established or
//! is closed cleanly by the broker.

use crate::hass_mqtt::event::{event_type_label, parse_event_capability, value_key};
use crate::opt_env_var;
use crate::platform_api::{from_json, DeviceCapabilityKind};
use crate::service::device::Device;
use crate::service::hass::event_state_topic;
use crate::service::state::StateHandle;
use anyhow::Context;
use async_channel::Receiver;
use chrono::Utc;
use mosquitto_rs::{Client, Event, QoS};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::{sleep, timeout};

const PLATFORM_MQTT_HOST: &str = "mqtt.openapi.govee.com";
const PLATFORM_MQTT_PORT: i32 = 8883;

/// Minimum interval between platform-API polls that are triggered by an
/// incoming event message for a given device. This protects the (rate
/// limited) Govee platform API from being hammered by chatty devices.
const POLL_COOLDOWN: Duration = Duration::from_secs(60);

const INITIAL_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(300);
/// If a connection stayed up for at least this long before dropping, treat it
/// as healthy and reset the reconnect backoff.
const HEALTHY_CONNECTION: Duration = Duration::from_secs(60);

type PollCooldown = Arc<Mutex<HashMap<String, Instant>>>;

/// Where to find the trust roots used to validate the TLS connection to the
/// Govee MQTT broker.
#[derive(Debug, Clone)]
pub enum CaLocation {
    /// A single PEM file containing one or more CA certificates.
    File(PathBuf),
    /// A directory of PEM-encoded CA certificates (OpenSSL c_rehash layout).
    Path(PathBuf),
}

impl CaLocation {
    fn configure(&self, client: &Client) -> anyhow::Result<()> {
        match self {
            CaLocation::File(file) => client.configure_tls(
                Some(file),
                None::<&Path>,
                None::<&Path>,
                None::<&Path>,
                None,
            ),
            CaLocation::Path(dir) => {
                client.configure_tls(None::<&Path>, Some(dir), None::<&Path>, None::<&Path>, None)
            }
        }
        .context("configure_tls")
    }
}

fn classify_ca(path: PathBuf) -> anyhow::Result<CaLocation> {
    if path.is_dir() {
        Ok(CaLocation::Path(path))
    } else if path.exists() {
        Ok(CaLocation::File(path))
    } else {
        anyhow::bail!("configured platform MQTT CA location {path:?} does not exist");
    }
}

/// Resolve the CA trust store to use for the platform MQTT TLS connection.
/// An explicit override (CLI argument or `GOVEE_MQTT_PLATFORM_CA`) wins;
/// otherwise we autodetect the system CA bundle from common locations.
pub fn resolve_ca_location(override_path: Option<PathBuf>) -> anyhow::Result<CaLocation> {
    if let Some(path) = override_path {
        return classify_ca(path);
    }
    if let Some(path) = opt_env_var::<PathBuf>("GOVEE_MQTT_PLATFORM_CA")? {
        return classify_ca(path);
    }

    // Common system CA bundle file locations across distributions.
    const CANDIDATE_FILES: &[&str] = &[
        "/etc/ssl/certs/ca-certificates.crt", // Debian/Ubuntu/distroless
        "/etc/pki/tls/certs/ca-bundle.crt",   // RHEL/Fedora/CentOS
        "/etc/ssl/cert.pem",                  // Alpine/macOS/BSD
    ];
    for file in CANDIDATE_FILES {
        let path = Path::new(file);
        if path.exists() {
            return Ok(CaLocation::File(path.to_path_buf()));
        }
    }

    // Fall back to a directory of hashed certs.
    let dir = Path::new("/etc/ssl/certs");
    if dir.is_dir() {
        return Ok(CaLocation::Path(dir.to_path_buf()));
    }

    anyhow::bail!(
        "could not locate a system CA bundle for the platform MQTT TLS connection; \
         set GOVEE_MQTT_PLATFORM_CA (or --platform-mqtt-ca) to point at your CA \
         certificate file or directory"
    );
}

/// A device event/state message published by the Govee platform MQTT broker.
#[derive(Deserialize, Debug)]
struct PlatformMessage {
    device: Option<String>,
    #[serde(default)]
    capabilities: Vec<PlatformCapability>,
}

#[derive(Deserialize, Debug)]
struct PlatformCapability {
    #[serde(rename = "type")]
    kind: DeviceCapabilityKind,
    instance: String,
    /// For event capabilities this is an array of state entries. We keep it
    /// as a raw value so that messages carrying other (object-shaped) state
    /// don't fail to parse.
    #[serde(default)]
    state: JsonValue,
}

#[derive(Deserialize, Debug, Clone)]
struct PlatformEventState {
    #[serde(default)]
    name: String,
    #[serde(default)]
    value: JsonValue,
    #[serde(default)]
    message: Option<String>,
}

/// Start the supervised platform MQTT subscription as a background task.
pub fn start_platform_mqtt_client(api_key: String, state: StateHandle, ca: CaLocation) {
    tokio::spawn(async move {
        let cooldown: PollCooldown = Arc::new(Mutex::new(HashMap::new()));
        let mut backoff = INITIAL_BACKOFF;

        loop {
            let outcome = run_platform_mqtt_once(&state, &api_key, &ca, &cooldown).await;

            // If we had a healthy, long-lived connection (measured from after
            // the connection was actually established, not including connect
            // time), reset the backoff so a single later drop reconnects
            // promptly. Reset before logging/sleeping so the logged value and
            // the actual delay agree.
            if matches!(&outcome, Ok(uptime) if *uptime >= HEALTHY_CONNECTION) {
                backoff = INITIAL_BACKOFF;
            }

            match outcome {
                Ok(uptime) => {
                    log::warn!(
                        "platform MQTT: connection closed after {uptime:?}; \
                         reconnecting in {backoff:?}"
                    );
                }
                Err(err) => {
                    log::error!("platform MQTT: {err:#}; retrying in {backoff:?}");
                }
            }

            sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    });
}

/// Build a client and establish a fresh TLS+authenticated connection to the
/// Govee platform MQTT broker. The returned client is connected but has not
/// yet subscribed to any topics.
async fn connect_platform_client(api_key: &str, ca: &CaLocation) -> anyhow::Result<Client> {
    let client = Client::with_id(
        &format!("gv2mqtt-platform-{}", uuid::Uuid::new_v4().simple()),
        true,
    )
    .context("creating platform MQTT client")?;

    // Back off exponentially (1s..60s) for mosquitto's own auto-reconnect
    // between the explicit supervisor rebuilds.
    client
        .set_reconnect_delay(Duration::from_secs(1), Duration::from_secs(60), true)
        .ok();

    ca.configure(&client)
        .context("configuring platform MQTT TLS")?;

    client
        .set_username_and_password(Some(api_key), Some(api_key))
        .context("setting platform MQTT credentials")?;

    log::info!("platform MQTT: connecting to {PLATFORM_MQTT_HOST}:{PLATFORM_MQTT_PORT}");
    let status = timeout(
        Duration::from_secs(60),
        client.connect(
            PLATFORM_MQTT_HOST,
            PLATFORM_MQTT_PORT,
            Duration::from_secs(90),
            None,
        ),
    )
    .await
    .with_context(|| format!("timeout connecting to platform MQTT {PLATFORM_MQTT_HOST}"))?
    .with_context(|| format!("failed to connect to platform MQTT {PLATFORM_MQTT_HOST}"))?;
    log::info!("platform MQTT: connected, status={status}");

    Ok(client)
}

/// Connect and run the subscriber loop until the connection is closed.
/// On success, returns how long the connection was up *after* connecting,
/// so the supervisor can distinguish a healthy session from connect-then-drop
/// flapping.
async fn run_platform_mqtt_once(
    state: &StateHandle,
    api_key: &str,
    ca: &CaLocation,
    cooldown: &PollCooldown,
) -> anyhow::Result<Duration> {
    let client = connect_platform_client(api_key, ca).await?;
    let connected_at = Instant::now();

    let subscriber = client
        .subscriber()
        .expect("first and only call to subscriber()");

    run_platform_subscriber(
        subscriber,
        state.clone(),
        client,
        api_key.to_string(),
        cooldown.clone(),
    )
    .await?;

    Ok(connected_at.elapsed())
}

async fn run_platform_subscriber(
    subscriber: Receiver<Event>,
    state: StateHandle,
    client: Client,
    api_key: String,
    cooldown: PollCooldown,
) -> anyhow::Result<()> {
    while let Ok(event) = subscriber.recv().await {
        match event {
            Event::Connected(status) => {
                log::info!("platform MQTT: (re)connected with status {status}");

                let topic = format!("GA/{api_key}");
                if let Err(err) = client.subscribe(&topic, QoS::AtMostOnce).await {
                    log::error!("platform MQTT: failed to subscribe to {topic}: {err:#}");
                }
                // The official documentation is internally inconsistent about
                // whether the topic is `GA/<key>` or the bare `<key>`.
                // Empirically it is `GA/<key>`, but subscribing to both is
                // cheap insurance against either behavior.
                if let Err(err) = client.subscribe(&api_key, QoS::AtMostOnce).await {
                    log::debug!("platform MQTT: failed to subscribe to bare-key topic: {err:#}");
                }
            }
            Event::Message(msg) => {
                log::trace!(
                    "platform MQTT: {} -> {}",
                    msg.topic,
                    String::from_utf8_lossy(&msg.payload)
                );

                // Publish events inline so that ordering is preserved (this
                // matters for eg: presence appeared-then-absent). The publish
                // path is fast; only the slow, rate-limited poll fallback is
                // spawned off the loop (see handle_platform_message).
                handle_platform_message(&state, &msg.payload, &cooldown).await;
            }
            Event::Disconnected(reason) => {
                if reason.is_unexpected_disconnect() {
                    log::warn!(
                        "platform MQTT: disconnected with reason {reason}; \
                         will attempt to auto-reconnect"
                    );
                } else {
                    log::info!(
                        "platform MQTT: broker closed the connection cleanly \
                         (reason {reason}); rebuilding"
                    );
                }
            }
        }
    }

    // The channel closed: mosquitto will not auto-reconnect in this case
    // (eg: a clean, broker-initiated disconnect). Returning lets the
    // supervisor rebuild the client from scratch.
    Ok(())
}

async fn handle_platform_message(state: &StateHandle, payload: &[u8], cooldown: &PollCooldown) {
    let message: PlatformMessage = match from_json(payload) {
        Ok(message) => message,
        Err(err) => {
            log::warn!("platform MQTT: failed to parse message: {err:#}");
            return;
        }
    };

    let Some(device_id) = message.device.as_deref() else {
        log::trace!("platform MQTT: message has no device id; ignoring");
        return;
    };

    let Some(device) = state.device_by_id(device_id).await else {
        log::debug!("platform MQTT: event for unknown device {device_id}; ignoring");
        return;
    };

    let hass = state.get_hass_client().await;
    let mut need_poll = false;

    for cap in &message.capabilities {
        if cap.kind != DeviceCapabilityKind::Event {
            // A non-event state update. The payload shape differs from the
            // platform API state, so rather than try to apply it directly we
            // refresh via a (debounced) poll, per the approach suggested in
            // <https://github.com/wez/govee2mqtt/issues/343>.
            need_poll = true;
            continue;
        }

        let info = device
            .get_capability_by_instance(&cap.instance)
            .filter(|c| c.kind == DeviceCapabilityKind::Event)
            .and_then(parse_event_capability);

        let entries: Vec<PlatformEventState> =
            serde_json::from_value(cap.state.clone()).unwrap_or_default();

        match (&hass, &info) {
            (Some(hass), Some(info)) => {
                let topic = event_state_topic(&device, &cap.instance);
                for entry in &entries {
                    let event_type = info
                        .label_by_value
                        .get(&value_key(&entry.value))
                        .cloned()
                        .unwrap_or_else(|| event_type_label(&entry.name, entry.message.as_deref()));

                    if !info.event_types.contains(&event_type) {
                        log::warn!(
                            "platform MQTT: event_type {event_type:?} for {device} {inst} \
                             is not in the advertised list {types:?}; Home Assistant will \
                             ignore it",
                            inst = cap.instance,
                            types = info.event_types
                        );
                    }

                    log::info!(
                        "platform MQTT: {device} {inst} event: {event_type}",
                        inst = cap.instance
                    );

                    let payload = json!({
                        "event_type": event_type,
                        "name": entry.name,
                        "value": entry.value,
                        "message": entry.message,
                    });

                    if let Err(err) = hass.publish_obj(&topic, &payload).await {
                        log::error!("platform MQTT: failed to publish event for {device}: {err:#}");
                    }
                }
            }
            _ => {
                // Either HASS isn't connected yet, or we have no metadata to
                // map this event onto an entity. Fall back to polling.
                need_poll = true;
            }
        }
    }

    if need_poll {
        // Offload the (potentially slow, rate-limited) poll so it cannot block
        // the subscriber loop or delay/ reorder subsequent event publishes.
        let state = state.clone();
        let device = device.clone();
        let cooldown = cooldown.clone();
        tokio::spawn(async move {
            maybe_poll_device(&state, &device, &cooldown).await;
        });
    }
}

async fn maybe_poll_device(state: &StateHandle, device: &Device, cooldown: &PollCooldown) {
    // This poll is only ever a fallback (unmappable event, or HASS not yet
    // ready). The Govee platform API is daily-quota limited, so only poll when
    // the device's state is actually stale; an event should not force a poll on
    // every message. This keeps the worst case in line with the normal
    // periodic polling cadence rather than ~15x hotter.
    if let Some(state) = device.device_state() {
        if Utc::now() - state.updated < device.preferred_poll_interval() {
            log::trace!("platform MQTT: skipping poll for {device}; state is still fresh");
            return;
        }
    }

    // A short cooldown additionally guards against a burst of concurrent polls
    // for the same device while a poll is already in flight (last_polled has
    // not been updated yet).
    {
        let mut map = cooldown.lock();
        let now = Instant::now();
        if let Some(last) = map.get(&device.id) {
            if now.duration_since(*last) < POLL_COOLDOWN {
                log::trace!("platform MQTT: skipping poll for {device}; within cooldown");
                return;
            }
        }
        map.insert(device.id.clone(), now);
    }

    log::info!("platform MQTT: polling {device} to refresh state after event");
    if let Err(err) = state.poll_platform_api(device).await {
        log::error!("platform MQTT: poll for {device} failed: {err:#}");
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Live connectivity smoke test against Govee's cloud MQTT broker.
    /// Ignored by default since it needs network access and a real key.
    /// Run with: `GOVEE_API_KEY=<key> cargo test --
    /// service::platform_mqtt::test::live_connect_smoke --ignored --nocapture`
    #[tokio::test]
    #[ignore = "requires network access and GOVEE_API_KEY"]
    async fn live_connect_smoke() {
        let api_key = std::env::var("GOVEE_API_KEY")
            .expect("set GOVEE_API_KEY in the environment to run this test");
        let ca = resolve_ca_location(None).expect("a system CA bundle to be available");

        let client = connect_platform_client(&api_key, &ca)
            .await
            .expect("connect to the Govee platform MQTT broker");
        let subscriber = client.subscriber().expect("subscriber channel");

        let topic = format!("GA/{api_key}");
        client
            .subscribe(&topic, QoS::AtMostOnce)
            .await
            .expect("subscribe to GA/<key>");
        eprintln!("connected + subscribed to GA/<redacted> successfully");

        // Drain any retained/immediate messages for a few seconds, if any.
        let _ = timeout(Duration::from_secs(3), async {
            while let Ok(event) = subscriber.recv().await {
                match event {
                    Event::Message(m) => {
                        eprintln!(
                            "message on {}: {}",
                            m.topic,
                            String::from_utf8_lossy(&m.payload)
                        );
                    }
                    Event::Connected(s) => eprintln!("connected: {s}"),
                    Event::Disconnected(r) => eprintln!("disconnected: {r}"),
                }
            }
        })
        .await;
    }

    #[test]
    fn parse_ice_maker_out_of_water() {
        let payload = br#"{
            "sku": "H7172",
            "device": "41:DA:D4:AD:FC:46:00:64",
            "deviceName": "H7172",
            "capabilities": [
                {
                    "type": "devices.capabilities.event",
                    "instance": "lackWaterEvent",
                    "state": [
                        {"name": "lack", "value": 1, "message": "Lack of Water"}
                    ]
                }
            ]
        }"#;

        let message: PlatformMessage = from_json(payload).unwrap();
        assert_eq!(message.device.as_deref(), Some("41:DA:D4:AD:FC:46:00:64"));
        assert_eq!(message.capabilities.len(), 1);
        let cap = &message.capabilities[0];
        assert_eq!(cap.kind, DeviceCapabilityKind::Event);
        assert_eq!(cap.instance, "lackWaterEvent");

        let entries: Vec<PlatformEventState> = serde_json::from_value(cap.state.clone()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "lack");
        assert_eq!(entries[0].value, json!(1));
        assert_eq!(entries[0].message.as_deref(), Some("Lack of Water"));
    }

    #[test]
    fn non_array_state_does_not_fail_message_parse() {
        // A hypothetical state-style push where `state` is an object rather
        // than an array must still parse at the message level.
        let payload = br#"{
            "sku": "H7172",
            "device": "AA:BB",
            "capabilities": [
                {
                    "type": "devices.capabilities.online",
                    "instance": "online",
                    "state": {"value": true}
                }
            ]
        }"#;

        let message: PlatformMessage = from_json(payload).unwrap();
        assert_eq!(message.capabilities.len(), 1);
        // And interpreting the object state as event entries yields nothing,
        // rather than panicking.
        let entries: Vec<PlatformEventState> =
            serde_json::from_value(message.capabilities[0].state.clone()).unwrap_or_default();
        assert!(entries.is_empty());
    }

    #[test]
    fn presence_event_parses() {
        let payload = br#"{
            "sku": "H5127",
            "device": "06:30:60:74:F4:45:B9:DA",
            "deviceName": "Presence Sensor",
            "capabilities": [
                {
                    "type": "devices.capabilities.event",
                    "instance": "bodyAppearedEvent",
                    "state": [ {"name": "Presence", "value": 1} ]
                }
            ]
        }"#;

        let message: PlatformMessage = from_json(payload).unwrap();
        let entries: Vec<PlatformEventState> =
            serde_json::from_value(message.capabilities[0].state.clone()).unwrap();
        assert_eq!(entries[0].name, "Presence");
        assert_eq!(entries[0].message, None);
    }
}
