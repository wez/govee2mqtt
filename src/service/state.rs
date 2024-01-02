use crate::http_api::GoveeApiClient;
use crate::lan_api::Client as LanClient;
use crate::service::device::Device;
use crate::undoc_api::GoveeUndocumentedApi;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{MappedMutexGuard, Mutex, MutexGuard};

#[derive(Default)]
pub struct State {
    devices_by_id: Mutex<HashMap<String, Device>>,
    lan_client: Mutex<Option<LanClient>>,
    platform_client: Mutex<Option<GoveeApiClient>>,
    undoc_client: Mutex<Option<GoveeUndocumentedApi>>,
}

pub type StateHandle = Arc<State>;

impl State {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a mutable version of the specified device, creating
    /// an entry for it if necessary.
    pub async fn device_mut(&self, sku: &str, id: &str) -> MappedMutexGuard<Device> {
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

    pub async fn get_undoc_client(&self) -> Option<GoveeUndocumentedApi> {
        self.undoc_client.lock().await.clone()
    }

    pub async fn device_power_on(&self, device: &Device, on: bool) -> anyhow::Result<()> {
        if let Some(lan_dev) = &device.lan_device {
            return lan_dev.send_turn(on).await;
        }

        if let Some(client) = self.get_platform_client().await {
            if let Some(info) = &device.http_device_info {
                client.set_power_state(info, on).await?;
                return Ok(());
            }
        }

        anyhow::bail!("Unable to control power state for {device}");
    }

    pub async fn device_set_brightness(&self, device: &Device, percent: u8) -> anyhow::Result<()> {
        if let Some(lan_dev) = &device.lan_device {
            return lan_dev.send_brightness(percent).await;
        }

        if let Some(client) = self.get_platform_client().await {
            if let Some(info) = &device.http_device_info {
                client.set_brightness(info, percent).await?;
                return Ok(());
            }
        }
        anyhow::bail!("Unable to control brightness for {device}");
    }

    pub async fn device_set_color_temperature(
        &self,
        device: &Device,
        kelvin: u32,
    ) -> anyhow::Result<()> {
        if let Some(lan_dev) = &device.lan_device {
            return lan_dev.send_color_temperature_kelvin(kelvin).await;
        }

        if let Some(client) = self.get_platform_client().await {
            if let Some(info) = &device.http_device_info {
                client.set_color_temperature(info, kelvin).await?;
                return Ok(());
            }
        }
        anyhow::bail!("Unable to control color temperature for {device}");
    }

    pub async fn device_set_color_rgb(
        &self,
        device: &Device,
        r: u8,
        g: u8,
        b: u8,
    ) -> anyhow::Result<()> {
        if let Some(lan_dev) = &device.lan_device {
            return lan_dev
                .send_color_rgb(crate::lan_api::DeviceColor { r, g, b })
                .await;
        }

        if let Some(client) = self.get_platform_client().await {
            if let Some(info) = &device.http_device_info {
                client.set_color_rgb(info, r, g, b).await?;
                return Ok(());
            }
        }
        anyhow::bail!("Unable to control color for {device}");
    }

    pub async fn device_list_scenes(&self, device: &Device) -> anyhow::Result<Vec<String>> {
        // TODO: some plumbing to maintain offline scene controls for preferred-LAN control

        if let Some(client) = self.get_platform_client().await {
            if let Some(info) = &device.http_device_info {
                return client.list_scene_names(info).await;
            }
        }

        anyhow::bail!("Unable to list scenes for {device}");
    }

    pub async fn device_set_scene(&self, device: &Device, scene: &str) -> anyhow::Result<()> {
        // TODO: some plumbing to maintain offline scene controls for preferred-LAN control

        if let Some(client) = self.get_platform_client().await {
            if let Some(info) = &device.http_device_info {
                client.set_scene_by_name(info, scene).await?;
                return Ok(());
            }
        }

        anyhow::bail!("Unable to set scene for {device}");
    }
}
