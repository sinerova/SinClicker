//! Windows wake-up backend for the click scheduler: blocks the worker until the
//! next click deadline or a command, without busy-waiting and without changing
//! the system-wide timer resolution.
//!
//! This module holds *all* Windows interop and `unsafe` for the wait. Click
//! *timing* (deadlines, drift-free stepping) is pure and testable, and lives in
//! [`crate::clicker::ClickScheduler`]; this module only decides *when control
//! returns to the worker*.
//!
//! ## Wake strategy (a ladder, best first)
//!
//! The old `mpsc::recv_timeout` wait was quantized to the system timer tick
//! (a few milliseconds), which capped a 10 ms (100 CPS) interval at roughly
//! 60–65 clicks/second in measurement. A waitable timer wakes on the requested
//! deadline, which lifts that ceiling. Three options are tried at startup, in
//! order, and the first that succeeds is used. None change the global timer
//! resolution and none busy-wait.
//!
//! 1. **High-resolution waitable timer**
//!    (`CREATE_WAITABLE_TIMER_HIGH_RESOLUTION`, Windows 10 1607+): wakes on the
//!    deadline with sub-millisecond precision, so a 10 ms interval can reach
//!    the full 100 clicks/second.
//! 2. **Standard waitable timer**: wakes on the deadline but only to the
//!    system timer resolution (often a few milliseconds). A note is logged so
//!    a reduced high rate is never silently promised.
//! 3. **Plain bounded wait on the command event**: the pre-timer behaviour —
//!    the bounded wait *is* the deadline, and it is likewise quantized to the
//!    system timer resolution. Used only when no waitable timer can be created
//!    at all; the logged note names the limitation explicitly.
//!
//! ## Commands wake immediately
//!
//! A separate auto-reset **command event** is signalled by the worker handle on
//! every `Start`/`Stop`/`Toggle`/`SetCps`/`Quit`. The event is the *wake hint*;
//! the `mpsc` channel is only a payload pipe that the worker fully drains
//! after each wake. An auto-reset event coalesces signals into one wake, and a
//! wake can also be the deadline timer or the timeout bound; the worker
//! therefore never trusts the wake *kind* and always re-checks its own
//! deadline against a monotonic clock and drains the channel. A coalesced
//! signal or a spurious early wake can never drop a command or fire an
//! early click — a stale extra wake is harmless, a lost command is not, and the
//! drain closes that gap.

use std::time::Instant;

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{
    CreateEventW, CreateWaitableTimerExW, CreateWaitableTimerW, SetEvent, SetWaitableTimer,
    WaitForMultipleObjects, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS,
};

/// 100-nanosecond units in one microsecond (a Windows 100-ns tick; 1 µs = 10
/// ticks). Converts a microsecond delay into a `SetWaitableTimer` due time.
pub const HUNDRED_NANOS_PER_MICROSECOND: i64 = 10;

/// Upper bound, in microseconds, on any single wait. Nothing may block longer
/// than this; it guarantees the worker re-checks its state and can shut down
/// even if a deadline were armed far ahead.
pub const MAX_WAIT_MICROS: u64 = 5_000_000;

/// The wait the worker performs when clicking is stopped, before re-checking
/// for a command. Kept modest (2 s) so Start/Stop/Quit stay responsive while
/// idle; every command signal still wakes it immediately.
pub const IDLE_WAIT_MICROS: u64 = 2_000_000;

/// The wait bound, in microseconds, placed slightly beyond the precise timer
/// deadline so a marginally-late timer is still reported as the deadline being
/// reached (the 100-nanosecond unit truncation of the bound makes the actual
/// bound at least `delay + 1` µs, so the timer always precedes the bound).
const TIMER_SLACK_MICROS: u64 = 1_000;

/// A kernel event handle that may be copied. It is a kernel object with no
/// thread affinity. The owning [`WakeSource`] is the single close site (its
/// `Drop`); copies held by the worker handle are never closed and are only
/// signalled while the worker is alive, so no double-close occurs.
#[derive(Clone, Copy)]
pub struct CommandEvent(HANDLE);

// SAFETY: the event is a kernel object with no thread affinity (any thread may
// signal or wait on it). It is closed exactly once, in `WakeSource::Drop`,
// after the worker thread (its only waiter) has ended.
unsafe impl Send for CommandEvent {}

