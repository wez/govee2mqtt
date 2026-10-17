//! Software fade ("transition") support for lights without native fade.
//!
//! Home Assistant sends `transition` (in seconds, per the light entity
//! schema) on `light.turn_on`. Govee's cloud control API has no fade
//! primitive, so a fade is emulated bridge-side as a sequence of
//! intermediate brightness commands, paced within Govee's documented
//! control budget for `/router/api/v1/device/control`
//! (2 requests/second/device sustained, burst 6, and 12 requests/second
//! per account, burst 80; see
//! <https://developer.govee.com/reference/control-you-devices>).
//!
//! The planner here is a pure function so that every scheduling decision
//! is testable without a device: a fade is a fixed schedule computed when
//! the command arrives, and the runner in `service::state` only sleeps to
//! the planned timestamps and calls the existing brightness setter per
//! step. A newer command supersedes an in-progress fade (latest command
//! wins; supersession happens under the device's control permit, and the
//! runner re-checks its cancellation token under that same permit, so a
//! queued step of a superseded fade is never emitted after the newer
//! command has been sent), and a fade schedules a single reconciling poll
//! for its final step, so its total poll cost is the same as one manual
//! command.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Minimum interval between consecutive brightness commands issued to the
/// same device. Govee documents a per-device control budget of
/// 2 requests/second (burst 6); pacing at one step per second leaves half
/// of that budget free for interleaved power/color commands and periodic
/// status polling. This floor is applied to all fades, including fades of
/// LAN-capable devices, so that a fade has the same visible pacing and the
/// same request accounting regardless of which transport carries it.
pub const MIN_STEP_INTERVAL: Duration = Duration::from_millis(1000);

/// Minimum interval between any two fade steps across ALL concurrently
/// fading devices. Govee documents an account-wide control budget of
/// 12 requests/second (burst 80); with each device already limited to one
/// step per second by [`MIN_STEP_INTERVAL`], this global floor matters
/// only when more than ~10 devices fade at once. When it does bite, step
/// timestamps slip slightly (never the ordering, and never the final
/// step's value); see `docs/CONFIG.md`.
pub const GLOBAL_STEP_FLOOR: Duration = Duration::from_millis(100);

/// Serialize fade step emissions across all fades so the account-wide
/// request rate stays under Govee's documented 12 requests/second account
/// budget. Holds no device permit of its own; callers call this while
/// holding the per-device coordinator.
pub async fn global_step_gate() {
    static GATE: std::sync::OnceLock<tokio::sync::Mutex<tokio::time::Instant>> =
        std::sync::OnceLock::new();
    let gate = GATE
        .get_or_init(|| tokio::sync::Mutex::new(tokio::time::Instant::now() - GLOBAL_STEP_FLOOR));
    let mut last = gate.lock().await;
    let due = *last + GLOBAL_STEP_FLOOR;
    let now = tokio::time::Instant::now();
    if due > now {
        tokio::time::sleep_until(due).await;
    }
    *last = tokio::time::Instant::now();
}

/// Cap on the number of commands a single fade may generate, so that a
/// pathological transition duration cannot generate an unbounded queue.
pub const MAX_FADE_STEPS: u64 = 1000;

/// Longest fade duration we honor; longer requested durations are clamped.
pub const MAX_FADE_DURATION: Duration = Duration::from_secs(3600);

/// One step of a computed fade schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FadeStep {
    /// Time since the fade was requested at which this step should be
    /// sent to the device.
    pub at: Duration,
    /// Brightness (0-100) to command at `at`.
    pub brightness: u8,
}

/// A live fade's cancellation token. The runner checks it before each
/// step and again under the device control permit; whoever supersedes
/// the fade flips the `superseded` flag (the runner may mark itself
/// `finished` around its final step, which must not be confused with
/// supersession). Once superseded, the runner stops at its next wake-up
/// without emitting any further commands, so superseded targets are
/// coalesced away rather than queued. The token also tracks the last
/// brightness the runner actually emitted, so that a fade retargeted
/// mid-flight continues from what the device is actually being told to
/// show rather than from a possibly stale reported state.
#[derive(Debug)]
pub struct FadeToken {
    superseded: AtomicBool,
    finished: AtomicBool,
    /// Set with `mark_finished` and swapped out by whichever
    /// `poll_after_control` lands first after completion: the winner
    /// performs the fade's single reconciling poll and retires the
    /// registration.
    poll_pending: AtomicBool,
    emitted: AtomicU8,
}

