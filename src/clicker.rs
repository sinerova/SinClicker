use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::win_input::left_mouse_down_up;

pub const MIN_CPS: i32 = 1;
pub const MAX_CPS: i32 = 100;
pub const DEFAULT_CPS: i32 = 10;

/// Clamps an arbitrary CPS value into the supported 1..=100 range.
///
/// This enforces the spec range in Rust even though the UI SpinBox already
/// restricts it.
pub fn validate_cps(cps: i32) -> i32 {
    // MIN_CPS (1) < MAX_CPS (100), so clamp cannot panic.
    cps.clamp(MIN_CPS, MAX_CPS)
}

/// Returns the time to wait between clicks for a CPS value that has been
/// clamped (or is already in range) to 1..=100. A CPS of 1 waits 1 second;
/// a CPS of 100 waits 10 milliseconds.
pub fn click_interval(cps: i32) -> Duration {
    Duration::from_secs_f64(1.0 / validate_cps(cps) as f64)
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
/// exited (the application is shutting down) the send fails silently.
#[derive(Clone)]
pub struct WorkerHandle {
    tx: Sender<Command>,
}

impl WorkerHandle {
    fn send(&self, command: Command) {
        let _ = self.tx.send(command);
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
/// `SendInput` at the current pointer position. It sleeps for the current
/// click interval between events and never busy-waits. `Start`, `Stop`,
/// `SetCps` and `Quit` commands arrive over an mpsc channel; state changes are
/// reported through the `report` closure given to [`Worker::spawn`].
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
        let thread = std::thread::Builder::new()
            .name("sinclicker-worker".to_owned())
            .spawn(move || worker_loop(rx, report, default_cps))
            .expect("failed to spawn the click-scheduler worker thread");
        Worker {
            handle: WorkerHandle { tx },
            thread,
        }
    }

    pub fn handle(&self) -> &WorkerHandle {
        &self.handle
    }

    /// Stops clicking, terminates the worker, and joins its thread. Joining is
    /// the practical way to guarantee the worker has fully stopped before the
    /// application exits. Consuming `self` is intentional: the worker is never
    /// restarted.
    pub fn shutdown(self) {
        self.handle.stop();
        self.handle.quit();
        if let Err(error) = self.thread.join() {
            eprintln!("failed to join the click worker thread: {error:?}");
        }
    }
}

fn worker_loop(command_rx: Receiver<Command>, report: impl Fn(WorkerEvent), default_cps: i32) {
    let mut cps = validate_cps(default_cps);
    let mut running = false;

    loop {
        // Sleep until the next scheduled click, or a very long time while
        // stopped. `recv_timeout` blocks (it does not poll or spin).
        let timeout = if running {
            click_interval(cps)
        } else {
            Duration::from_secs(u32::MAX as u64)
        };

        match command_rx.recv_timeout(timeout) {
            Ok(Command::Start) => {
                if !running {
                    running = true;
                    report(WorkerEvent::Started);
                }
            }
            Ok(Command::Stop) => {
                if running {
                    running = false;
                    report(WorkerEvent::Stopped);
                }
            }
            Ok(Command::Toggle) => {
                if running {
                    running = false;
                    report(WorkerEvent::Stopped);
                } else {
                    running = true;
                    report(WorkerEvent::Started);
                }
            }
            Ok(Command::Quit) => break,
            Ok(Command::SetCps(value)) => {
                cps = validate_cps(value);
            }
            Err(RecvTimeoutError::Timeout) => {
                if running {
                    if let Err(error) = left_mouse_down_up() {
                        // Never report a failed click: stop the clicker and
                        // surface the failure through the UI.
                        running = false;
                        report(WorkerEvent::Error(error.message));
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_cps_is_ten() {
        assert_eq!(DEFAULT_CPS, 10);
        assert_eq!(click_interval(DEFAULT_CPS), Duration::from_millis(100));
    }

    #[test]
    fn cps_below_range_is_clamped_up() {
        assert_eq!(validate_cps(0), 1);
        assert_eq!(validate_cps(-7), 1);
        assert_eq!(click_interval(0), Duration::from_secs(1));
    }

    #[test]
    fn cps_above_range_is_clamped_down() {
        assert_eq!(validate_cps(101), 100);
        assert_eq!(validate_cps(10_000), 100);
        assert_eq!(click_interval(101), Duration::from_millis(10));
    }

    #[test]
    fn cps_in_range_is_kept() {
        for cps in [1, 2, 10, 50, 99, 100] {
            assert_eq!(validate_cps(cps), cps);
        }
    }

    #[test]
    fn interval_matches_cps() {
        assert_eq!(click_interval(1), Duration::from_secs(1));
        assert_eq!(click_interval(10), Duration::from_millis(100));
        assert_eq!(click_interval(100), Duration::from_millis(10));
        assert!(click_interval(2) < Duration::from_secs(1));
    }
}