impl CommandEvent {
    /// Creates an auto-reset, initially-clear, anonymous event. Panics when the
    /// event cannot be created (an allocation failure of a kernel object is not
    /// recoverable here).
    fn new() -> Self {
        let event = unsafe {
            // SAFETY: null attributes and a null name create a plain,
            // non-inheritable, anonymous, auto-reset event usable from any
            // thread.
            CreateEventW(None, false, false, PWSTR::null())
        }
        .unwrap_or_else(|error| panic!("failed to create a click-scheduler event: {error:?}"));
        CommandEvent(event)
    }

    /// Signals the event, waking a worker blocked in [`WakeSource::wait_for`].
    /// Ignored if the event was already closed (the worker is shutting down).
    pub fn signal(&self) {
        let _ = unsafe {
            // SAFETY: `SetEvent` only affects the kernel event this handle
            // refers to.
            SetEvent(self.0)
        };
    }
}

/// Why a wait returned control to the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wake {
    /// The wait's time elapsing — the deadline timer fired (possibly already
    /// past), the command signal arrived, or the safety bound expired. The
    /// worker cannot tell which: it must re-check its own deadline against the
    /// monotonic clock and drain the command channel.
    Elapsed,
    /// A wait error occurred: stop clicking and report it. The worker must not
    /// spin trying to recover.
    Failed(String),
}

/// The wait strategy in use, for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerKind {
    /// High-resolution waitable timer (best wake precision).
    HighResolution,
    /// Standard waitable timer (wake limited to the system timer resolution).
    Standard,
    /// No waitable timer: bounded wait on the command event only (likewise
    /// limited to the system timer resolution).
    EventOnly,
}

impl TimerKind {
    /// A short human description, for logging.
    pub fn description(&self) -> &'static str {
        match self {
            TimerKind::HighResolution => "high-resolution waitable timer",
            TimerKind::Standard => {
                "standard waitable timer (wakes limited to the system timer resolution)"
            }
            TimerKind::EventOnly => {
                "plain bounded wait (no waitable timer; wakes limited to the system timer \
                 resolution)"
            }
        }
    }
}

/// Backend that blocks the worker until the next click deadline or a command.
///
/// One instance lives on the worker thread for the whole worker lifetime. The
/// command event is signalled by the worker handle on every command; the
/// waitable timer (when present) is armed to the current deadline on every
/// cycle, so a CPS change re-arms immediately and no stale deadline is ever
/// waited on.
///
/// The waitable timer is *auto-reset*, so a wait consumes its signal. Two
/// invariants keep that safe: `wait_for` re-arms the timer before every wait,
/// and the `WaitForMultipleObjects` bound always extends past the deadline, so
/// even a missed signal cannot keep the worker blocked past the deadline. The
/// worker's own `now >= deadline` check (against a monotonic clock) decides
/// whether a click fires, so no wake — command signal, timer, or timeout — can
/// cause an early click.
pub struct WakeSource {
    command_event: CommandEvent,
    timer: Option<HANDLE>,
    kind: TimerKind,
    /// Epoch of the monotonic deadline clock (see [`Self::now_micros`]).
    epoch: Instant,
}

// SAFETY: every field is either already `Send` (`CommandEvent` above,
// `TimerKind`) or a kernel object with no thread affinity (`HANDLE`). The
// owning instance is created on the UI thread and moved into the worker thread
// (`Worker::spawn`), where it lives for the thread's whole lifetime: the worker
// is the only thread that calls `wait_for`/`now_micros` or drops it, and no
// other thread touches the handles concurrently. So moving it across threads
// never creates a data race.
unsafe impl Send for WakeSource {}