impl FadeToken {
    pub fn new(start: u8) -> Arc<Self> {
        Arc::new(Self {
            superseded: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            poll_pending: AtomicBool::new(false),
            emitted: AtomicU8::new(start),
        })
    }

    /// Mark the fade as superseded by a newer control command.
    pub fn mark_superseded(&self) {
        self.superseded.store(true, Ordering::SeqCst);
    }

    pub fn is_superseded(&self) -> bool {
        self.superseded.load(Ordering::SeqCst)
    }

    /// Mark that this runner has entered its final step (state polling
    /// for the device is no longer suppressed while it lands). Also
    /// arms the reconciling-poll claim: exactly one `poll_after_control`
    /// landing after this will perform the fade's single reconciling
    /// poll and retire the registration.
    pub fn mark_finished(&self) {
        self.finished.store(true, Ordering::SeqCst);
        self.poll_pending.store(true, Ordering::SeqCst);
    }

    /// Claim the right to perform this finished fade's reconciling poll.
    /// Returns `true` to exactly one caller (the first to land); everyone
    /// else must defer, since the fade's state is being reconciled (or
    /// already was) by that poll.
    pub fn claim_reconcile(&self) -> bool {
        self.poll_pending.swap(false, Ordering::SeqCst)
    }

    /// Undo a `claim_reconcile` when the claimed poll failed to land:
    /// the next parked poll retries the retire, so continuity keeps
    /// winning over an unrefreshed reported state until a poll succeeds.
    pub fn restore_reconcile(&self) {
        self.poll_pending.store(true, Ordering::SeqCst);
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    /// Record a brightness that was actually sent to the device.
    pub fn note_emitted(&self, brightness: u8) {
        self.emitted.store(brightness, Ordering::SeqCst);
    }

    /// The last brightness this fade emitted (or the planned start, until
    /// the first step lands).
    pub fn emitted(&self) -> u8 {
        self.emitted.load(Ordering::SeqCst)
    }
}

/// Linearly interpolate between two brightness values at time `at_ms`
/// within a `duration_ms` window, rounding half away from zero (so the
/// schedule of a fade-down is the exact mirror of the same fade run
/// upward between the same two values) and clamping to 0..=100.
fn interp(start: u8, target: u8, at_ms: u64, duration_ms: u64) -> u8 {
    let delta = i64::from(target) - i64::from(start);
    let scaled = delta * at_ms as i64;
    let half = duration_ms as i64 / 2;
    let magnitude = (scaled.abs() + half) / duration_ms as i64;
    let value = if scaled < 0 {
        i64::from(start) - magnitude
    } else {
        i64::from(start) + magnitude
    };
    value.clamp(0, 100) as u8
}

/// Compute a brightness fade schedule from `start` to `target` over
/// `duration`.
///
/// Invariants:
/// * The result is never empty and its last step is always exactly
///   `target`, so callers can unconditionally issue the final value.
/// * When a fade cannot meaningfully happen (target equals start, or the
///   duration is shorter than a single command spacing), the schedule
///   collapses to one immediate step, reproducing the pre-fade behavior
///   of exactly one brightness command.
/// * No two consecutive steps carry the same brightness (a fade never
///   sends the same value twice back to back), and a leading step that
///   repeats the device's current brightness is dropped.
/// * Consecutive steps are spaced by at least `MIN_STEP_INTERVAL`, which
///   keeps the fade inside the cloud API's per-device request budget.
/// * `plan_fade(a, b, d)` is the exact mirror of `plan_fade(b, a, d)`:
///   step `i` of the two schedules sums to `a + b`.
pub fn plan_fade(start: u8, target: u8, duration: Duration) -> Vec<FadeStep> {
    let duration = duration.min(MAX_FADE_DURATION);

    if target == start || duration < MIN_STEP_INTERVAL {
        return vec![FadeStep {
            at: Duration::ZERO,
            brightness: target,
        }];
    }

    let duration_ms = duration.as_millis() as u64;
    let min_step_ms = MIN_STEP_INTERVAL.as_millis() as u64;
    let num_steps = (duration_ms / min_step_ms).clamp(1, MAX_FADE_STEPS);

    let mut steps: Vec<FadeStep> = Vec::with_capacity(num_steps as usize);
    for i in 1..=num_steps {
        let at_ms = duration_ms * i / num_steps;
        steps.push(FadeStep {
            at: Duration::from_millis(at_ms),
            brightness: interp(start, target, at_ms, duration_ms),
        });
    }
    // The final step is the contract with Home Assistant: it must be the
    // exact commanded value, at the exact requested duration,
    // irrespective of rounding in the interpolated steps above.
    if let Some(last) = steps.last_mut() {
        last.brightness = target;
    }

    // Coalesce consecutive steps with the same brightness, keeping the
    // later timestamp so the remaining pace stays as even as possible.
    let mut coalesced: Vec<FadeStep> = Vec::with_capacity(steps.len());
    for step in steps {
        match coalesced.last_mut() {
            Some(prev) if prev.brightness == step.brightness => {
                *prev = step;
            }
            _ => coalesced.push(step),
        }
    }

    // A first step that repeats the brightness the device already
    // reports is a wasted command; drop it unless it is also the final
    // (and therefore load-bearing) step.
    if coalesced.len() > 1 && coalesced[0].brightness == start {
        coalesced.remove(0);
    }

    coalesced
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    fn brightnesses(steps: &[FadeStep]) -> Vec<u8> {
        steps.iter().map(|s| s.brightness).collect()
    }

    #[test]
    fn fade_to_same_value_collapses_to_single_immediate_step() {
        assert_eq!(
            plan_fade(50, 50, Duration::from_secs(10)),
            vec![FadeStep {
                at: Duration::ZERO,
                brightness: 50
            }]
        );
    }

    #[test]
    fn fade_shorter_than_one_step_interval_collapses_to_immediate_set() {
        assert_eq!(
            plan_fade(10, 90, Duration::from_millis(500)),
            vec![FadeStep {
                at: Duration::ZERO,
                brightness: 90
            }]
        );
    }

    #[test]
    fn fade_produces_evenly_spaced_monotonic_steps() {
        let steps = plan_fade(10, 90, Duration::from_secs(4));
        assert_eq!(
            steps,
            vec![
                FadeStep {
                    at: ms(1000),
                    brightness: 30
                },
                FadeStep {
                    at: ms(2000),
                    brightness: 50
                },
                FadeStep {
                    at: ms(3000),
                    brightness: 70
                },
                FadeStep {
                    at: ms(4000),
                    brightness: 90
                },
            ]
        );
    }

    #[test]
    fn fade_down_is_the_exact_mirror_of_fade_up() {
        // Rounding is half-away-from-zero on the delta, so a fade down
        // visits exactly the values the same fade run upward would: step
        // i of the two schedules sums to start + target.
        assert_eq!(
            brightnesses(&plan_fade(90, 10, Duration::from_secs(4))),
            vec![70, 50, 30, 10]
        );
        assert_eq!(
            brightnesses(&plan_fade(80, 20, Duration::from_secs(3))),
            vec![60, 40, 20]
        );
        assert_eq!(
            brightnesses(&plan_fade(20, 80, Duration::from_secs(3))),
            vec![40, 60, 80]
        );
    }

    #[test]
    fn fade_up_and_down_are_pairwise_symmetric() {
        for (a, b, secs) in [
            (10u8, 90u8, 4u64),
            (0, 30, 7),
            (80, 20, 3),
            (1, 13, 7),
            (3, 100, 11),
            (0, 2, 10),
            (55, 46, 9),
            (7, 63, 5),
            (99, 4, 13),
            (20, 80, 3),
            (0, 100, 30),
        ] {
            let up = plan_fade(a, b, Duration::from_secs(secs));
            let down = plan_fade(b, a, Duration::from_secs(secs));
            assert_eq!(up.len(), down.len(), "{a}->{b}/{secs}s vs mirror");
            for (u, d) in up.iter().zip(&down) {
                assert_eq!(
                    u16::from(u.brightness) + u16::from(d.brightness),
                    u16::from(a) + u16::from(b),
                    "{a}->{b}/{secs}s vs mirror: {u:?} + {d:?}"
                );
            }
        }
    }

    #[test]
    fn fade_up_from_off_starts_at_zero_and_never_flashes_up_first() {
        // With start=0 (light is off) the ramp begins small and grows;
        // it cannot "flash on at the remembered brightness".
        assert_eq!(
            brightnesses(&plan_fade(0, 30, Duration::from_secs(7))),
            vec![4, 9, 13, 17, 21, 26, 30]
        );
    }

    #[test]
    fn final_step_is_always_exact_target_at_exact_duration() {
        // A duration that does not divide evenly by the step count.
        let steps = plan_fade(1, 13, Duration::from_secs(7));
        assert_eq!(steps.last().unwrap().brightness, 13);
        assert_eq!(steps.last().unwrap().at, Duration::from_secs(7));
        // Times are non-decreasing and brightness moves monotonically
        // toward the target.
        for pair in steps.windows(2) {
            assert!(pair[0].at < pair[1].at);
            assert!(pair[0].brightness <= pair[1].brightness);
        }
    }

    #[test]
    fn no_consecutive_duplicate_brightnesses() {
        // 0 -> 2 over 10s: many steps round to the same value. The plan
        // emits each value once (at the end of the interval where it is
        // the visually correct value) and drops the initial repeat of
        // the current brightness.
        let steps = plan_fade(0, 2, Duration::from_secs(10));
        assert_eq!(
            steps,
            vec![
                FadeStep {
                    at: ms(7000),
                    brightness: 1
                },
                FadeStep {
                    at: ms(10000),
                    brightness: 2
                },
            ]
        );
    }

    #[test]
    fn fade_down_is_monotonic_decreasing() {
        let steps = plan_fade(80, 20, Duration::from_secs(3));
        assert_eq!(steps.last().unwrap().brightness, 20);
        for pair in steps.windows(2) {
            assert!(pair[0].brightness >= pair[1].brightness);
        }
    }

    #[test]
    fn step_spacing_never_faster_than_min_interval() {
        let steps = plan_fade(0, 100, Duration::from_secs(30));
        assert_eq!(steps.len(), 30);
        for pair in steps.windows(2) {
            assert!(pair[1].at - pair[0].at >= MIN_STEP_INTERVAL);
        }
    }

    #[test]
    fn duration_and_step_count_are_clamped() {
        // 2 hours requests one command per second: that would be 7200
        // commands; the duration clamps to 1 hour and the command count
        // is further bounded by MAX_FADE_STEPS (and by coalescing: only
        // the 100 distinct brightness values actually need a command).
        let steps = plan_fade(0, 100, Duration::from_secs(7200));
        assert!(steps.len() <= MAX_FADE_STEPS as usize);
        assert_eq!(steps.len(), 100);
        assert_eq!(steps.last().unwrap().at, MAX_FADE_DURATION);
        assert_eq!(steps.last().unwrap().brightness, 100);
    }

    #[tokio::test]
    async fn plan_and_token_contract_emit_on_schedule_and_stop_when_cancelled() {
        // Exercises the schedule + token contract the runner depends on,
        // driven through the same sleep_until composition. (The runner
        // itself — permit handling, supersession under the permit, poll
        // budget — is covered by the fade runtime tests in
        // service::hass.)
        let token = FadeToken::new(0);
        let steps = plan_fade(0, 50, Duration::from_secs(2));
        assert_eq!(steps.len(), 2);

        let emitted = Arc::new(std::sync::Mutex::new(Vec::<(u128, u8)>::new()));
        let started = tokio::time::Instant::now();
        let (emitted_inner, token_inner) = (Arc::clone(&emitted), Arc::clone(&token));
        let runner = tokio::spawn(async move {
            for step in steps {
                tokio::time::sleep_until(started + step.at).await;
                if token_inner.is_superseded() {
                    return;
                }
                token_inner.note_emitted(step.brightness);
                emitted_inner
                    .lock()
                    .unwrap()
                    .push((started.elapsed().as_millis(), step.brightness));
            }
        });

        // Supersede just after the first step lands; the loop must not
        // emit anything further.
        tokio::time::sleep(Duration::from_millis(1200)).await;
        token.mark_superseded();
        runner.await.unwrap();

        let emitted = emitted.lock().unwrap();
        assert_eq!(emitted.len(), 1, "superseded fade emitted {emitted:?}");
        // First step planned at t=1000ms; allow coarse-scheduler slack.
        assert!(
            emitted[0].0 >= 1000 && emitted[0].0 <= 1200,
            "step landed at {}ms",
            emitted[0].0
        );
        assert_eq!(emitted[0].1, 25);
        // The token remembers what was actually sent, for retargeting.
        assert_eq!(token.emitted(), 25);
    }
}
