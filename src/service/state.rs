use crate::ble::{Base64HexBytes, SetHumidifierMode, SetHumidifierNightlightParams};
use crate::lan_api::{Client as LanClient, DeviceStatus as LanDeviceStatus, LanDevice};
use crate::platform_api::{DeviceCapability, DeviceType, GoveeApiClient};
use crate::service::coordinator::Coordinator;
use crate::service::device::Device;
use crate::service::fade::{self, FadeStep, FadeToken};
use crate::service::hass::{topic_safe_id, HassClient};
use crate::service::iot::IotClient;
use crate::temperature::{TemperatureScale, TemperatureValue};
use crate::undoc_api::GoveeUndocumentedApi;
use anyhow::Context;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{MappedMutexGuard, Mutex, MutexGuard, Semaphore};
use tokio::time::{sleep, sleep_until, Duration};

#[derive(Default)]
pub struct State {
    devices_by_id: Mutex<HashMap<String, Device>>,
    semaphore_by_id: Mutex<HashMap<String, Arc<Semaphore>>>,
    lan_client: Mutex<Option<LanClient>>,
    platform_client: Mutex<Option<GoveeApiClient>>,
    undoc_client: Mutex<Option<GoveeUndocumentedApi>>,
    iot_client: Mutex<Option<IotClient>>,
    hass_client: Mutex<Option<HassClient>>,
    hass_discovery_prefix: Mutex<String>,
    temperature_scale: Mutex<TemperatureScale>,
    /// The live fade per device, if any
    fades: Mutex<HashMap<String, Arc<FadeToken>>>,
}

