//! The click scheduler: pure, testable click cadence (deadlines) plus the worker
//! thread that actually sends clicks and services commands.
//!
//! The timing here is *pure* — no Windows, no I/O — so it can be unit-tested
//! without ever sending input. The Windows wake mechanism (the thing that blocks
//! until a deadline or a command) lives in [`crate::win_scheduler::WakeSource`]
//! and is driven by the worker.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::win_input::left_mouse_down_up;
use crate::win_scheduler::{CommandEvent, Wake, WakeSource, IDLE_WAIT_MICROS};

pub const MIN_CPS: i32 = 1;
pub const MAX_CPS: i32 = 500;
pub const DEFAULT_CPS: i32 = 10;

/// Clamps an arbitrary CPS value into the supported 1..=500 range.
///
/// This enforces the spec range in Rust even though the UI SpinBox already
/// restricts it.
pub fn validate_cps(cps: i32) -> i32 {
    // MIN_CPS (1) < MAX_CPS (500), so clamp cannot panic.
    cps.clamp(MIN_CPS, MAX_CPS)
}

/// Returns the time to wait between clicks for a CPS value that has been
/// clamped (or is already in range) to 1..=500. A CPS of 1 waits 1 second;
/// a CPS of 500 waits 2 milliseconds.
pub fn click_interval(cps: i32) -> Duration {
    Duration::from_secs_f64(1.0 / validate_cps(cps) as f64)
}

/// Microsecond interval between clicks for a CPS value (clamped into range).
/// The pure deadline math works in whole microseconds, so `cps * interval`
/// equals one second within integer rounding.
pub fn click_interval_micros(cps: i32) -> u64 {
    1_000_000 / validate_cps(cps) as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Start,
    Stop,
    Toggle,
    SetCps(i32),
    Quit,
}

/// State reports emitted by the worker back toward the UI thread.
#[derive(Debug, Clone)]
pub enum WorkerEvent {
    Started,
    Stopped,
    Error(String),
}

/// A cheap, cloneable command sender for the click-scheduler worker.
///
/// Command sending is intentionally fire-and-forget: if the worker has already
/// exited (the application is shutting down) the send fails silently. Each send
/// also signals the worker's command event so the worker wakes promptly,
/// even if it is blocked in a long wait.
#[derive(Clone)]
pub struct WorkerHandle {
    tx: Sender<Command>,
    command_event: CommandEvent,
}

impl WorkerHandle {
    fn send(&self, command: Command) {
        if self.tx.send(command).is_err() {
            // The worker thread is gone (the application is shutting down).
            return;
        }
        // Wake the worker promptly. Safe even if the event has just been
        // closed (shut down): `signal` ignores the failure.
        self.command_event.signal();
    }

    /// Starts clicking. Starting while already running is ignored.
    pub fn start(&self) {
        self.send(Command::Start);
    }

    /// Stops clicking. Stopping while already stopped is ignored.
    pub fn stop(&self) {
        self.send(Command::Stop);
    }

    /// Toggles clicking on/off. The running state lives on the worker thread,
    /// so the toggle is authoritative there; the UI is synced from the
    /// `Started`/`Stopped` reports the worker emits as a result.
    pub fn toggle(&self) {
        self.send(Command::Toggle);
    }

    /// Updates the click rate. Takes effect on the next scheduled click when
    /// the worker is running; otherwise it is stored for the next start.
    pub fn set_cps(&self, cps: i32) {
        self.send(Command::SetCps(validate_cps(cps)));
    }

    /// Asks the worker to stop and terminate.
    pub fn quit(&self) {
        self.send(Command::Quit);
    }
}

/// Owns the click-scheduler worker thread.
///
/// The worker runs on its own thread and sends left-click down/up pairs with
/// `SendInput` at the current pointer position. It blocks (never busy-waits)
/// on a waitable timer until the next click deadline, and a command event wakes
/// it promptly for `Start`/`Stop`/`SetCps`/`Quit`. State changes are reported
/// through the `report` closure given to [`Worker::spawn`].
pub struct Worker {
    handle: WorkerHandle,
    thread: JoinHandle<()>,
}