impl WakeSource {
    /// Picks the best available wait strategy, trying each in order and using
    /// the first that succeeds. Never panics: if no waitable timer can be
    /// created it degrades to a bounded wait on the command event and a note is
    /// logged so the limitation is not silent.
    ///
    /// Called exactly once, from [`crate::clicker::Worker::spawn`], and owned
    /// by the worker thread for its whole life. There is deliberately no
    /// `Default` impl: constructing the backend allocates kernel objects, so
    /// an implicit `Default::default()` elsewhere would be a footgun.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let (timer, kind) = match create_high_resolution_timer() {
            Ok(handle) => (Some(handle), TimerKind::HighResolution),
            Err(high_res_error) => match create_standard_timer() {
                Ok(handle) => (Some(handle), TimerKind::Standard),
                Err(standard_error) => {
                    eprintln!(
                        "high-resolution waitable timer unavailable ({high_res_error}); \
                         standard waitable timer unavailable ({standard_error}); \
                         falling back to a plain bounded wait: the click rate is then limited \
                         by the system timer resolution (a few milliseconds), so the requested \
                         rate may not be reached — 100 CPS in particular will run well below \
                         its target. This does not change the system timer resolution."
                    );
                    (None, TimerKind::EventOnly)
                }
            },
        };

        // Any non-high-resolution strategy is bounded by the system timer
        // resolution; note it so a reduced rate is never silently promised.
        if kind != TimerKind::HighResolution {
            eprintln!(
                "click-scheduler wait strategy: {} — rates above roughly two to three clicks \
                 per system timer tick (often about 60/second) may run slower than requested",
                kind.description()
            );
        }

        WakeSource {
            command_event: CommandEvent::new(),
            timer,
            kind,
            epoch: Instant::now(),
        }
    }

    /// The command event handle, for the worker handle to signal it. The copy
    /// is never closed (the source's `Drop` is the single close site).
    pub fn command_event(&self) -> CommandEvent {
        self.command_event
    }

    /// The wait strategy in use, for diagnostics.
    pub fn timer_kind(&self) -> TimerKind {
        self.kind
    }

    /// Blocks until the next click deadline (`delay_micros` from now) or a
    /// command signal, or the safety bound, and reports the outcome. It always
    /// returns (never blocks unbounded) and never signals a click itself: it
    /// only re-enters the worker, which re-checks the deadline itself.
    ///
    /// When a waitable timer is present it is armed to that deadline and the
    /// wait bound is placed a little beyond it, so the (precise) timer always
    /// wins; when none is present the bounded wait itself *is* the deadline
    /// (the pre-timer behaviour).
    pub fn wait_for(&mut self, delay_micros: u64) -> Wake {
        let delay_micros = delay_micros.clamp(1, MAX_WAIT_MICROS);
        let (handles, bound_micros): (&[HANDLE], u64) = match self.kind {
            TimerKind::HighResolution | TimerKind::Standard => {
                let Some(timer) = self.timer else {
                    // Should not happen for these kinds; fail cleanly.
                    return Wake::Failed("no waitable timer available".to_owned());
                };
                if let Err(error) = self.arm_deadline(timer, delay_micros) {
                    return Wake::Failed(error);
                }
                (
                    &[timer, self.command_event.0],
                    delay_micros.saturating_add(TIMER_SLACK_MICROS),
                )
            }
            TimerKind::EventOnly => {
                // No timer: the bounded wait's timeout is the deadline.
                (&[self.command_event.0], delay_micros)
            }
        };
        // Millisecond precision is all `WaitForMultipleObjects` offers; the
        // division truncates, but `TIMER_SLACK_MICROS` (1 ms) keeps the bound
        // at or past the deadline in every strategy.
        let bound_ms = (bound_micros / 1000).clamp(1, MAX_WAIT_MICROS / 1000) as u32;
        let status = unsafe {
            // SAFETY: every handle is a live kernel object for the whole wait
            // (the event and any timer are closed only in `Drop`, at the end of
            // the worker thread); the slice stays valid for the duration of the
            // call.
            WaitForMultipleObjects(handles, false, bound_ms)
        }
        .0;
        // `WAIT_OBJECT_0` is the precise deadline timer (index 0); the command
        // event is index 1; `WAIT_TIMEOUT` is the safety bound. All three mean
        // "proceed and re-check": the worker compares its own deadline against
        // the clock and drains the channel, so a command-signal wake cannot
        // fire an early click. `WAIT_FAILED` and any other status (e.g.
        // `WAIT_ABANDONED`, unreachable for kernel waitable objects but not
        // trusted) end the clicker with a reported error instead of spinning.
        // The 0.62.2 `WAIT_EVENT` constants are not usable as patterns, so the
        // Win32 values are named here (verified in the crate source:
        // WAIT_OBJECT_0 = 0, WAIT_TIMEOUT = 258, WAIT_FAILED = u32::MAX).
        const STATUS_OBJECT_0: u32 = WAIT_OBJECT_0.0;
        const STATUS_OBJECT_1: u32 = WAIT_OBJECT_0.0.wrapping_add(1);
        const STATUS_TIMEOUT: u32 = WAIT_TIMEOUT.0;
        const STATUS_FAILED: u32 = WAIT_FAILED.0;
        match status {
            STATUS_OBJECT_0 | STATUS_OBJECT_1 | STATUS_TIMEOUT => Wake::Elapsed,
            STATUS_FAILED => {
                Wake::Failed("the click-scheduler wait was aborted by Windows".to_owned())
            }
            other => Wake::Failed(format!(
                "the click-scheduler wait returned an unexpected status {other}"
            )),
        }
    }

    /// Monotonic source for the scheduler's deadline math: microseconds since
    /// this backend was created.
    ///
    /// The clock is `std::time::Instant`, which on Windows is backed by the
    /// high-resolution, monotonic `QueryPerformanceCounter` (it is unaffected
    /// by wall-clock changes and does not roll back). `GetTickCount64` would
    /// be a tempting choice but is quantized to the system timer resolution
    /// (a few milliseconds): a deadline clock on it would make the
    /// `now >= deadline` test lag by up to a tick, capping a 10 ms (100 CPS)
    /// interval at roughly 60–65 clicks/second even with a precise timer.
    /// `Instant` avoids that quantization. It is also the same basis used for
    /// the wait's own deadline, so the clock and the wake are coherent.
    pub fn now_micros(&self) -> u64 {
        self.epoch.elapsed().as_micros() as u64
    }

    /// Arms the waitable timer to wake `delay_micros` from now.
    /// `SetWaitableTimer` takes the due time as a *negative* 100-ns value (a
    /// relative deadline); the timer is one-shot (period 0) and re-armed on
    /// every cycle. The caller guarantees `timer` came from
    /// [`create_high_resolution_timer`] or [`create_standard_timer`] (created
    /// with `TIMER_ALL_ACCESS`, which includes `TimerModifyState`).
    fn arm_deadline(&self, timer: HANDLE, delay_micros: u64) -> Result<(), String> {
        // `delay_micros` is already clamped to >= 1, so the due time is a
        // negative (relative) value.
        let due_100ns = -(delay_micros as i64) * HUNDRED_NANOS_PER_MICROSECOND;
        unsafe {
            // SAFETY: `timer` is a live waitable timer owned for the whole
            // worker lifetime (closed only in `Drop`, at the end of the worker
            // thread); the due time is a simple relative 100-ns value; period
            // 0 means one-shot; no completion routine.
            SetWaitableTimer(timer, &due_100ns, 0, None, None, false)
        }
        .map_err(|error| format!("the click-scheduler timer could not be set: {error:?}"))
    }
}