pub type StateHandle = Arc<State>;

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn set_temperature_scale(&self, scale: TemperatureScale) {
        *self.temperature_scale.lock().await = scale;
    }

    pub async fn get_temperature_scale(&self) -> TemperatureScale {
        *self.temperature_scale.lock().await
    }

    pub async fn set_hass_disco_prefix(&self, prefix: String) {
        *self.hass_discovery_prefix.lock().await = prefix;
    }

    /// The last brightness emitted by a still-live fade for this device,
    /// if one is in progress. Control handlers read this *before*
    /// acquiring the control permit — acquiring it supersedes the fade —
    /// so that a retarget can continue the ramp from the value the device
    /// is actually being told to show rather than from possibly stale
    /// reported state; see `start_fade`. A cleanly finished fade keeps
    /// its registration (and therefore this continuity) until its
    /// reconciling poll lands and retires it, because until that poll the
    /// reported state is still the pre-fade value.
    pub async fn live_fade_value(&self, id: &str) -> Option<u8> {
        self.fades
            .lock()
            .await
            .get(id)
            .filter(|token| !token.is_superseded())
            .map(|token| token.emitted())
    }

    /// Test-only: whether a fade registration (live or finishing) exists
    /// for the device id.
    #[cfg(test)]
    pub(crate) async fn has_registered_fade(&self, id: &str) -> bool {
        self.fades.lock().await.contains_key(id)
    }

    /// True while a fade is in progress for the given device id
    /// (live, not merely finished). Test-visible assertion helper:
    /// production paths key off the token registry inside
    /// `poll_after_control` / `live_fade_value` / `cancel_fade` instead.
    #[cfg(test)]
    pub async fn fade_active(&self, id: &str) -> bool {
        self.fades
            .lock()
            .await
            .get(id)
            .map(|token| !token.is_superseded() && !token.is_finished())
            .unwrap_or(false)
    }

    /// Supersede any fade in progress for the device. The runner stops
    /// at its next wake-up without emitting any further steps, so the
    /// latest command always wins and stale targets are never queued.
    pub async fn cancel_fade(self: &Arc<Self>, id: &str) {
        if let Some(token) = self.fades.lock().await.remove(id) {
            token.mark_superseded();
            log::debug!("Superseding in-progress fade for device {id}");
        }
    }

    /// Forget our own registration, but only if it is still the current
    /// one (a newer fade may already have replaced it).
    async fn forget_fade(&self, id: &str, token: &Arc<FadeToken>) {
        let mut fades = self.fades.lock().await;
        if fades
            .get(id)
            .map(|current| Arc::ptr_eq(current, token))
            .unwrap_or(false)
        {
            fades.remove(id);
        }
    }

    /// Start a software fade of this device's brightness toward `target`
    /// over `duration`, replacing any fade already in progress.
    ///
    /// The ramp starts from `continue_from` when given (the value of the
    /// fade that a retarget supersedes, captured by the caller before it
    /// acquired the control permit; this also covers a fade that finished
    /// cleanly but whose reconciling poll has not landed yet, whose
    /// registration is deliberately kept until then), otherwise from the
    /// device's reported brightness — or from 0 when the light is off, so
    /// that a fade on an off light fades up from dark instead of flashing
    /// on at whatever brightness it was last commanded to before being
    /// switched off. Falls back to a single immediate brightness command
    /// when no fade can meaningfully happen (see `fade::plan_fade`).
    ///
    /// Returns `true` when the fade collapsed to a single immediate
    /// brightness set (no ramp, no runner): the caller's entry permit was
    /// taken without scheduling a poll (the fade path owns its one
    /// reconciling poll through the runner's final step), so on collapse
    /// the caller owes the reconciling poll itself — see
    /// `Coordinator::arm_reconciling_poll`.
    pub async fn start_fade(
        self: &Arc<Self>,
        device: &Device,
        target: u8,
        duration: Duration,
        continue_from: Option<u8>,
    ) -> anyhow::Result<bool> {
        let start = match continue_from
            .or(self.live_fade_value(&device.id).await)
            .or_else(|| {
                device.device_state().map(|state| {
                    if state.light_on.unwrap_or(state.on) {
                        state.brightness
                    } else {
                        0
                    }
                })
            }) {
            Some(start) => start,
            None => {
                // We don't know the starting brightness, so there is no
                // ramp we could compute; preserve the single-command
                // behavior instead of guessing a start value.
                log::debug!("No known brightness for {device}; sending direct set instead of fade");
                return self
                    .device_set_brightness(device, target)
                    .await
                    .map(|()| true);
            }
        };

        let steps = fade::plan_fade(start, target, duration);
        log::debug!(
            "Fading {device} brightness {start} -> {target} over {duration:?} in {} step(s)",
            steps.len()
        );

        if steps.len() == 1 {
            return self
                .device_set_brightness(device, steps[0].brightness)
                .await
                .map(|()| true);
        }

        // Replace (and thereby supersede) any live fade under a single
        // lock, so there is no window in which neither token is current.
        let token = FadeToken::new(start);
        {
            let mut fades = self.fades.lock().await;
            if let Some(old) = fades.insert(device.id.clone(), Arc::clone(&token)) {
                old.mark_superseded();
            }
        }

        let state = Arc::clone(self);
        let device_id = device.id.clone();
        tokio::spawn(async move {
            state.run_fade(device_id, token, steps).await;
        });

        Ok(false)
    }

    async fn run_fade(
        self: &Arc<Self>,
        device_id: String,
        token: Arc<FadeToken>,
        steps: Vec<FadeStep>,
    ) {
        let started = tokio::time::Instant::now();
        let target = steps.last().map(|step| step.brightness).unwrap_or(0);
        let mut failed = false;
        // Safety net: if this runner ever dies before its terminal
        // cleanup (a panic unwinding out of the loop), the token must not
        // linger as "live" and hold the device's state polling shut
        // forever. Dropped un-completed, it marks the fade superseded,
        // which every reader treats as "no fade".
        let mut abort_guard = FadeAbortGuard {
            token: Arc::clone(&token),
            completed: false,
        };

        for (index, step) in steps.iter().enumerate() {
            let is_last = index + 1 == steps.len();
            sleep_until(started + step.at).await;

            if token.is_superseded() {
                // Superseded by a newer command: it owns the device now.
                return;
            }

            if self.device_by_id(&device_id).await.is_none() {
                token.mark_superseded();
                self.forget_fade(&device_id, &token).await;
                return;
            }

            let outcome = {
                // Take the control permit WITHOUT scheduling a poll
                // (only the final step's coordinator triggers the one
                // reconciling poll a fade costs) and WITHOUT the
                // acquire-time supersession that external control paths
                // use — that would cancel *this* fade.
                // Exactly one reconciling poll per fade command, no
                // matter how the command ends: never scheduled on
                // acquisition, armed explicitly on the coordinator that
                // carries the command's last send attempt (final step on
                // success, recovery attempt otherwise).
                let mut control = match self
                    .resolve_device_for_control_inner(&device_id, false, false)
                    .await
                {
                    Ok(control) => control,
                    Err(err) => {
                        log::error!("fade step for {device_id}: {err:#}");
                        // The device cannot be resolved at all; recovery
                        // would fail the same way, so don't try.
                        token.mark_superseded();
                        failed = true;
                        break;
                    }
                };

                // Re-check supersession under the permit. Whoever
                // supersedes a fade does so while holding this same
                // permit (and the registration is only ever dropped by
                // us after this point), so if we queued behind a newer
                // command, that one has already been sent by now and
                // emitting anything further would be stale traffic
                // after it.
                if token.is_superseded() {
                    return;
                }

                // The final step's poll must not be suppressed: flag
                // the fade as finished so polling resumes while it
                // lands. The registration itself stays in place — now
                // and past the send — so a command racing us here still
                // reaches (and blocks) this token via cancel_fade, and
                // so `live_fade_value` keeps a retarget continuous until
                // the reconciling poll retires the registration.
                if is_last {
                    token.mark_finished();
                }

                // Stay inside Govee's account-wide control budget even
                // when many devices fade concurrently.
                fade::global_step_gate().await;

                // Route through State so LAN/IoT/platform precedence
                // stays identical to a plain brightness command.
                let result = self.device_set_brightness(&control, step.brightness).await;
                if result.is_ok() {
                    token.note_emitted(step.brightness);
                }
                // The final step is this command's last send attempt on
                // the success path, so this coordinator owns the one
                // reconciling poll the command costs; when it fails, the
                // recovery block below takes that responsibility instead.
                if is_last && result.is_ok() {
                    control.arm_reconciling_poll();
                }
                result
            }; // control drops here, firing the poll when armed

            if let Err(err) = outcome {
                log::error!("fade step for {device_id} failed: {err:#}");
                // Don't strand the device mid-ramp: abandon the
                // remaining intermediate steps and go for the contract
                // value directly, below.
                failed = true;
                break;
            }
        }

        // A step failed and no newer command superseded us: issue the
        // final target once anyway. Leaving the device at the last
        // intermediate value silently breaks the contract with Home
        // Assistant (the light must end where it was told to end); one
        // redundant final set is cheap by comparison.
        if failed && !token.is_superseded() {
            if let Ok(mut control) = self
                .resolve_device_for_control_inner(&device_id, false, false)
                .await
            {
                // Same under-permit re-check: a command may have taken
                // over while we were resolving.
                if !token.is_superseded() {
                    match self.device_set_brightness(&control, target).await {
                        Ok(()) => token.note_emitted(target),
                        Err(err) => {
                            log::error!(
                                "final brightness recovery for {device_id} after failed fade: {err:#}"
                            );
                        }
                    }
                    // This attempt is the command's last send regardless
                    // of whether it landed, so this coordinator owns the
                    // single reconciling poll the command costs —
                    // mirroring a plain control that failed to send.
                    control.arm_reconciling_poll();
                }
            }
        }

        // A cleanly completed fade deliberately KEEPS its registration
        // (finished, no longer live): until its reconciling poll lands,
        // the device's reported state is still the pre-fade value, and
        // `live_fade_value` uses the finished registration to keep a
        // retarget in that window continuous with what was actually
        // emitted. `poll_after_control` retires it once the poll has
        // actually run. Anything other than clean completion (a failed
        // step, or supersession by a newer command that already removed
        // or marked the token) forgets it right away.
        if failed || token.is_superseded() {
            self.forget_fade(&device_id, &token).await;
        }
        abort_guard.completed = true;
    }

    pub async fn get_hass_disco_prefix(&self) -> String {
        self.hass_discovery_prefix.lock().await.to_string()
    }

    /// Returns a mutable version of the specified device, creating
    /// an entry for it if necessary.
    pub async fn device_mut(&self, sku: &str, id: &str) -> MappedMutexGuard<'_, Device> {
        let devices = self.devices_by_id.lock().await;
        MutexGuard::map(devices, |devices| {
            devices
                .entry(id.to_string())
                .or_insert_with(|| Device::new(sku, id))
        })
    }

    pub async fn devices(&self) -> Vec<Device> {
        self.devices_by_id.lock().await.values().cloned().collect()
    }

    /// Returns an immutable copy of the specified Device
    pub async fn device_by_id(&self, id: &str) -> Option<Device> {
        let devices = self.devices_by_id.lock().await;
        devices.get(id).cloned()
    }

    async fn semaphore_for_device(&self, device: &Device) -> Arc<Semaphore> {
        self.semaphore_by_id
            .lock()
            .await
            .entry(device.id.clone())
            .or_insert_with(|| Arc::new(Semaphore::new(1)))
            .clone()
    }

    pub async fn resolve_device_read_only(self: &Arc<Self>, label: &str) -> anyhow::Result<Device> {
        self.resolve_device(label)
            .await
            .ok_or_else(|| anyhow::anyhow!("device '{label}' not found"))
    }

    /// Resolve a device based on its label.
    /// Assuming the device is found, returns a Coordinator, which is a
    /// struct that ensures that only one task at a time can be processing
    /// control requests for a device.
    /// This method will not return until the calling task is permitted
    /// to proceed with its control attempt.
    ///
    /// Taking the permit also supersedes any software fade in progress
    /// for the device: the fade runner re-checks its cancellation token
    /// under this same permit, so once this returns, no queued fade step
    /// can be emitted after what the caller sends (latest command wins,
    /// for every control entry point — MQTT, HTTP API, and the hass
    /// entity platforms alike).
    pub async fn resolve_device_for_control(
        self: &Arc<Self>,
        label: &str,
    ) -> anyhow::Result<Coordinator> {
        self.resolve_device_for_control_inner(label, true, true)
            .await
    }

    /// Shared implementation of [`Self::resolve_device_for_control`].
    /// `schedule_poll` controls whether dropping the returned
    /// Coordinator triggers a reconciling state poll (the fade entry path
    /// passes `false`: a fade's one reconciling poll is scheduled by its
    /// runner's final step, so the entry command must not add a second
    /// one — `Coordinator::arm_reconciling_poll` covers a fade that
    /// collapses to a single set). `supersede_fade` controls whether
    /// taking the permit cancels a live fade. The fade runner itself
    /// calls this with supersession off (it would cancel its own fade)
    /// and with polling scheduled only for its final step.
    pub(crate) async fn resolve_device_for_control_inner(
        self: &Arc<Self>,
        label: &str,
        schedule_poll: bool,
        supersede_fade: bool,
    ) -> anyhow::Result<Coordinator> {
        let device = self
            .resolve_device(label)
            .await
            .ok_or_else(|| anyhow::anyhow!("device '{label}' not found"))?;
        let semaphore = self.semaphore_for_device(&device).await;
        let permit = semaphore.acquire_owned().await?;
        if supersede_fade {
            self.cancel_fade(&device.id).await;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();

        {
            // Park the reconciling-poll follow-up on every control
            // acquisition: it runs only if the Coordinator is dropped
            // *armed* (its Drop fires the message; an un-armed drop
            // closes the channel and this exits without polling).
            // Parking unarmed by default lets every entry point —
            // including the fade runner and its recovery retry, which
            // acquire unarmed and arm later — leave exactly one poll
            // behind on whichever coordinator carried the command's
            // last send attempt, and nothing otherwise.
            let state = self.clone();
            let device_id = device.id.to_string();
            tokio::spawn(async move {
                if rx.await.is_ok() {
                    state.poll_after_control(device_id).await
                }
            });
        }

        Ok(Coordinator::new(device, permit, tx, schedule_poll))
    }

    /// Resolve a device using its name, computed name, id or label,
    /// ignoring case.
    pub async fn resolve_device(&self, label: &str) -> Option<Device> {
        let devices = self.devices_by_id.lock().await;

        // Try by id first
        if let Some(device) = devices.get(label) {
            return Some(device.clone());
        }

        for d in devices.values() {
            if d.name().eq_ignore_ascii_case(label)
                || d.id.eq_ignore_ascii_case(label)
                || topic_safe_id(d).eq_ignore_ascii_case(label)
                || d.ip_addr()
                    .map(|ip| ip.to_string().eq_ignore_ascii_case(label))
                    .unwrap_or(false)
                || d.computed_name().eq_ignore_ascii_case(label)
            {
                return Some(d.clone());
            }
        }

        None
    }

    pub async fn set_hass_client(&self, client: HassClient) {
        self.hass_client.lock().await.replace(client);
    }

    pub async fn get_hass_client(&self) -> Option<HassClient> {
        self.hass_client.lock().await.clone()
    }

    pub async fn set_iot_client(&self, client: IotClient) {
        self.iot_client.lock().await.replace(client);
    }

    pub async fn get_iot_client(&self) -> Option<IotClient> {
        self.iot_client.lock().await.clone()
    }

    pub async fn set_lan_client(&self, client: LanClient) {
        self.lan_client.lock().await.replace(client);
    }

    pub async fn get_lan_client(&self) -> Option<LanClient> {
        self.lan_client.lock().await.clone()
    }

    pub async fn set_platform_client(&self, client: GoveeApiClient) {
        self.platform_client.lock().await.replace(client);
    }

    pub async fn get_platform_client(&self) -> Option<GoveeApiClient> {
        self.platform_client.lock().await.clone()
    }

    pub async fn set_undoc_client(&self, client: GoveeUndocumentedApi) {
        self.undoc_client.lock().await.replace(client);
    }

    #[allow(dead_code)]
    pub async fn get_undoc_client(&self) -> Option<GoveeUndocumentedApi> {
        self.undoc_client.lock().await.clone()
    }

    pub async fn poll_iot_api(self: &Arc<Self>, device: &Device) -> anyhow::Result<bool> {
        if let Some(iot) = self.get_iot_client().await {
            if let Some(info) = device.undoc_device_info.clone() {
                if iot.is_device_compatible(&info.entry) {
                    let device_state = device.device_state();
                    log::info!("requesting update via IoT MQTT {device} {device_state:?}");
                    match iot
                        .request_status_update(&info.entry)
                        .await
                        .context("iot.request_status_update")
                    {
                        Err(err) => {
                            log::error!("Failed: {err:#}");
                        }
                        Ok(()) => {
                            // The response will come in async via the mqtt loop in iot.rs
                            // However, if the device is offline, nothing will change our state.
                            // Let's explicitly mark the device as having been polled so that
                            // we don't keep sending a request every minute.
                            self.device_mut(&device.sku, &device.id)
                                .await
                                .set_last_polled();

                            return Ok(true);
                        }
                    }
                }
            }
        }
        Ok(false)
    }

    pub async fn poll_platform_api(self: &Arc<Self>, device: &Device) -> anyhow::Result<bool> {
        if let Some(client) = self.get_platform_client().await {
            if let DeviceType::Other(other) = &device.device_type() {
                // Cannot poll an unknown device
                // <https://github.com/wez/govee2mqtt/issues/391>
                // <https://github.com/wez/govee2mqtt/issues/501>
                // <https://github.com/wez/govee2mqtt/issues/394>
                log::trace!("device {device} cannot be polled because it has type Other: {other}");
                return Ok(false);
            }

            let device_state = device.device_state();
            log::info!("requesting update via Platform API {device} {device_state:?}");
            if let Some(info) = &device.http_device_info {
                let http_state = client
                    .get_device_state(info)
                    .await
                    .context("get_device_state")?;
                log::trace!("updated state for {device}");

                {
                    let mut device = self.device_mut(&device.sku, &device.id).await;
                    device.set_http_device_state(http_state);
                    device.set_last_polled();
                }
                self.notify_of_state_change(&device.id)
                    .await
                    .context("state.notify_of_state_change")?;
                return Ok(true);
            }
        } else {
            log::trace!(
                "device {device} wanted a status update, but there is no platform client available"
            );
        }
        Ok(false)
    }

    async fn poll_lan_api<F: Fn(&LanDeviceStatus) -> bool>(
        self: &Arc<Self>,
        device: &LanDevice,
        acceptor: F,
    ) -> anyhow::Result<()> {
        match self.get_lan_client().await {
            Some(client) => {
                let deadline = Instant::now() + Duration::from_secs(5);
                while Instant::now() <= deadline {
                    let status = client.query_status(device).await?;
                    let accepted = (acceptor)(&status);
                    self.device_mut(&device.sku, &device.device)
                        .await
                        .set_lan_device_status(status);
                    if accepted {
                        break;
                    }
                    sleep(Duration::from_millis(100)).await;
                }
                self.notify_of_state_change(&device.device).await?;
                Ok(())
            }
            None => anyhow::bail!("no lan client"),
        }
    }

    pub async fn device_control<V: Into<JsonValue>>(
        self: &Arc<Self>,
        device: &Device,
        capability: &DeviceCapability,
        value: V,
    ) -> anyhow::Result<()> {
        let value: JsonValue = value.into();
        if let Some(client) = self.get_platform_client().await {
            if let Some(info) = &device.http_device_info {
                log::info!("Using Platform API to send {value:?} control to {device}");
                client.control_device(info, capability, value).await?;
                return Ok(());
            }
        }

        anyhow::bail!("Unable to use Platform API to control {device}");
    }

    pub async fn device_light_power_on(
        self: &Arc<Self>,
        device: &Device,
        on: bool,
    ) -> anyhow::Result<()> {
        if self
            .try_humidifier_set_nightlight(device, |p| p.on = on)
            .await?
        {
            return Ok(());
        }

        let instance_name = device
            .get_light_power_toggle_instance_name()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Don't know how to toggle just the light portion of {device}. \
                     Please share the device metadata and state if you report this issue"
                )
            })?;

        if let Some(lan_dev) = &device.lan_device {
            log::info!("Using LAN API to set {device} light power state");
            lan_dev.send_turn(on).await?;
            self.poll_lan_api(lan_dev, |status| status.on == on).await?;
            return Ok(());
        }

        if device.iot_api_supported() {
            if let Some(iot) = self.get_iot_client().await {
                if let Some(info) = &device.undoc_device_info {
                    log::info!("Using IoT API to set {device} light power state");
                    iot.set_power_state(&info.entry, on).await?;
                    return Ok(());
                }
            }
        }

        if let Some(client) = self.get_platform_client().await {
            if let Some(info) = &device.http_device_info {
                log::info!("Using Platform API to set {device} light {instance_name} state");
                client.set_toggle_state(info, instance_name, on).await?;
                return Ok(());
            }
        }

        anyhow::bail!("Unable to control light power state for {device}");
    }

    pub async fn device_power_on(
        self: &Arc<Self>,
        device: &Device,
        on: bool,
    ) -> anyhow::Result<()> {
        if let Some(lan_dev) = &device.lan_device {
            log::info!("Using LAN API to set {device} power state");
            lan_dev.send_turn(on).await?;
            self.poll_lan_api(lan_dev, |status| status.on == on).await?;
            return Ok(());
        }

        if device.iot_api_supported() {
            if let Some(iot) = self.get_iot_client().await {
                if let Some(info) = &device.undoc_device_info {
                    log::info!("Using IoT API to set {device} power state");
                    iot.set_power_state(&info.entry, on).await?;
                    return Ok(());
                }
            }
        }

        if let Some(client) = self.get_platform_client().await {
            if let Some(info) = &device.http_device_info {
                log::info!("Using Platform API to set {device} power state");
                client.set_power_state(info, on).await?;
                return Ok(());
            }
        }

        anyhow::bail!("Unable to control power state for {device}");
    }

    pub async fn device_set_brightness(
        self: &Arc<Self>,
        device: &Device,
        percent: u8,
    ) -> anyhow::Result<()> {
        #[cfg(test)]
        if test_sink::ENABLED.load(std::sync::atomic::Ordering::SeqCst) {
            // Test-only simulated device (see `test_sink`): record the
            // send, optionally fail it, then optionally hold the caller
            // (and its control permit) for a simulated round-trip delay.
            test_sink::note_send(&device.id, percent);
            test_sink::maybe_fail(&device.id)?;
            test_sink::step_latency(&device.id).await;
            return Ok(());
        }
        if self
            .try_humidifier_set_nightlight(device, |p| {
                p.brightness = percent;
                p.on = true;
            })
            .await?
        {
            return Ok(());
        }

        if let Some(lan_dev) = &device.lan_device {
            log::info!("Using LAN API to set {device} brightness");
            lan_dev.send_brightness(percent).await?;
            self.poll_lan_api(lan_dev, |status| status.brightness == percent)
                .await?;
            return Ok(());
        }

        if device.iot_api_supported() {
            if let Some(iot) = self.get_iot_client().await {
                if let Some(info) = &device.undoc_device_info {
                    log::info!("Using IoT API to set {device} brightness");
                    iot.set_brightness(&info.entry, percent).await?;
                    return Ok(());
                }
            }
        }

        if let Some(client) = self.get_platform_client().await {
            if let Some(info) = &device.http_device_info {
                log::info!("Using Platform API to set {device} brightness");
                client.set_brightness(info, percent).await?;
                return Ok(());
            }
        }
        anyhow::bail!("Unable to control brightness for {device}");
    }

    pub async fn device_set_color_temperature(
        self: &Arc<Self>,
        device: &Device,
        kelvin: u32,
    ) -> anyhow::Result<()> {
        if let Some(lan_dev) = &device.lan_device {
            log::info!("Using LAN API to set {device} color temperature");
            lan_dev.send_color_temperature_kelvin(kelvin).await?;
            self.poll_lan_api(lan_dev, |status| status.color_temperature_kelvin == kelvin)
                .await?;
            self.device_mut(&device.sku, &device.id)
                .await
                .set_active_scene(None);
            return Ok(());
        }

        if device.iot_api_supported() {
            if let Some(iot) = self.get_iot_client().await {
                if let Some(info) = &device.undoc_device_info {
                    log::info!("Using IoT API to set {device} color temperature");
                    iot.set_color_temperature(&info.entry, kelvin).await?;
                    return Ok(());
                }
            }
        }

        if let Some(client) = self.get_platform_client().await {
            if let Some(info) = &device.http_device_info {
                log::info!("Using Platform API to set {device} color temperature");
                client.set_color_temperature(info, kelvin).await?;
                self.device_mut(&device.sku, &device.id)
                    .await
                    .set_active_scene(None);
                return Ok(());
            }
        }
        anyhow::bail!("Unable to control color temperature for {device}");
    }

    // FIXME: this function probably shouldn't exist here
    async fn try_humidifier_set_nightlight<F: Fn(&mut SetHumidifierNightlightParams)>(
        self: &Arc<Self>,
        device: &Device,
        apply: F,
    ) -> anyhow::Result<bool> {
        let mut params: SetHumidifierNightlightParams =
            device.nightlight_state.unwrap_or_default().into();
        (apply)(&mut params);

        if let Ok(command) = Base64HexBytes::encode_for_sku(&device.sku, &params) {
            if let Some(iot) = self.get_iot_client().await {
                if let Some(info) = &device.undoc_device_info {
                    log::info!("Using IoT API to set {device} color");
                    iot.send_real(&info.entry, command.base64()).await?;
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    pub async fn humidifier_set_parameter(
        self: &Arc<Self>,
        device: &Device,
        work_mode: i64,
        value: i64,
    ) -> anyhow::Result<()> {
        if let Ok(command) = Base64HexBytes::encode_for_sku(
            &device.sku,
            &SetHumidifierMode {
                mode: work_mode as u8,
                param: value as u8,
            },
        ) {
            if let Some(iot) = self.get_iot_client().await {
                if let Some(info) = &device.undoc_device_info {
                    iot.send_real(&info.entry, command.base64()).await?;
                    return Ok(());
                }
            }
        }

        if let Some(client) = self.get_platform_client().await {
            if let Some(info) = &device.http_device_info {
                client.set_work_mode(info, work_mode, value).await?;
                return Ok(());
            }
        }
        anyhow::bail!("Unable to control humidifier parameter work_mode={work_mode} for {device}");
    }

    pub async fn device_set_color_rgb(
        self: &Arc<Self>,
        device: &Device,
        r: u8,
        g: u8,
        b: u8,
    ) -> anyhow::Result<()> {
        if self
            .try_humidifier_set_nightlight(device, |p| {
                p.r = r;
                p.g = g;
                p.b = b;
                p.on = true;
            })
            .await?
        {
            return Ok(());
        }

        if let Some(lan_dev) = &device.lan_device {
            let color = crate::lan_api::DeviceColor { r, g, b };
            log::info!("Using LAN API to set {device} color");
            lan_dev.send_color_rgb(color).await?;
            self.poll_lan_api(lan_dev, |status| status.color == color)
                .await?;
            self.device_mut(&device.sku, &device.id)
                .await
                .set_active_scene(None);
            return Ok(());
        }

        if device.iot_api_supported() {
            if let Some(iot) = self.get_iot_client().await {
                if let Some(info) = &device.undoc_device_info {
                    log::info!("Using IoT API to set {device} color");
                    iot.set_color_rgb(&info.entry, r, g, b).await?;
                    return Ok(());
                }
            }
        }

        if let Some(client) = self.get_platform_client().await {
            if let Some(info) = &device.http_device_info {
                log::info!("Using Platform API to set {device} color");
                client.set_color_rgb(info, r, g, b).await?;
                self.device_mut(&device.sku, &device.id)
                    .await
                    .set_active_scene(None);
                return Ok(());
            }
        }
        anyhow::bail!("Unable to control color for {device}");
    }

    pub async fn poll_after_control(self: &Arc<Self>, id: String) {
        let Some(device) = self.device_by_id(&id).await else {
            return;
        };

        let iot_available = self.get_iot_client().await.is_some();

        if device.pollable_via_iot() && iot_available {
            return;
        }
        if device.pollable_via_lan() {
            return;
        }

        // Add a slight delay, as the status returned
        // by the platform API isn't guaranteed to be
        // coherent with the command we just issued
        // right away :-/
        sleep(Duration::from_secs(5)).await;

        // A fade costs exactly one reconciling poll: the one scheduled by
        // its final step, which — landing after the fade finished —
        // claims the poll here, reconciles, and retires the registration.
        // Any other poll landing while a fade owns the device, or while a
        // cleanly finished fade still awaits that one poll, is deferred:
        // the fade's poll reconciles at least as fresh. (A superseded or
        // already-retired registration is as good as absent: the control
        // that superseded it owes — and carries — its own poll.)
        enum FadePollDecision {
            Normal,
            Defer,
            Reconcile(Arc<FadeToken>),
        }
        let decision = {
            let fades = self.fades.lock().await;
            match fades.get(&id) {
                Some(token) if !token.is_superseded() => {
                    if token.is_finished() && token.claim_reconcile() {
                        FadePollDecision::Reconcile(Arc::clone(token))
                    } else {
                        FadePollDecision::Defer
                    }
                }
                _ => FadePollDecision::Normal,
            }
        };
        if matches!(decision, FadePollDecision::Defer) {
            log::debug!("Deferring state poll for {device}: fade owns reconciliation");
            return;
        }

        #[cfg(test)]
        test_sink::note_poll(&id);

        log::info!("Polling {device} to get latest state after control");
        match self.poll_platform_api(&device).await {
            Ok(_) => {
                if let FadePollDecision::Reconcile(token) = decision {
                    // The reported state is current now, so the finished
                    // fade's registration is no longer needed to keep a
                    // retarget continuous.
                    self.forget_fade(&id, &token).await;
                }
            }
            Err(err) => {
                // If this was the finished fade's one poll and it
                // failed, un-claim it: the registration is kept and the
                // next parked poll retries the retire, so continuity
                // keeps winning over the (unrefreshed) reported state
                // until a poll actually lands.
                if let FadePollDecision::Reconcile(token) = &decision {
                    token.restore_reconcile();
                }
                log::error!("Polling {device} failed: {err:#}");
            }
        }
    }

    pub async fn device_list_scenes(&self, device: &Device) -> anyhow::Result<Vec<String>> {
        // TODO: some plumbing to maintain offline scene controls for preferred-LAN control
        if let Some(client) = self.get_platform_client().await {
            if let Some(info) = &device.http_device_info {
                return Ok(sort_and_dedup_scenes(client.list_scene_names(info).await?));
            }
        }

        if let Ok(categories) = GoveeUndocumentedApi::get_scenes_for_device(&device.sku).await {
            let mut names = vec![];
            for cat in categories {
                for scene in cat.scenes {
                    for effect in scene.light_effects {
                        if effect.scene_code != 0 {
                            names.push(scene.scene_name);
                            break;
                        }
                    }
                }
            }
            return Ok(sort_and_dedup_scenes(names));
        }

        log::trace!("Platform API unavailable: Don't know how to list scenes for {device}");

        Ok(vec![])
    }

    pub async fn device_set_target_temperature(
        self: &Arc<Self>,
        device: &Device,
        instance_name: &str,
        target: TemperatureValue,
    ) -> anyhow::Result<()> {
        if let Some(client) = self.get_platform_client().await {
            if let Some(info) = &device.http_device_info {
                log::info!("Using Platform API to set {device} target temperature to {target}");
                client
                    .set_target_temperature(info, instance_name, target)
                    .await?;
                return Ok(());
            }
        }

        anyhow::bail!("Unable to set temperature for {device}");
    }

    pub async fn device_set_scene(
        self: &Arc<Self>,
        device: &Device,
        scene: &str,
    ) -> anyhow::Result<()> {
        // TODO: some plumbing to maintain offline scene controls for preferred-LAN control
        let avoid_platform_api = device.avoid_platform_api();

        if !avoid_platform_api {
            if let Some(client) = self.get_platform_client().await {
                if let Some(info) = &device.http_device_info {
                    log::info!("Using Platform API to set {device} to scene {scene}");
                    client.set_scene_by_name(info, scene).await?;
                    self.device_mut(&device.sku, &device.id)
                        .await
                        .set_active_scene(Some(scene));
                    return Ok(());
                }
            }
        }

        if let Some(lan_dev) = &device.lan_device {
            log::info!("Using LAN API to set {device} to scene {scene}");
            lan_dev.set_scene_by_name(scene).await?;

            self.device_mut(&device.sku, &device.id)
                .await
                .set_active_scene(Some(scene));
            return Ok(());
        }

        anyhow::bail!("Unable to set scene for {device}");
    }

    // Take care not to call this while you hold a mutable device
    // reference, as that will deadlock!
    pub async fn notify_of_state_change(self: &Arc<Self>, device_id: &str) -> anyhow::Result<()> {
        let Some(canonical_device) = self.device_by_id(device_id).await else {
            anyhow::bail!("cannot find device {device_id}!?");
        };

        if let Some(hass) = self.get_hass_client().await {
            hass.advise_hass_of_light_state(&canonical_device, self)
                .await?;
        }

        Ok(())
    }
}

pub fn sort_and_dedup_scenes(mut scenes: Vec<String>) -> Vec<String> {
    scenes.sort_by_key(|s| s.to_ascii_lowercase());
    scenes.dedup();
    scenes
}

/// Safety net for `run_fade`: dropped without `completed` being set
/// (a panic unwinding out of the runner), the guard marks the fade's
/// token superseded so the dead runner cannot leave a "live"
/// registration deferring the device's state polling indefinitely.
/// Every normal exit path sets `completed` first.
struct FadeAbortGuard {
    token: Arc<FadeToken>,
    completed: bool,
}

impl Drop for FadeAbortGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.token.mark_superseded();
        }
    }
}