impl Worker {
    /// Spawns the worker with `default_cps` (clamped into range). The
    /// `report` closure is invoked on the worker thread whenever a state
    /// change occurs (`Started`, `Stopped`, or `Error`). It is `Send +
    /// 'static` because it moves to the worker thread.
    pub fn spawn(default_cps: i32, report: impl Fn(WorkerEvent) + Send + 'static) -> Worker {
        let (tx, rx) = channel();
        // The wake backend is created here (kernel objects have no thread
        // affinity) and moved into the worker thread; its `Drop` on that
        // thread is the single close site for the timer/event handles. The
        // worker handle keeps only a copy of the command event, which it
        // signals but never closes.
        let source = WakeSource::new();
        let handle = WorkerHandle {
            tx,
            command_event: source.command_event(),
        };
        let thread = std::thread::Builder::new()
            .name("sinclicker-worker".to_owned())
            .spawn(move || worker_loop(source, rx, report, default_cps))
            .expect("failed to spawn the click-scheduler worker thread");
        Worker { handle, thread }
    }

    pub fn handle(&self) -> &WorkerHandle {
        &self.handle
    }

    /// Stops clicking, requests termination, and joins the worker thread.
    /// Joining is the practical way to guarantee the worker fully stops before
    /// the application exits. Consuming `self` is intentional: the worker is
    /// never restarted.
    pub fn shutdown(self) {
        self.handle.stop();
        self.handle.quit();
        if let Err(error) = self.thread.join() {
            eprintln!("failed to join the click worker thread: {error:?}");
        }
    }
}

/// Runs on the worker thread: clicks at the current CPS when running and
/// services `Start`/`Stop`/`Toggle`/`SetCps`/`Quit` commands.
///
/// Each cycle either fires a due click and then blocks, or blocks directly.
/// Blocking uses the wake backend ([`WakeSource`]), which returns on the click
/// deadline, on a command signal (Stop/Toggle/SetCps/Quit always arrive with a
/// signal, so they wake promptly), or on a wait error. Commands are drained
/// (non-blocking, fully) after every wake, so no command is ever lost even
/// though the auto-reset event may coalesce signals.
fn worker_loop(
    mut wake: WakeSource,
    command_rx: Receiver<Command>,
    report: impl Fn(WorkerEvent),
    default_cps: i32,
) {
    let mut scheduler = ClickScheduler::new(default_cps);
    let mut running = false;

    loop {
        let now = wake.now_micros();
        let wait = scheduler.wait_us(running, now);
        if wait == 0 {
            // A click is due now (this is only reachable while running).
            if let Err(error) = left_mouse_down_up() {
                // Never report a failed click: surface the failure through the
                // UI and end the worker (no further clicks are sent).
                report(WorkerEvent::Error(error.message));
                break;
            }
            scheduler.advance(now);
            continue;
        }

        // Not due (or stopped): block until the next deadline or a command.
        if let Wake::Failed(error) = wake.wait_for(wait) {
            // A wait failure ends the worker with a report; it must not spin
            // trying to recover.
            report(WorkerEvent::Error(error));
            break;
        }

        // `Wake::Elapsed`: the wake's cause (deadline timer, command signal, or
        // the safety bound) is not trusted; the loop top re-checks the
        // deadline against the monotonic clock.

        // Service any commands this wake (timer or command signal). `Quit` ends
        // the loop; a `Stop` simply makes the next cycle take the idle wait.
        if drain_commands(
            &command_rx,
            &mut running,
            &mut scheduler,
            wake.now_micros(),
            &report,
        ) {
            break;
        }
    }
}

/// Drains the command channel, applying each command. Returns `true` when a
/// `Quit` was among them (the worker should exit), `false` otherwise.
fn drain_commands(
    command_rx: &Receiver<Command>,
    running: &mut bool,
    scheduler: &mut ClickScheduler,
    now: u64,
    report: &impl Fn(WorkerEvent),
) -> bool {
    let mut quit = false;
    while let Ok(command) = command_rx.try_recv() {
        match command {
            Command::Start => {
                if !*running {
                    *running = true;
                    scheduler.start(now);
                    report(WorkerEvent::Started);
                }
            }
            Command::Stop => {
                if *running {
                    *running = false;
                    scheduler.stop();
                    report(WorkerEvent::Stopped);
                }
            }
            Command::Toggle => {
                if *running {
                    *running = false;
                    scheduler.stop();
                    report(WorkerEvent::Stopped);
                } else {
                    *running = true;
                    scheduler.start(now);
                    report(WorkerEvent::Started);
                }
            }
            Command::SetCps(value) => {
                scheduler.set_cps(value, *running, now);
            }
            Command::Quit => {
                quit = true;
                *running = false;
                scheduler.stop();
            }
        }
    }
    quit
}

