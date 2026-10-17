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
    // Fired by `Drop` when `arm_poll` is set (taken, then consumed by
    // the send), releasing the follow-up reconciling poll scheduled by
    // `resolve_device_for_control`. An un-armed drop closes the channel
    // instead and the parked poll task exits without polling.
    trigger_poll: Option<OneShotSender<()>>,
    arm_poll: bool,
}

impl Coordinator {
    pub fn new(
        device: Device,
        permit: OwnedSemaphorePermit,
        trigger_poll: OneShotSender<()>,
        arm_poll: bool,
    ) -> Self {
        Self {
            device,
            permit,
            trigger_poll: Some(trigger_poll),
            arm_poll,
        }
    }

    /// Ask the Coordinator to trigger the reconciling state poll when it
    /// is dropped, as if it had been taken with polling scheduled. Used
    /// when a command takes the fade path (whose entry permit schedules
    /// no poll of its own — the fade's final step owns the one
    /// reconciling poll) but the fade collapses to a single immediate
    /// brightness set, which owes the same poll any plain command gets.
    pub fn arm_reconciling_poll(&mut self) {
        self.arm_poll = true;
    }
}

impl Drop for Coordinator {
    fn drop(&mut self) {
        if self.arm_poll {
            // The value is irrelevant; the receiving task treats any
            // message (as opposed to a closed channel) as "poll now".
            if let Some(tx) = self.trigger_poll.take() {
                let _ = tx.send(());
            }
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
