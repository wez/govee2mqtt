use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::hass_mqtt::instance::{publish_entity_config, EntityInstance};
use crate::platform_api::DeviceCapability;
use crate::service::device::Device as ServiceDevice;
use crate::service::hass::{
    availability_topic, camel_case_to_space_separated, switch_instance_state_topic, topic_safe_id,
    HassClient,
};
use crate::service::state::StateHandle;
use async_trait::async_trait;
use serde::Serialize;
use serde_json::json;

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

    pub async fn publish(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        publish_entity_config("switch", state, client, &self.base, self).await
    }
}

pub struct CapabilitySwitch {
    switch: SwitchConfig,
    device_id: String,
    state: StateHandle,
    instance_name: String,
}

impl CapabilitySwitch {
    pub async fn new(
        device: &ServiceDevice,
        state: &StateHandle,
        instance: &DeviceCapability,
    ) -> anyhow::Result<Self> {
        let switch = SwitchConfig::for_device(device, instance).await?;
        Ok(Self {
            switch,
            device_id: device.id.to_string(),
            state: state.clone(),
            instance_name: instance.instance.to_string(),
        })
    }
}

#[async_trait]
impl EntityInstance for CapabilitySwitch {
    async fn publish_config(&self, state: &StateHandle, client: &HassClient) -> anyhow::Result<()> {
        self.switch.publish(&state, &client).await
    }

    async fn notify_state(&self, client: &HassClient) -> anyhow::Result<()> {
        let device = self
            .state
            .device_by_id(&self.device_id)
            .await
            .expect("device to exist");

        if self.instance_name == "powerSwitch" {
            if let Some(state) = device.device_state() {
                client
                    .publish(
                        &self.switch.state_topic,
                        if state.on { "ON" } else { "OFF" },
                    )
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
                if cap.instance == self.instance_name {
                    match cap.state.pointer("/value").and_then(|v| v.as_i64()) {
                        Some(n) => {
                            return client
                                .publish(
                                    &self.switch.state_topic,
                                    if n != 0 { "ON" } else { "OFF" },
                                )
                                .await;
                        }
                        None => {
                            if cap.state.pointer("/value") == Some(&json!("")) {
                                log::trace!(
                                    "CapabilitySwitch::notify_state ignore useless \
                                            empty string state for {cap:?}"
                                );
                            } else {
                                log::warn!(
                                    "CapabilitySwitch::notify_state: Do something with {cap:#?}"
                                );
                            }
                            return Ok(());
                        }
                    }
                }
            }
        }
        log::trace!(
            "CapabilitySwitch::notify_state: didn't find state for {device} {instance}",
            instance = self.instance_name
        );
        Ok(())
    }
}