/// The pure, testable cadence core.
///
/// It keeps an absolute rolling deadline (in monotonic microseconds) so that:
/// - the click rate does not drift from re-anchoring on jittery "now" values,
/// - a missed deadline (e.g. the machine was suspended) is not replayed as a
///   burst of catch-up clicks,
/// - a CPS change applies from the next click without a restart, moving a
///   pending deadline earlier when the new interval is shorter.
#[derive(Debug, Clone)]
pub struct ClickScheduler {
    interval_us: u64,
    next_deadline_us: Option<u64>,
}

impl ClickScheduler {
    /// Builds a scheduler for `cps` (clamped into range). Nothing is running
    /// until [`start`](Self::start) arms the first deadline.
    pub fn new(cps: i32) -> Self {
        ClickScheduler {
            interval_us: click_interval_micros(cps),
            next_deadline_us: None,
        }
    }

    /// The current interval between clicks, in microseconds.
    pub fn interval_us(&self) -> u64 {
        self.interval_us
    }

    /// The armed next-click deadline, in monotonic microseconds (`None` when
    /// not running).
    pub fn next_deadline_us(&self) -> Option<u64> {
        self.next_deadline_us
    }

    /// Arms the first deadline for a start/transition to running. The deadline
    /// is exactly `now`, so the next [`wait_us`](Self::wait_us) is `0`: the
    /// first click fires immediately on start.
    pub fn start(&mut self, now: u64) {
        self.next_deadline_us = Some(now);
    }

    /// Clears the deadline on a transition to stopped.
    pub fn stop(&mut self) {
        self.next_deadline_us = None;
    }

    /// Updates the interval for `cps` (clamped). When running, the pending
    /// deadline is pulled forward to `now + new interval` only when that is
    /// earlier than the deadline already armed, so a rate increase applies
    /// within one new interval; a rate decrease (or a change that would
    /// *delay* the next click) keeps the pending deadline and the new interval
    /// then applies from [`advance`](Self::advance) onward.
    pub fn set_cps(&mut self, cps: i32, running: bool, now: u64) {
        let interval = click_interval_micros(cps);
        self.interval_us = interval;
        if !running {
            self.next_deadline_us = None;
            return;
        }
        let new_deadline = now.saturating_add(interval);
        let pending = match self.next_deadline_us {
            Some(d) if now < d => d.min(new_deadline),
            // No deadline armed (defensive: running implies one is) or the
            // pending deadline has already lapsed: schedule the next click one
            // new interval from now, consistent with `advance`'s catch-up
            // policy (no burst).
            None | Some(_) => new_deadline,
        };
        self.next_deadline_us = Some(pending);
    }

    /// True when running and a click deadline has already arrived.
    pub fn is_due(&self, now: u64) -> bool {
        matches!(self.next_deadline_us, Some(d) if now >= d)
    }

    /// How long, in microseconds, to wait before the next click: `0` when a
    /// click is due immediately, the remaining time to the deadline otherwise,
    /// and the idle wait when not running.
    pub fn wait_us(&self, running: bool, now: u64) -> u64 {
        if !running {
            return IDLE_WAIT_MICROS;
        }
        match self.next_deadline_us {
            None => IDLE_WAIT_MICROS,
            Some(d) if now >= d => 0,
            Some(d) => d.saturating_sub(now),
        }
    }