/// Test-only simulated device, attached at the two points where control
/// traffic actually leaves the bridge: `State::device_set_brightness`
/// (brightness sends) and `State::poll_after_control` (reconciling
/// polls). With `ENABLED` set, tests can drive the real command
/// handlers, the real fade runner, and the real control semaphore, and
/// observe exactly what a device would have received, in order, with
/// per-device isolation so tests can run in parallel.
#[cfg(test)]
pub mod test_sink {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    pub static ENABLED: AtomicBool = AtomicBool::new(false);

    #[derive(Default)]
    struct SinkState {
        delays: HashMap<String, std::collections::VecDeque<u64>>,
        log: HashMap<String, Vec<(std::time::Instant, u8)>>,
        fails: HashMap<String, usize>,
        fail_at: HashMap<String, usize>,
        attempts: HashMap<String, usize>,
        polls: HashMap<String, usize>,
    }

    static SINK: std::sync::LazyLock<Mutex<SinkState>> =
        std::sync::LazyLock::new(|| Mutex::new(SinkState::default()));

    fn with_sink<T>(f: impl FnOnce(&mut SinkState) -> T) -> T {
        f(&mut SINK.lock().unwrap())
    }

    /// Start recording for a device id (a fresh log); `delays_ms` are
    /// per-send simulated round-trip latencies, consumed front-first
    /// (0 = instantaneous) for as long as they last.
    pub fn arm(id: &str, delays_ms: &[u64]) {
        ENABLED.store(true, Ordering::SeqCst);
        with_sink(|s| {
            s.log.insert(id.to_string(), Vec::new());
            s.polls.insert(id.to_string(), 0);
            s.fails.remove(id);
            s.fail_at.remove(id);
            s.attempts.insert(id.to_string(), 0);
            s.delays
                .insert(id.to_string(), delays_ms.iter().copied().collect());
        });
    }

