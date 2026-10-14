use crate::ble::{NotifyEnergyMonitoring, NotifyHumidifierNightlightParams};
use crate::commands::serve::POLL_INTERVAL;
use crate::lan_api::{DeviceColor, DeviceStatus as LanDeviceStatus, LanDevice};
use crate::platform_api::{
    DeviceCapability, DeviceCapabilityState, DeviceType, HttpDeviceInfo, HttpDeviceState,
};
use crate::service::quirks::{resolve_quirk, Quirk, BULB};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::net::IpAddr;

#[derive(Default, Clone, Debug)]
pub struct Device {
    pub sku: String,
    pub id: String,

    /// Probed LAN device information, found either via discovery
    /// or explicit probing by IP address
    pub lan_device: Option<LanDevice>,
    pub last_lan_device_update: Option<DateTime<Utc>>,

    pub lan_device_status: Option<LanDeviceStatus>,
    pub last_lan_device_status_update: Option<DateTime<Utc>>,

    pub http_device_info: Option<HttpDeviceInfo>,
    pub last_http_device_update: Option<DateTime<Utc>>,

    pub http_device_state: Option<HttpDeviceState>,
    pub last_http_device_state_update: Option<DateTime<Utc>>,

    pub undoc_device_info: Option<UndocDeviceInfo>,
    pub last_undoc_device_info_update: Option<DateTime<Utc>>,

    pub iot_device_status: Option<LanDeviceStatus>,
    pub last_iot_device_status_update: Option<DateTime<Utc>>,

    pub nightlight_state: Option<NotifyHumidifierNightlightParams>,
    pub target_humidity_percent: Option<u8>,
    pub humidifier_work_mode: Option<u8>,
    pub humidifier_param_by_mode: HashMap<u8, u8>,

    /// The most recent energy monitoring readings and when they arrived
    pub energy_monitoring: Option<(DateTime<Utc>, NotifyEnergyMonitoring)>,
    pub energy_counter: Option<EnergyCounterState>,
    /// Whether any energy counter state saved by a previous run
    /// has been restored, see restore_energy_counter
    pub energy_counter_restored: bool,
    pub last_energy_monitoring_poll: Option<DateTime<Utc>>,

    pub last_polled: Option<DateTime<Utc>>,

    active_scene: Option<ActiveSceneInfo>,
}

/// Tracks the energy counter of a plug with energy monitoring, so that
/// Home Assistant can tell real resets of the counter from reboots.
///
/// The plug's counter covers the current day: it resets at local midnight,
/// and also when the plug reboots, but then the plug restores a saved copy
/// of it shortly after. The energy sensor uses the `total` state class, for
/// which Home Assistant only starts a new cycle when `last_reset` changes,
/// and books any other change, including the dip and restore of a reboot,
/// as a plain difference. So we only need to move `last_reset` when the
/// counter drops while the plug didn't reboot, or when it rebooted but
/// doesn't restore its counter, eg. after losing power over its reset time.
///
/// This is persisted in a retained MQTT message, see energy_counter_topic,
/// as `last_reset` must not change when govee2mqtt restarts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnergyCounterState {
    pub last_reset: DateTime<Utc>,
    /// When the last reading was tracked
    pub updated: DateTime<Utc>,
    /// The last energy reading, in units of 0.1Wh
    pub energy_deciwatt_hours: u32,
    /// The last on-time reading, in seconds
    pub on_time_seconds: u32,
    /// The last uptime reported by the plug, in seconds
    pub uptime_seconds: u64,
    /// When we first saw the counter drop after a reboot, while we're
    /// waiting for the plug to restore it. The readings until then
    /// aren't tracked, so that nothing goes out with the wrong last_reset.
    pub reboot_seen: Option<DateTime<Utc>>,
}

