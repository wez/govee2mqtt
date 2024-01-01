use crate::lan_api::{DeviceStatus as LanDeviceStatus, LanDevice};
use chrono::{DateTime, Utc};
use std::net::IpAddr;

#[derive(Default, Clone, Debug)]
pub struct Device {
    pub sku: String,
    pub id: String,

    /// Name assigned via the Govee App
    pub govee_name: Option<String>,
    /// The name of the group assigned in the Govee App
    pub room: Option<String>,

    /// The IP address as found via LAN discovery or other means
    pub ip_addr: Option<IpAddr>,

    /// Probed LAN device information, found either via discovery
    /// or explicit probing by IP address
    pub lan_device: Option<LanDevice>,
    pub last_lan_device_update: Option<DateTime<Utc>>,
    pub lan_device_status: Option<LanDeviceStatus>,
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
        match &self.govee_name {
            Some(name) => name.to_string(),
            None => self.computed_name(),
        }
    }

    /// compute a name from the SKU and the last couple of bytes from the
    /// device id, similar to the device name that would show up in a BLE
    /// scan, or the default name for the device if not otherwise configured
    /// in the Govee App.
    pub fn computed_name(&self) -> String {
        let mut name = format!("{}_{}", self.sku, &self.id[18..]);
        name.retain(|c| c != ':');
        name
    }

    /// Sets the assigned name
    pub fn set_govee_name<N: Into<String>>(&mut self, name: N) {
        self.govee_name.replace(name.into());
    }

    /// Sets the room
    pub fn set_room<N: Into<String>>(&mut self, room: N) {
        self.room.replace(room.into());
    }

    /// Sets the IP address
    pub fn set_ip_addr(&mut self, ip_addr: IpAddr) {
        self.ip_addr.replace(ip_addr);
    }

    /// Update the LAN device information
    pub fn set_lan_device(&mut self, device: LanDevice) {
        self.lan_device.replace(device);
        self.last_lan_device_update.replace(Utc::now());
    }

    /// Update the LAN device status information
    pub fn set_lan_device_status(&mut self, status: LanDeviceStatus) {
        self.lan_device_status.replace(status);
        self.last_lan_device_update.replace(Utc::now());
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn name_compute() {
        let device = Device::new("H6000", "AA:BB:CC:DD:EE:FF:42:2A");
        assert_eq!(device.name(), "H6000_422A");
    }
}