    /// Make the next `n` brightness sends for this device fail.
    pub fn arm_fails(id: &str, n: usize) {
        with_sink(|s| s.fails.insert(id.to_string(), n));
    }

    /// Make exactly the `attempt`-th brightness send for this device
    /// (1-based, counted from `arm`) fail, leaving earlier sends and any
    /// retries to succeed.
    pub fn arm_fail_at(id: &str, attempt: usize) {
        ENABLED.store(true, Ordering::SeqCst);
        with_sink(|s| s.fail_at.insert(id.to_string(), attempt));
    }

    pub fn note_send(id: &str, brightness: u8) {
        with_sink(|s| {
            *s.attempts.entry(id.to_string()).or_default() += 1;
            if s.log.contains_key(id) {
                s.log
                    .get_mut(id)
                    .unwrap()
                    .push((std::time::Instant::now(), brightness));
            }
        });
    }

    pub fn maybe_fail(id: &str) -> anyhow::Result<()> {
        let fail = with_sink(|s| match s.fail_at.get(id) {
            Some(at) if s.attempts.get(id).copied() == Some(*at) => true,
            _ => match s.fails.get_mut(id) {
                Some(n) if *n > 0 => {
                    *n -= 1;
                    true
                }
                _ => false,
            },
        });
        if fail {
            return Err(anyhow::anyhow!("simulated device I/O failure"));
        }
        Ok(())
    }