    /// Advances the deadline after a click fired at `now`. When the schedule is
    /// kept (this machine was not suspended), the next deadline is exactly one
    /// interval after the *scheduled* deadline — the schedule is not reset to
    /// `now`, so per-click jitter does not accumulate into drift. When the
    /// scheduled next deadline already lies in the past (a missed window), it
    /// is set one interval after `now`: one catch-up click at most, never a
    /// burst.
    pub fn advance(&mut self, now: u64) {
        let d = self.next_deadline_us.take().unwrap_or(now);
        let scheduled = d.saturating_add(self.interval_us);
        let next = if scheduled > now {
            scheduled
        } else {
            now.saturating_add(self.interval_us)
        };
        self.next_deadline_us = Some(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_cps_is_ten() {
        assert_eq!(DEFAULT_CPS, 10);
        assert_eq!(click_interval(DEFAULT_CPS), Duration::from_millis(100));
        assert_eq!(click_interval_micros(DEFAULT_CPS), 100_000);
    }

    #[test]
    fn cps_below_range_is_clamped_up() {
        assert_eq!(validate_cps(0), 1);
        assert_eq!(validate_cps(-7), 1);
        assert_eq!(click_interval(0), Duration::from_secs(1));
        assert_eq!(click_interval_micros(0), 1_000_000);
    }

    #[test]
    fn cps_above_range_is_clamped_down() {
        assert_eq!(validate_cps(501), 500);
        assert_eq!(validate_cps(10_000), 500);
        assert_eq!(click_interval(501), Duration::from_millis(2));
        assert_eq!(click_interval_micros(501), 2_000);
    }

    #[test]
    fn cps_in_range_is_kept() {
        for cps in [1, 2, 10, 50, 99, 100, 250, 499, 500] {
            assert_eq!(validate_cps(cps), cps);
        }
    }

    #[test]
    fn interval_matches_cps() {
        assert_eq!(click_interval(1), Duration::from_secs(1));
        assert_eq!(click_interval(10), Duration::from_millis(100));
        assert_eq!(click_interval(100), Duration::from_millis(10));
        assert_eq!(click_interval(500), Duration::from_millis(2));
        assert!(click_interval(2) < Duration::from_secs(1));
    }

    #[test]
    fn interval_microseconds_are_inverse_of_cps() {
        // `click_interval_micros` floors `1_000_000 / cps`, so the product
        // `micro * cps` is within `cps - 1` of one second (never above it).
        for cps in 1..=500i32 {
            let micro = click_interval_micros(cps);
            let under_one_second = micro * cps as u64 <= 1_000_000;
            assert!(
                under_one_second && micro * cps as u64 >= 1_000_000 - (cps as u64 - 1),
                "cps {cps}: {micro} µs is not ~1s"
            );
        }
        assert_eq!(click_interval_micros(500), 2_000);
        assert_eq!(click_interval_micros(100), 10_000);
        assert_eq!(click_interval_micros(1), 1_000_000);
        assert_eq!(click_interval_micros(3), 333_333); // floor, never up
    }

    fn simulate(cps: i32, duration_us: u64, cps_changes: &[(u64, i32)]) -> usize {
        // A faithful but click-free simulation of the worker loop: it wakes the
        // same way the worker does (immediately when a click is due, otherwise
        // at the next deadline) and counts clicks, without ever calling
        // `SendInput`.
        let mut sched = ClickScheduler::new(cps);
        sched.start(0);
        let mut t = 0;
        let mut clicks = 0usize;
        let mut change_idx = 0;
        loop {
            if change_idx < cps_changes.len() && t >= cps_changes[change_idx].0 {
                sched.set_cps(cps_changes[change_idx].1, true, t);
                change_idx += 1;
            }
            let wait = sched.wait_us(true, t);
            if wait == 0 {
                clicks += 1;
                sched.advance(t);
                if t >= duration_us {
                    break;
                }
            } else {
                t = t.saturating_add(wait);
                if t >= duration_us {
                    break;
                }
            }
        }
        clicks
    }

    #[test]
    fn starting_clicks_immediately_then_holds_the_rate() {
        let clicks = simulate(100, 1_000_000, &[]);
        assert!(
            (95..=105).contains(&clicks),
            "at 100 CPS, one second should yield ~100 clicks, got {clicks}"
        );
    }

    #[test]
    fn the_top_rate_of_500_clicks_per_second_is_scheduled_exactly() {
        // 500 CPS is the range maximum and its exact 2 ms interval divides
        // one second evenly, so the schedule is exact (no rounding loss).
        assert_eq!(click_interval_micros(500), 2_000);
        let clicks = simulate(500, 1_000_000, &[]);
        assert_eq!(
            clicks, 500,
            "at 500 CPS, one second should yield exactly 500 clicks, got {clicks}"
        );
        // A CPS change up to the top of the range applies within one new
        // (2 ms) interval.
        let mut sched = ClickScheduler::new(10); // 100 ms interval
        sched.start(0);
        sched.advance(0); // click at 0, next deadline 100_000
        sched.set_cps(500, true, 20_000);
        assert_eq!(sched.next_deadline_us(), Some(22_000));
        assert_eq!(sched.wait_us(true, 20_000), 2_000);
    }

    #[test]
    fn low_cps_holds_one_second_per_click() {
        let clicks = simulate(1, 3_500_000, &[]);
        assert!(
            (3..=4).contains(&clicks),
            "at 1 CPS, 3.5 s should yield 3-4 clicks, got {clicks}"
        );
    }

    #[test]
    fn missed_deadlines_do_not_burst() {
        let mut sched = ClickScheduler::new(100);
        sched.start(0);
        assert_eq!(sched.wait_us(true, 0), 0);
        sched.advance(0);
        // The machine goes away for 200 ms: the 10 ms deadline is long past.
        // Exactly one click fires at the return, and no catch-up burst.
        assert_eq!(sched.wait_us(true, 200_000), 0, "the overdue click is due");
        sched.advance(200_000);
        assert_eq!(
            sched.next_deadline_us(),
            Some(200_000 + 10_000),
            "the deadline rolls to now+interval, not the old schedule"
        );
        assert_ne!(sched.wait_us(true, 200_000 + 5_000), 0);
    }

    #[test]
    fn cps_increase_applies_within_one_new_interval() {
        let mut sched = ClickScheduler::new(10); // 100 ms interval
        sched.start(0);
        sched.advance(0); // click at 0, next deadline 100_000
        assert_eq!(sched.next_deadline_us(), Some(100_000));
        // The user bumps to 100 CPS at t = 20 ms (mid-old-interval). The next
        // wait must be the new 10 ms, not the stale ~80 ms.
        sched.set_cps(100, true, 20_000);
        assert_eq!(sched.next_deadline_us(), Some(30_000));
        assert_eq!(sched.wait_us(true, 20_000), 10_000);
        // Clicking resumes at 100 CPS from there: the next click at 30 ms is
        // within one new (10 ms) interval of the change at 20 ms.
        sched.advance(30_000);
        assert_eq!(sched.next_deadline_us(), Some(40_000));
    }

    #[test]
    fn cps_decrease_keeps_the_pending_deadline_then_uses_the_new_interval() {
        let mut sched = ClickScheduler::new(100); // 10 ms interval
        sched.start(0);
        sched.advance(0); // next deadline 10_000
                          // Slow down to 1 CPS at t = 2 ms: the pending 8 ms click is kept (a
                          // slower rate must not be delayed further), then 1 s applies.
        sched.set_cps(1, true, 2_000);
        assert_eq!(sched.next_deadline_us(), Some(10_000));
        assert_eq!(sched.wait_us(true, 2_000), 8_000);
        // After that click at t = 10_000, the interval is one second.
        sched.advance(10_000);
        assert_eq!(sched.next_deadline_us(), Some(10_000 + 1_000_000));
    }

    #[test]
    fn stopping_clears_the_deadline_and_restarting_clicks_immediately() {
        let mut sched = ClickScheduler::new(100);
        sched.start(0);
        sched.advance(0);
        assert_ne!(sched.next_deadline_us(), None);
        sched.stop();
        assert_eq!(sched.next_deadline_us(), None);
        assert_eq!(sched.wait_us(false, 5_000), IDLE_WAIT_MICROS);
        // A CPS change while stopped must not arm a deadline.
        sched.set_cps(50, false, 5_000);
        assert_eq!(sched.next_deadline_us(), None);
        // Restart: click immediately, then the (1 s / 50 = 20 ms) interval.
        sched.start(5_000);
        assert!(sched.is_due(5_000));
        assert_eq!(sched.wait_us(true, 5_000), 0);
        sched.advance(5_000);
        assert_eq!(sched.next_deadline_us(), Some(5_000 + 20_000));
    }

    #[test]
    fn the_schedule_does_not_drift_over_many_cycles() {
        // With zero jitter, the k-th click is exactly at k intervals.
        let mut sched = ClickScheduler::new(100);
        sched.start(0);
        let mut t = 0u64;
        for k in 1..=1000 {
            assert_eq!(sched.wait_us(true, t), 0, "click {k} due at t={t}");
            sched.advance(t);
            let expected = (k as u64) * 10_000;
            assert_eq!(
                sched.next_deadline_us(),
                Some(expected),
                "click {k}: schedule drifted (deadline {:?}, expected {expected})",
                sched.next_deadline_us()
            );
            t = expected;
        }
    }

    #[test]
    fn a_small_constant_jitter_does_not_accumulate() {
        // Every wake is 300 µs late (constant jitter). The deadline stays
        // anchored to the schedule, so after many cycles the accumulated error
        // is bounded (one jitter), not k*jitter.
        let mut sched = ClickScheduler::new(100);
        sched.start(0);
        let jitter = 300u64;
        let mut last_click_t = 0u64;
        for k in 1..=500 {
            // Woke 300 µs after the scheduled deadline.
            let t = (k as u64 - 1) * 10_000 + jitter;
            assert!(sched.is_due(t));
            sched.advance(t);
            last_click_t = t;
        }
        // The final deadline must be within one interval of the ideal
        // position, not off by 499*jitter.
        let ideal = 500 * 10_000;
        let deadline = sched.next_deadline_us().expect("armed deadline");
        assert!(
            deadline.abs_diff(ideal) <= 10_000,
            "deadline {deadline} drifted from ideal {ideal}"
        );
        assert!(last_click_t < ideal + 10_000);
    }
}