impl EnergyCounterState {
    /// Drops of up to 1Wh or 5 minutes of on-time are noise, not resets.
    /// A reset sets both back to zero, and on-time catches resets after
    /// days with little energy use, as long as the outlet was on.
    const ENERGY_NOISE_DECIWATT_HOURS: u32 = 10;
    const ON_TIME_NOISE_SECONDS: u32 = 300;

    /// Plugs restore their counters within about a minute of coming online
    const RESTORE_TIMEOUT: chrono::Duration = chrono::Duration::minutes(10);

    pub fn new(
        now: DateTime<Utc>,
        energy_deciwatt_hours: u32,
        on_time_seconds: u32,
        uptime_seconds: u64,
    ) -> Self {
        Self {
            last_reset: now,
            updated: now,
            energy_deciwatt_hours,
            on_time_seconds,
            uptime_seconds,
            reboot_seen: None,
        }
    }

    /// The plug's uptime runs about 6% slow, in steps of 10 seconds. If it
    /// grew clearly less than the time that passed since the last reading,
    /// the plug must have rebooted in between, even if it has been up for
    /// longer than it was at the last reading.
    fn rebooted(&self, now: DateTime<Utc>, uptime_seconds: u64) -> bool {
        let elapsed = (now - self.updated).num_seconds().max(0) as u64;
        uptime_seconds + 30 < self.uptime_seconds + elapsed * 9 / 10
    }

    /// Whether the plug has restored its counters after a reboot. On-time
    /// only counts while the plug is up, so without a restore it can't be
    /// well above the uptime, nor grow faster than it. Both come from the
    /// plug's clock, so this holds even if readings reach us delayed.
    fn restored(&self, on_time_seconds: u32, uptime_seconds: u64) -> bool {
        let on_time = on_time_seconds as u64;
        let uptime_growth = uptime_seconds.saturating_sub(self.uptime_seconds);
        on_time > uptime_seconds * 3 / 2 + 300
            || on_time > self.on_time_seconds as u64 + uptime_growth * 6 / 5 + 30
    }

    pub fn update(
        &mut self,
        now: DateTime<Utc>,
        energy_deciwatt_hours: u32,
        on_time_seconds: u32,
        uptime_seconds: u64,
    ) {
        let dropped = energy_deciwatt_hours + Self::ENERGY_NOISE_DECIWATT_HOURS
            < self.energy_deciwatt_hours
            || on_time_seconds + Self::ON_TIME_NOISE_SECONDS < self.on_time_seconds;
        if let Some(reboot_seen) = self.reboot_seen {
            if self.restored(on_time_seconds, uptime_seconds) {
                self.reboot_seen = None;
            } else if now - reboot_seen >= Self::RESTORE_TIMEOUT {
                // No restore came, so the reboot was a real reset
                self.reboot_seen = None;
                self.last_reset = reboot_seen;
            }
        } else if dropped {
            if !self.rebooted(now, uptime_seconds) {
                self.last_reset = now;
            } else if !self.restored(on_time_seconds, uptime_seconds) {
                self.reboot_seen = Some(now);
            }
        }
        // While waiting for a restore, keep the energy from before the
        // reboot, so that the readings until then aren't published
        if self.reboot_seen.is_none() {
            self.energy_deciwatt_hours = energy_deciwatt_hours;
        }
        self.updated = now;
        self.on_time_seconds = on_time_seconds;
        self.uptime_seconds = uptime_seconds;
    }
}

const ENERGY_MONITORING_POLL_INTERVAL: chrono::Duration = chrono::Duration::seconds(60);

impl std::fmt::Display for Device {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(fmt, "{} ({} {})", self.name(), self.id, self.sku)
    }
}

/// Govee doesn't report the active scene or music mode,
/// so we maintain our own idea of it, clearing it when
/// the color of the light is changed
#[derive(Clone, Debug)]
struct ActiveSceneInfo {
    pub name: String,
    pub color: crate::lan_api::DeviceColor,
    pub kelvin: u32,
}