    /// Pop the next simulated round-trip latency for this device and
    /// sleep it, holding the caller (and its control permit) like a slow
    /// cloud round trip would.
    pub async fn step_latency(id: &str) {
        let delay = with_sink(|s| {
            s.delays
                .get_mut(id)
                .and_then(|d| d.pop_front())
                .unwrap_or(0)
        });
        if delay > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
        }
    }

    pub fn note_poll(id: &str) {
        with_sink(|s| {
            if let Some(n) = s.polls.get_mut(id) {
                *n += 1;
            }
        });
    }

    /// Number of reconciling polls that actually ran for this device.
    pub fn polls(id: &str) -> usize {
        with_sink(|s| s.polls.get(id).copied().unwrap_or(0))
    }

    /// (ms since `t0`, brightness) for every recorded send, in order.
    pub fn log_since(id: &str, t0: std::time::Instant) -> Vec<(u128, u8)> {
        with_sink(|s| {
            s.log
                .get(id)
                .map(|entries| {
                    entries
                        .iter()
                        .map(|(t, b)| (t.duration_since(t0).as_millis(), *b))
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    /// Just the brightnesses recorded, in order.
    pub fn sent(id: &str) -> Vec<u8> {
        with_sink(|s| {
            s.log
                .get(id)
                .map(|entries| entries.iter().map(|(_, b)| *b).collect())
                .unwrap_or_default()
        })
    }
}