impl Drop for WakeSource {
    fn drop(&mut self) {
        if let Some(timer) = self.timer {
            let _ = unsafe {
                // SAFETY: the timer is owned by this object; this is the sole
                // close site and runs when the worker thread (its only user)
                // ends.
                CloseHandle(timer)
            };
        }
        let _ = unsafe {
            // SAFETY: the command event is owned by this object; this is the
            // sole close site. A worker-handle copy is never closed and is only
            // signalled while this object is alive.
            CloseHandle(self.command_event.0)
        };
    }
}

/// Creates a high-resolution waitable timer (Windows 10 1607+), reporting the
/// error so the caller can fall back to a standard timer.
fn create_high_resolution_timer() -> Result<HANDLE, String> {
    unsafe {
        // SAFETY: null attributes and a null name create a plain, anonymous,
        // non-inheritable waitable timer; the high-resolution flag requests
        // the high-resolution facility (unsupported on older systems, in which
        // case the call fails and the caller falls back); `TIMER_ALL_ACCESS`
        // lets this process set and close it.
        CreateWaitableTimerExW(
            None,
            PWSTR::null(),
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
            TIMER_ALL_ACCESS.0,
        )
    }
    .map_err(|error| format!("CreateWaitableTimerExW: {error:?}"))
}

/// Creates a standard (system-resolution) waitable timer, reporting the error.
fn create_standard_timer() -> Result<HANDLE, String> {
    unsafe {
        // SAFETY: null attributes and a null name; the timer is auto-reset and
        // re-armed one-shot on every cycle, so the reset behaviour does not
        // lose a deadline (the wait bound always covers it).
        CreateWaitableTimerW(None, false, PWSTR::null())
    }
    .map_err(|error| format!("CreateWaitableTimerW: {error:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Bounded, hang-proof exercise of the real wait path on this machine. No
    /// mouse input is sent and no hotkey is registered: it creates the
    /// backend, performs a short bounded wait (which must return for a 150 ms
    /// deadline), and separately proves the command signal wakes a long wait
    /// promptly (what keeps Stop/Quit responsive).
    #[test]
    fn a_short_wait_is_bounded_and_a_command_wakes_the_wait() {
        let mut src = WakeSource::new();
        let started = std::time::Instant::now();
        assert_eq!(src.wait_for(150_000), Wake::Elapsed); // 150 ms deadline
        let elapsed = started.elapsed();
        assert!(
            (140..=1_000).contains(&elapsed.as_millis()),
            "a 150 ms wait should elapse within its bound, took {elapsed:?}"
        );
        drop(src);

        // A long-deadline wait must be woken promptly by the command signal.
        let mut long_src = WakeSource::new();
        let signal_handle = long_src.command_event();
        let wait_thread = std::thread::spawn(move || long_src.wait_for(2_000_000)); // 2 s
        let signaled_at = {
            // Give the wait thread a moment to enter the wait; a 60 ms sleep
            // is generous against thread startup and keeps the test fast.
            std::thread::sleep(Duration::from_millis(60));
            signal_handle.signal();
            std::time::Instant::now()
        };
        let result = wait_thread.join().expect("the wait thread must not panic");
        assert_eq!(result, Wake::Elapsed);
        let after_signal = std::time::Instant::now().duration_since(signaled_at);
        let total = started.elapsed();
        assert!(
            after_signal.as_millis() < 500,
            "the command signal should wake a long wait promptly; {after_signal:?} after the signal, total {total:?}",
        );
    }

    /// The backend selects a real waitable timer on a modern Windows machine.
    #[test]
    fn the_backend_selects_a_waitable_timer_here() {
        let wake = WakeSource::new();
        assert!(
            matches!(
                wake.timer_kind(),
                TimerKind::HighResolution | TimerKind::Standard
            ),
            "expected a waitable timer on this machine, got {:?}",
            wake.timer_kind()
        );
    }

    /// A CPS change or Quit must be able to wake promptly in every strategy,
    /// because the command event is what wakes the wait (the channel is only a
    /// payload pipe). This proves responsiveness of the wake mechanism without
    /// sending input.
    #[test]
    fn the_command_signal_is_immediate_independent_of_deadline() {
        for _ in 0..5 {
            let mut src = WakeSource::new();
            let handle = src.command_event();
            let thread = std::thread::spawn(move || src.wait_for(2_000_000)); // 2 s
            std::thread::sleep(Duration::from_millis(15));
            handle.signal();
            let woke = thread.join().expect("wait thread must not panic") == Wake::Elapsed;
            assert!(woke, "signal did not wake the wait");
        }
    }

    /// Measures the real wake mechanism at the full 10 ms (100 CPS) interval,
    /// without sending any input. This is exactly what the old
    /// `recv_timeout` scheduler failed at, timed end to end on the actual
    /// backend in use.
    ///
    /// It is bounded and cannot hang: the window is fixed at one second of
    /// elapsed monotonic time, and every `wait_for` call either returns on its
    /// own deadline, a command signal, or the `MAX_WAIT_MICROS` safety bound,
    /// so the loop always makes forward progress and stops when the window
    /// elapses. (On this machine a 10 ms deadline returns in ~10 ms, so the
    /// probe takes about one second of wall time.)
    ///
    /// The assertion is a soft floor (90% of the requested 100/s): a
    /// high-resolution timer should wake near 100/s; a standard timer (whose
    /// wakes are quantized to the system timer resolution) is expected to fall
    /// short — that is the documented, logged limitation of that strategy, and
    /// a failure here on such a machine is informative about the hardware, not
    /// a regression in the scheduling math.
    #[test]
    fn the_wait_backend_meets_most_of_a_100cps_interval() {
        const PROBE_US: u64 = 1_000_000; // measure over a one-second window
        const INTERVAL_US: u64 = 10_000; // 10 ms interval -> 100 CPS target
        let mut src = WakeSource::new();
        let window_start = src.now_micros();
        let mut next_due = window_start; // first deadline is immediate
        let mut cycles = 0u32;
        let start = std::time::Instant::now();
        loop {
            let now = src.now_micros();
            // Stop once the one-second window has elapsed.
            if now.saturating_sub(window_start) >= PROBE_US {
                break;
            }
            if now >= next_due {
                cycles += 1;
                next_due = next_due.saturating_add(INTERVAL_US);
                // Never emit a catch-up burst: roll any lapsed deadlines
                // forward instead of firing them one by one.
                while next_due <= src.now_micros() {
                    next_due = next_due.saturating_add(INTERVAL_US);
                }
            } else {
                assert_eq!(
                    src.wait_for(next_due - now),
                    Wake::Elapsed,
                    "the wait must not fail under normal operation"
                );
            }
        }
        let took = start.elapsed().as_secs_f64();
        let effective = cycles as f64 / took;
        eprintln!(
            "scheduler-wake probe: {kind:?}, {cycles} cycles in {took:.2?} s \
             ({effective:.1} wake/s at a 10 ms target)",
            kind = src.timer_kind()
        );
        assert!(
            effective >= 90.0,
            "a high-resolution timer should wake near 100/s; measured {effective:.1} wake/s \
             ({cycles} in {took:.2?} s)"
        );
    }
}