/// Represents the device state; synthesized from the various
/// sources of facts that we have in the Device
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DeviceState {
    /// Whether the device is powered on
    pub on: bool,
    /// Whether the light function of the device is powered on
    pub light_on: Option<bool>,

    /// Whether the device is connected to the Govee cloud
    pub online: Option<bool>,

    /// The color temperature in kelvin
    pub kelvin: u32,

    /// The color
    pub color: crate::lan_api::DeviceColor,

    /// The brightness in percent (0-100)
    pub brightness: u8,

    /// The active effect mode, if known
    pub scene: Option<String>,

    /// Where the information came from
    pub source: &'static str,
    pub updated: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct UndocDeviceInfo {
    pub room_name: Option<String>,
    pub entry: crate::undoc_api::DeviceEntry,
}

impl Device {
    /// Create a new device given just its sku and id.
    /// No other facts are known or reflected by it at this time;
    /// they will need to be added by the caller.
    pub fn new<S: Into<String>, I: Into<String>>(sku: S, id: I) -> Self {
        Self {
            sku: sku.into(),
            id: id.into(),
            ..Self::default()
        }
    }

    /// Returns the device name; either the name defined in the Govee App,
    /// or, if we don't have the information for some reason, then we compute
    /// a name from the SKU and the last couple of bytes from the device id,
    /// similar to the device name that would show up in a BLE scan, or
    /// the default name for the device if not otherwise configured in the
    /// Govee App.
    pub fn name(&self) -> String {
        if let Some(name) = self.govee_name() {
            return name.to_string();
        }
        self.computed_name()
    }

    /// Returns the name defined for the device in the Govee App
    pub fn govee_name(&self) -> Option<&str> {
        if let Some(info) = &self.http_device_info {
            return Some(&info.device_name);
        }
        None
    }

    pub fn room_name(&self) -> Option<&str> {
        if let Some(info) = &self.undoc_device_info {
            return info.room_name.as_deref();
        }
        None
    }

    /// compute a name from the SKU and the last couple of bytes from the
    /// device id, similar to the device name that would show up in a BLE
    /// scan, or the default name for the device if not otherwise configured
    /// in the Govee App.
    pub fn computed_name(&self) -> String {
        // The id is usually "XX:XX:XX:XX:XX:XX:XX:XX" but some devices
        // report it without colons, and in lowercase.  Normalize it.
        let mut id = String::new();
        for c in self.id.chars() {
            if c == ':' {
                continue;
            }
            id.push(c.to_ascii_uppercase());
        }

        format!("{}_{}", self.sku, &id[id.len().saturating_sub(4)..])
    }

    pub fn preferred_poll_interval(&self) -> chrono::Duration {
        match self.device_type() {
            // If the kettle is on, read its temperature more frequently
            DeviceType::Kettle => {
                if self.device_state().map(|s| s.on).unwrap_or(false) {
                    chrono::Duration::seconds(60)
                } else {
                    *POLL_INTERVAL
                }
            }
            _ => *POLL_INTERVAL,
        }
    }

    pub fn ip_addr(&self) -> Option<IpAddr> {
        self.lan_device.as_ref().map(|device| device.ip)
    }

    pub fn set_last_polled(&mut self) {
        self.last_polled.replace(Utc::now());
    }

    pub fn set_nightlight_state(&mut self, params: NotifyHumidifierNightlightParams) {
        self.nightlight_state.replace(params);
    }

    pub fn set_target_humidity(&mut self, percent: u8) {
        self.target_humidity_percent.replace(percent);
    }

    pub fn set_energy_monitoring(&mut self, report: NotifyEnergyMonitoring) {
        self.energy_monitoring.replace((Utc::now(), report));
    }

    /// Tracks the energy counter of the report. This is only done once any
    /// state saved by a previous run has been restored, as otherwise
    /// last_reset would change.
    pub fn update_energy_counter(&mut self, report: &NotifyEnergyMonitoring, uptime_seconds: u64) {
        if !self.energy_counter_restored {
            return;
        }
        let now = Utc::now();
        let energy = report.energy_deciwatt_hours.0;
        let on_time = report.on_time_seconds.0;
        self.energy_counter
            .get_or_insert_with(|| EnergyCounterState::new(now, energy, on_time, uptime_seconds))
            .update(now, energy, on_time, uptime_seconds);
    }

    pub fn restore_energy_counter(&mut self, saved: EnergyCounterState) {
        self.energy_counter.get_or_insert(saved);
    }

    pub fn energy_monitoring_report(&self) -> Option<&NotifyEnergyMonitoring> {
        self.energy_monitoring.as_ref().map(|(_, report)| report)
    }

    pub fn set_last_energy_monitoring_poll(&mut self) {
        self.last_energy_monitoring_poll.replace(Utc::now());
    }

    pub fn supports_energy_monitoring(&self) -> bool {
        resolve_quirk(&self.sku).is_some_and(|quirk| quirk.energy_monitoring)
    }

    pub fn energy_monitoring_poll_due(&self) -> bool {
        self.last_energy_monitoring_poll
            .is_none_or(|last| Utc::now() - last >= ENERGY_MONITORING_POLL_INTERVAL)
    }

    /// Discards the energy monitoring readings once they are too old
    /// to reflect the current state of the plug, which happens when
    /// it stops answering our polls.
    /// Returns true if readings were discarded.
    pub fn expire_energy_monitoring(&mut self) -> bool {
        self.energy_monitoring
            .take_if(|(updated, _)| Utc::now() - *updated > ENERGY_MONITORING_POLL_INTERVAL * 5)
            .is_some()
    }

    pub fn set_humidifier_work_mode_and_param(&mut self, mode: u8, param: u8) {
        self.humidifier_work_mode.replace(mode);
        self.humidifier_param_by_mode.insert(mode, param);
    }

    /// Update the LAN device information
    pub fn set_lan_device(&mut self, device: LanDevice) {
        self.lan_device.replace(device);
        self.last_lan_device_update.replace(Utc::now());
    }

    /// Update the LAN device status information
    pub fn set_lan_device_status(&mut self, status: LanDeviceStatus) -> bool {
        let changed = self
            .lan_device_status
            .as_ref()
            .map(|prior| *prior != status)
            .unwrap_or(true);
        self.lan_device_status.replace(status);
        self.last_lan_device_status_update.replace(Utc::now());
        self.clear_scene_if_color_changed();
        changed
    }

    pub fn set_iot_device_status(&mut self, status: LanDeviceStatus) {
        self.iot_device_status.replace(status);
        self.last_iot_device_status_update.replace(Utc::now());
        self.clear_scene_if_color_changed();
    }

    pub fn set_http_device_info(&mut self, info: HttpDeviceInfo) {
        self.http_device_info.replace(info);
        self.last_http_device_update.replace(Utc::now());
    }

    pub fn set_http_device_state(&mut self, state: HttpDeviceState) {
        self.http_device_state.replace(state);
        self.last_http_device_state_update.replace(Utc::now());
        self.clear_scene_if_color_changed();
    }

    pub fn set_undoc_device_info(
        &mut self,
        entry: crate::undoc_api::DeviceEntry,
        room_name: Option<&str>,
    ) {
        self.undoc_device_info.replace(UndocDeviceInfo {
            entry,
            room_name: room_name.map(|s| s.to_string()),
        });
        self.last_undoc_device_info_update.replace(Utc::now());
        self.clear_scene_if_color_changed();
    }

    pub fn compute_iot_device_state(&self) -> Option<DeviceState> {
        let updated = self.last_iot_device_status_update?;
        let status = self.iot_device_status.as_ref()?;

        Some(DeviceState {
            on: status.on,
            light_on: if self.device_type() == DeviceType::Light {
                Some(status.on)
            } else {
                self.nightlight_state.as_ref().map(|s| s.on)
            },
            online: None,
            brightness: status.brightness,
            color: status.color,
            kelvin: status.color_temperature_kelvin,
            scene: self.active_scene.as_ref().map(|info| info.name.to_string()),
            source: "AWS IoT API",
            updated,
        })
    }

    pub fn compute_lan_device_state(&self) -> Option<DeviceState> {
        let updated = self.last_lan_device_status_update?;
        let status = self.lan_device_status.as_ref()?;

        Some(DeviceState {
            on: status.on,
            light_on: Some(status.on), // assumption: LAN API == light
            online: None,
            brightness: status.brightness,
            color: status.color,
            kelvin: status.color_temperature_kelvin,
            scene: self.active_scene.as_ref().map(|info| info.name.to_string()),
            source: "LAN API",
            updated,
        })
    }

    pub fn compute_http_device_state(&self) -> Option<DeviceState> {
        let updated = self.last_http_device_state_update?;
        let state = self.http_device_state.as_ref()?;

        let mut online = None;
        let mut on = false;
        let mut light_on = None;
        let mut brightness = 0;
        let mut color = DeviceColor::default();
        let mut kelvin = 0;

        #[derive(serde::Deserialize)]
        struct IntegerValueState {
            value: u32,
        }
        #[derive(serde::Deserialize)]
        struct BoolValueState {
            value: bool,
        }

        let light_instance = self.get_light_power_toggle_instance_name();

        for cap in &state.capabilities {
            if let Ok(value) = serde_json::from_value::<IntegerValueState>(cap.state.clone()) {
                if light_instance
                    .map(|inst| inst == cap.instance.as_str())
                    .unwrap_or(false)
                {
                    light_on.replace(value.value != 0);
                }

                match cap.instance.as_str() {
                    "powerSwitch" => {
                        on = value.value != 0;
                    }
                    "colorRgb" => {
                        color = DeviceColor {
                            r: ((value.value >> 16) & 0xff) as u8,
                            g: ((value.value >> 8) & 0xff) as u8,
                            b: (value.value & 0xff) as u8,
                        };
                    }
                    "brightness" => {
                        brightness = value.value as u8;
                    }
                    "colorTemperatureK" => {
                        kelvin = value.value;
                    }
                    _ => {}
                }
            } else if cap.instance == "online" {
                if let Ok(value) = serde_json::from_value::<BoolValueState>(cap.state.clone()) {
                    online.replace(value.value);
                }
            }
        }

        Some(DeviceState {
            on,
            light_on,
            online,
            brightness,
            color,
            kelvin,
            scene: self.active_scene.as_ref().map(|info| info.name.to_string()),
            source: "PLATFORM API",
            updated,
        })
    }

    /// Returns the most recently received state information
    pub fn device_state(&self) -> Option<DeviceState> {
        let mut candidates = vec![];

        if let Some(state) = self.compute_lan_device_state() {
            candidates.push(state);
        }
        if let Some(state) = self.compute_http_device_state() {
            candidates.push(state);
        }
        if let Some(state) = self.compute_iot_device_state() {
            candidates.push(state);
        }

        candidates.sort_by(|a, b| a.updated.cmp(&b.updated));

        candidates.pop()
    }

    /// Records the active scene name
    pub fn set_active_scene(&mut self, scene: Option<&str>) {
        match scene {
            None => {
                self.active_scene.take();
            }
            Some(scene) => {
                let (color, kelvin) = self
                    .device_state()
                    .map(|s| (s.color, s.kelvin))
                    .unwrap_or_default();
                self.active_scene.replace(ActiveSceneInfo {
                    name: scene.to_string(),
                    color,
                    kelvin,
                });
            }
        }
    }

    pub fn clear_scene_if_color_changed(&mut self) {
        if let Some(info) = &self.active_scene {
            let current = self
                .device_state()
                .map(|s| (s.color, s.kelvin))
                .unwrap_or_default();
            let scene_state = (info.color, info.kelvin);
            if current != scene_state {
                log::info!(
                    "Clearing reported scene because current {current:?} != {scene_state:?}"
                );
                self.active_scene.take();
            }
        }
    }

    pub fn device_type(&self) -> DeviceType {
        if let Some(info) = &self.http_device_info {
            info.device_type.clone()
        } else if let Some(q) = resolve_quirk(&self.sku) {
            q.device_type.clone()
        } else {
            DeviceType::Light
        }
    }

    /// Indicate whether we require the platform API data in order
    /// to correctly report the device
    pub fn needs_platform_poll(&self) -> bool {
        if !self.iot_api_supported() {
            return true;
        }

        let device_type = self.device_type();
        match (device_type, self.sku.as_str()) {
            (_, "H7160") => false,
            (DeviceType::Humidifier, _) => true,
            (DeviceType::Light, _) => false,
            (DeviceType::Kettle, _) => true,
            _ => true,
        }
    }

    pub fn pollable_via_lan(&self) -> bool {
        self.lan_device.is_some()
    }

    pub fn pollable_via_iot(&self) -> bool {
        if !self.iot_api_supported() {
            return false;
        }
        let device_type = self.device_type();
        matches!(
            (device_type, self.sku.as_str()),
            (_, "H7160") | (DeviceType::Light, _)
        )
    }

    pub fn avoid_platform_api(&self) -> bool {
        if let Some(quirk) = self.resolve_quirk() {
            if quirk.avoid_platform_api {
                return true;
            }
            if self.lan_device.is_some()
                && !self
                    .http_device_info
                    .as_ref()
                    .map(|info| info.supports_rgb())
                    .unwrap_or(false)
            {
                // Conflicting information:
                // Platform API says that this device isn't
                // a light, but the LAN API support suggests
                // that it is a light!
                // Therefore we will not trust the Platform API
                return true;
            }
        }
        false
    }

    pub fn resolve_quirk(&self) -> Option<Quirk> {
        match resolve_quirk(&self.sku) {
            Some(q) => Some(q.clone()),
            None => {
                // It's an unknown device, but since it showed up via LAN disco,
                // we can assume that it is a light
                if self.lan_device.is_some() {
                    Some(Quirk::light(Cow::Owned(self.sku.to_string()), BULB).with_lan_api())
                } else {
                    None
                }
            }
        }
    }

    pub fn get_capability_by_instance(&self, instance: &str) -> Option<&DeviceCapability> {
        self.http_device_info
            .as_ref()
            .and_then(|info| info.capability_by_instance(instance))
    }

    pub fn get_state_capability_by_instance(
        &self,
        instance: &str,
    ) -> Option<&DeviceCapabilityState> {
        self.http_device_state
            .as_ref()
            .and_then(|info| info.capability_by_instance(instance))
    }

    pub fn get_light_power_toggle_instance_name(&self) -> Option<&'static str> {
        match self.device_type() {
            DeviceType::Light => Some("powerSwitch"),
            _ => {
                // If the device's primary function is not a light,
                // then we need to avoid powering on its other function
                // here.  If it has a nightlight capability, that is
                // probably what we are controlling.
                // We may need to expand this to other power toggles
                // in the future.
                if self
                    .get_capability_by_instance("nightlightToggle")
                    .is_some()
                {
                    Some("nightlightToggle")
                } else {
                    None
                }
            }
        }
    }

    pub fn get_color_temperature_range(&self) -> Option<(u32, u32)> {
        if let Some(quirk) = self.resolve_quirk() {
            return quirk.color_temp_range;
        }

        if self.lan_device.is_some() {
            // LAN API support suggests that it is a light
            return Some((2000, 9000));
        }

        self.http_device_info
            .as_ref()
            .and_then(|info| info.get_color_temperature_range())
    }

    pub fn supports_brightness(&self) -> bool {
        if let Some(quirk) = self.resolve_quirk() {
            return quirk.supports_brightness;
        }

        if self.lan_device.is_some() {
            // LAN API support suggests that it is a light
            return true;
        }

        self.http_device_info
            .as_ref()
            .map(|info| info.supports_brightness())
            .unwrap_or(false)
    }

    pub fn iot_api_supported(&self) -> bool {
        if let Some(quirk) = self.resolve_quirk() {
            return quirk.iot_api_supported;
        }

        false
    }

    pub fn supports_rgb(&self) -> bool {
        if let Some(quirk) = self.resolve_quirk() {
            return quirk.supports_rgb;
        }

        if self.lan_device.is_some() {
            // LAN API support suggests that it is a light
            return true;
        }

        self.http_device_info
            .as_ref()
            .map(|info| info.supports_rgb())
            .unwrap_or(false)
    }

    pub fn is_ble_only_device(&self) -> Option<bool> {
        if let Some(quirk) = self.resolve_quirk() {
            return Some(quirk.ble_only);
        }

        if self.http_device_info.is_some() {
            // truly BLE-only devices are not returned via the Platform API,
            // unless we have a quirk to say otherwise
            return Some(false);
        }

        self.undoc_device_info
            .as_ref()
            .map(|info| info.entry.device_ext.device_settings.wifi_name.is_none())
    }

    pub fn is_controllable(&self) -> bool {
        !matches!(self.is_ble_only_device(), Some(true))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn energy_counter_resets() {
        let start = Utc::now();
        let minute = chrono::Duration::minutes(1);

        // Midnight, captured from the dehumidifier: uptime keeps running
        let mut counter = EnergyCounterState::new(start, 58518, 86284, 29790);
        counter.update(start + minute, 1, 5, 29850);
        assert_eq!(counter.last_reset, start + minute);

        // Midnight on a day with less than 1Wh used, while the outlet was on
        let mut counter = EnergyCounterState::new(start, 8, 86000, 29790);
        counter.update(start + minute, 0, 5, 29850);
        assert_eq!(counter.last_reset, start + minute);

        // Midnight passed while the plug was offline for 8 hours
        let later = start + chrono::Duration::hours(8);
        let mut counter = EnergyCounterState::new(start, 58518, 86284, 29790);
        counter.update(later, 3120, 26000, 56800);
        assert_eq!(counter.last_reset, later);

        // Reboot, captured from the dehumidifier: the counter drops along
        // with the uptime, and isn't tracked until the saved copy is restored
        let mut counter = EnergyCounterState::new(start, 38910, 54568, 52410);
        counter.update(start + minute, 5, 26, 20);
        counter.update(start + minute, 5, 27, 20);
        assert_eq!(counter.reboot_seen, Some(start + minute));
        assert_eq!(counter.energy_deciwatt_hours, 38910);
        counter.update(start + minute * 2, 38920, 54652, 40);
        assert_eq!(counter.last_reset, start);
        assert_eq!(counter.reboot_seen, None);
        assert_eq!(counter.energy_deciwatt_hours, 38920);

        // Reboot with a small saved copy: a heater that ran for 4 minutes.
        // The restore shows in on-time growing faster than the clock
        let mut counter = EnergyCounterState::new(start, 1000, 240, 5000);
        counter.update(start + minute, 2, 20, 20);
        assert!(counter.reboot_seen.is_some());
        counter.update(start + minute * 2, 1000, 240, 76);
        assert_eq!(counter.reboot_seen, None);
        assert_eq!(counter.last_reset, start);
        assert_eq!(counter.energy_deciwatt_hours, 1000);

        // Two readings a minute apart that reach us at the same time, eg.
        // after a reconnect, don't look like a restore
        let mut counter = EnergyCounterState::new(start, 38910, 54568, 52410);
        counter.update(start + minute, 5, 26, 20);
        counter.update(start + minute * 2, 9, 82, 76);
        counter.update(start + minute * 2, 14, 142, 132);
        assert!(counter.reboot_seen.is_some());

        // Powered off over the reset time: no restore comes, so the reboot
        // becomes the reset once we've waited for it long enough
        let mut counter = EnergyCounterState::new(start, 58518, 86284, 29790);
        let boot = start + minute * 20;
        counter.update(boot, 3, 20, 20);
        counter.update(boot + minute * 5, 30, 320, 300);
        assert_eq!(counter.last_reset, start);
        assert_eq!(counter.energy_deciwatt_hours, 58518);
        counter.update(boot + minute * 11, 70, 680, 640);
        assert_eq!(counter.last_reset, boot);
        assert_eq!(counter.reboot_seen, None);
        assert_eq!(counter.energy_deciwatt_hours, 70);

        // Rebooted during a 3 hour gap, and has been up for longer than it
        // was before by now; the restored copy was 0.84kWh behind
        let later = start + chrono::Duration::hours(3);
        let mut counter = EnergyCounterState::new(start, 71964, 78032, 5000);
        counter.update(later, 63553, 69631, 6800);
        assert_eq!(counter.last_reset, start);
        assert_eq!(counter.reboot_seen, None);

        // Tiny dips, captured from the freezer
        let mut counter = EnergyCounterState::new(start, 5055, 695, 1000);
        counter.update(start + minute, 5054, 656, 1056);
        assert_eq!(counter.last_reset, start);

        // Switching the outlet off and on doesn't change the counters
        let mut counter = EnergyCounterState::new(start, 5055, 695, 1000);
        counter.update(start + minute, 5055, 695, 1056);
        assert_eq!(counter.last_reset, start);
    }

    #[test]
    fn energy_counter_reboot_detection() {
        let start = Utc::now();
        let counter = EnergyCounterState::new(start, 0, 0, 29790);
        // Uptime grows by about 56 seconds per minute
        assert!(!counter.rebooted(start + chrono::Duration::minutes(1), 29846));
        assert!(!counter.rebooted(start + chrono::Duration::days(1), 29790 + 81216));
        assert!(counter.rebooted(start + chrono::Duration::minutes(2), 20));
        assert!(counter.rebooted(start + chrono::Duration::hours(3), 6800));
    }

    #[test]
    fn energy_counter_restore() {
        let start = Utc::now();
        let saved = EnergyCounterState::new(start, 38910, 54568, 52410);

        let mut device = Device::new("H5086", "AA:BB:CC:DD:EE:FF:42:2A");
        device.restore_energy_counter(saved.clone());
        assert_eq!(device.energy_counter, Some(saved.clone()));

        // Our own retained message comes back while we're running
        let current = EnergyCounterState::new(start + chrono::Duration::hours(1), 1, 5, 60);
        device.energy_counter.replace(current.clone());
        device.restore_energy_counter(saved);
        assert_eq!(device.energy_counter, Some(current));
    }

    #[test]
    fn energy_monitoring_expiry() {
        let mut device = Device::new("H5086", "AA:BB:CC:DD:EE:FF:42:2A");
        assert!(!device.expire_energy_monitoring());

        device.set_energy_monitoring(NotifyEnergyMonitoring::default());
        assert!(!device.expire_energy_monitoring());
        assert!(device.energy_monitoring.is_some());

        device.energy_monitoring = Some((
            Utc::now() - ENERGY_MONITORING_POLL_INTERVAL * 6,
            NotifyEnergyMonitoring::default(),
        ));
        assert!(device.expire_energy_monitoring());
        assert!(device.energy_monitoring.is_none());
        // Only reported once
        assert!(!device.expire_energy_monitoring());
    }

    #[test]
    fn name_compute() {
        let device = Device::new("H6000", "AA:BB:CC:DD:EE:FF:42:2A");
        assert_eq!(device.name(), "H6000_422A");

        let device = Device::new("H6127", "cef142b0b354995f");
        assert_eq!(device.name(), "H6127_995F");

        let device = Device::new("H6127", "ce");
        assert_eq!(device.name(), "H6127_CE");
    }
}
