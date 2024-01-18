use crate::service::device::Device;
use tokio::sync::oneshot::Sender as OneShotSender;
use tokio::sync::OwnedSemaphorePermit;

/// The Coordinator ensures that only one task at a time can
/// be processing requests that control or otherwise change
/// the state of a device.
/// We do this to avoid strangess if multiple concurrent
/// mqtt or http requests are coming in concurrently for
/// the same device; while our data structures are ok with
/// that, it is best for overall coherence if we serialize
/// control attempts because eg: HASS can bundle several
/// stages of control into a single request, which have
/// to be sent to the device separately. If we allowed
/// those to interleave with other requests, it would
/// be rather chaotic.
pub struct Coordinator {
    device: Device,

    // These fields are not unused; we are keeping them
    // alive until we drop at which point they release
    // resources and/or trigger follow up work in other tasks.
    #[allow(unused)]
    permit: OwnedSemaphorePermit,
    #[allow(unused)]
    trigger_poll: OneShotSender<()>,
}

impl Coordinator {
    pub fn new(
        device: Device,
        permit: OwnedSemaphorePermit,
        trigger_poll: OneShotSender<()>,
    ) -> Self {
        Self {
            device,
            permit,
            trigger_poll,
        }
    }
}

impl std::ops::Deref for Coordinator {
    type Target = Device;

    fn deref(&self) -> &Device {
        &self.device
    }
}

impl std::fmt::Display for Coordinator {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        self.device.fmt(fmt)
    }
}
