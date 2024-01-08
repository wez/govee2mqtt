use crate::hass_mqtt::base::{Device, EntityConfig, Origin};
use crate::platform_api::DeviceCapability;
use crate::service::device::Device as ServiceDevice;
use crate::service::hass::{
    availability_topic, camel_case_to_space_separated, instance_from_topic,
    switch_instance_state_topic, topic_safe_id, HassClient,
};
use crate::service::state::StateHandle;
use serde::Serialize;

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
        let disco = state.get_hass_disco_prefix().await;
        let topic = format!(
            "{disco}/switch/{unique_id}/config",
            unique_id = self.base.unique_id
        );

        client.publish_obj(topic, self).await
    }

    pub async fn notify_state(
        &self,
        device: &ServiceDevice,
        client: &HassClient,
    ) -> anyhow::Result<()> {
        let instance = instance_from_topic(&self.command_topic).expect("topic to be valid");

        if instance == "powerSwitch" {
            if let Some(state) = device.device_state() {
                client
                    .publish(&self.state_topic, if state.on { "ON" } else { "OFF" })
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
                if cap.instance == instance {
                    log::warn!("SwitchConfig::notify_state: Do something with {cap:#?}");
                    return Ok(());
                }
            }
        }
        log::trace!("SelectConfig::notify_state: didn't find state for {device} {instance}");
        Ok(())
    }
}
