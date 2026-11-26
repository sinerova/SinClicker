//! Global hotkey subsystem: a dedicated thread owns the whole lifecycle
//! (registration, message pumping, unregistration), so the Slint UI thread is
//! never involved in Win32 hotkey handling.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::{WAIT_ABANDONED, WAIT_FAILED, WAIT_OBJECT_0};
use windows::Win32::System::Threading::INFINITE;
use windows::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey;
use windows::Win32::UI::WindowsAndMessaging::{
    MsgWaitForMultipleObjectsEx, PeekMessageW, MSG, MWMO_NONE, PM_REMOVE, QS_ALLINPUT, QS_HOTKEY,
    WM_HOTKEY,
};

use crate::settings::HotkeyChoice;
use crate::win_event::Event;
use crate::win_hotkey::{
    describe_registration_failure, register_hotkey, HOTKEY_ID_MAX, HOTKEY_ID_MIN,
};

/// Events emitted toward the UI thread by the hotkey subsystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// The global hotkey was pressed; the caller should toggle clicking.
    Activated,
    /// The initial registration at startup failed, so no global hotkey is
    /// active. Clicking stays as it is and the message must be surfaced in the
    /// UI. (A failed *re*-registration is reported to the caller of
    /// [`HotkeyHandle::set_binding`] instead.)
    RegistrationFailed(String),
}

/// A failed (re-)registration report: the user-facing message plus the
/// binding that is still active on the hotkey thread (`None` when nothing is
/// registered).
#[derive(Debug, Clone)]
pub struct RegistrationReport {
    pub message: String,
    pub active: Option<HotkeyChoice>,
}

/// Commands delivered to the hotkey thread (FIFO, in arrival order).
enum HotkeyCommand {
    /// Re-register the hotkey with `choice` on the hotkey thread: register
    /// `choice` (under a fresh id), and only after that succeeds, retire the
    /// binding that was active before (if any). If the registration fails,
    /// the previous binding, if any, stays active. If `choice` is already the
    /// registered binding, nothing is re-registered (the combination is still
    /// held, and `RegisterHotKey` would reject it as 1409). Either way the
    /// outcome is reported through `response` — `Ok` when `choice` is the
    /// active binding after the command runs.
    Register {
        choice: HotkeyChoice,
        response: Sender<Result<(), RegistrationReport>>,
    },
}

/// A cheap, cloneable handle for changing the active hotkey binding.
///
/// `set_binding` sends a re-registration command to the hotkey thread and
/// blocks (bounded) until the thread reports the outcome, so the caller
/// always knows whether the new binding is active. After
/// [`Hotkey::shutdown`] it returns an error instead of blocking.
#[derive(Clone)]
pub struct HotkeyHandle {
    command_tx: Sender<HotkeyCommand>,
    request_event: Arc<Event>,
}

impl HotkeyHandle {
    /// Re-registers the global hotkey with `choice`.
    ///
    /// On `Ok`, `choice` is the active global hotkey (a different previous
    /// binding, if any, was retired on the hotkey thread). On `Err`, the
    /// report is authoritative: its message is safe to show in the UI and its
    /// `active` field is the binding still registered on the hotkey thread —
    /// the previous one (if any), which a failed re-registration does not
    /// retire.
    pub fn set_binding(&self, choice: HotkeyChoice) -> Result<(), RegistrationReport> {
        let (response_tx, response_rx) = channel::<Result<(), RegistrationReport>>();
        if self
            .command_tx
            .send(HotkeyCommand::Register {
                choice,
                response: response_tx,
            })
            .is_err()
        {
            // The hotkey thread is gone (the app is shutting down); nothing
            // is registered.
            return Err(RegistrationReport {
                message: "the hotkey service is no longer running".to_owned(),
                active: None,
            });
        }
        self.request_event.signal();
        // Bounded wait: re-registration never blocks longer than the two
        // `RegisterHotKey`/`UnregisterHotKey` calls on the hotkey thread.
        match response_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(report)) => Err(report),
            Err(_) => Err(RegistrationReport {
                message: "the hotkey service did not respond; the hotkey is unavailable".to_owned(),
                active: None,
            }),
        }
    }
}

/// Owns the dedicated background thread that registers and services the
/// global hotkey.
///
/// Registration and unregistration both happen **on the hotkey thread**, as
/// Windows requires (a hotkey is unregistered by the thread that registered
/// it). The thread blocks in `MsgWaitForMultipleObjectsEx` woken by queued
/// input, the request event, or the shutdown event; it never polls or spins.
/// It holds no reference to the clicker worker: `Activated` is reported to
/// the caller, which routes it through the worker so the worker stays
/// authoritative for the running state.
pub struct Hotkey {
    thread: JoinHandle<()>,
    shutdown_event: Arc<Event>,
    request_event: Arc<Event>,
    command_tx: Sender<HotkeyCommand>,
}

impl Hotkey {
    /// Spawns the hotkey thread, which registers `initial` on itself.
    ///
    /// If the initial registration fails, `on_event` is called with
    /// [`HotkeyEvent::RegistrationFailed`] from the hotkey thread.
    pub fn spawn(initial: HotkeyChoice, on_event: impl Fn(HotkeyEvent) + Send + 'static) -> Hotkey {
        let shutdown_event = Arc::new(Event::new());
        let request_event = Arc::new(Event::new());
        let (command_tx, command_rx) = channel();

        let thread = std::thread::Builder::new()
            .name("sinclicker-hotkey".to_owned())
            .spawn({
                let shutdown = Arc::clone(&shutdown_event);
                let request = Arc::clone(&request_event);
                move || hotkey_loop(initial, &shutdown, &request, command_rx, on_event)
            })
            .expect("failed to spawn the hotkey thread");

        Hotkey {
            thread,
            shutdown_event,
            request_event,
            command_tx,
        }
    }

    pub fn handle(&self) -> HotkeyHandle {
        HotkeyHandle {
            command_tx: self.command_tx.clone(),
            request_event: Arc::clone(&self.request_event),
        }
    }

    /// Shuts the hotkey down: wakes the thread (which unregisters the active
    /// hotkey on itself before exiting) and joins it. After this returns no
    /// global hotkey is registered and no hotkey thread remains.
    pub fn shutdown(self) {
        self.shutdown_event.signal();
        if let Err(error) = self.thread.join() {
            eprintln!("failed to join the hotkey thread: {error:?}");
        }
    }
}

/// The binding currently registered on the hotkey thread.
#[derive(Clone, Copy)]
struct CurrentBinding {
    choice: HotkeyChoice,
    id: i32,
}

/// Runs on the hotkey thread: registers `initial`, then pumps the message
/// queue servicing hotkey presses and binding-change commands until woken by
/// the shutdown event, and unregisters the active hotkey on the way out.
fn hotkey_loop(
    initial: HotkeyChoice,
    shutdown: &Event,
    request: &Event,
    command_rx: Receiver<HotkeyCommand>,
    on_event: impl Fn(HotkeyEvent),
) {
    // The binding currently registered (if any). Every (re-)registration
    // gets the next id in the valid range (wrapping), so `is_toggle_hotkey`
    // accepts only the `WM_HOTKEY` message of the *current* binding: a
    // message queued by a superseded binding (pressed just before a
    // re-register) still carries the old id and is ignored instead of acting
    // as a stray toggle. Re-registration is register-new-then-retire-old, so
    // a failed re-registration leaves the previous binding active.
    let mut ids = (HOTKEY_ID_MIN..=HOTKEY_ID_MAX).cycle();
    let mut next_id = move || ids.next().expect("a cycled non-empty range never ends");
    let id = next_id();
    let mut current: Option<CurrentBinding> = match register_hotkey(initial, id) {
        Ok(()) => Some(CurrentBinding {
            choice: initial,
            id,
        }),
        Err(code) => {
            let message = describe_registration_failure(
                initial.display,
                code,
                false, // nothing was registered before the initial attempt
            );
            on_event(HotkeyEvent::RegistrationFailed(message));
            None
        }
    };

    let mut msg = MSG::default();
    // `QS_HOTKEY | QS_ALLINPUT`: wake on any queued input event; `QS_HOTKEY`
    // documents the hotkey intent explicitly.
    let wake_mask = QS_HOTKEY | QS_ALLINPUT;
    loop {
        // Block until queued input, a binding-change request, or shutdown.
        let wait = unsafe {
            // SAFETY: both events are live for the whole loop (borrowed from
            // `Arc`s that the spawning closure owns).
            MsgWaitForMultipleObjectsEx(
                Some(&[shutdown.raw(), request.raw()]),
                INFINITE,
                wake_mask,
                MWMO_NONE,
            )
        };

        if wait == WAIT_OBJECT_0 {
            break;
        }
        if wait == WAIT_FAILED || wait == WAIT_ABANDONED {
            eprintln!("hotkey message wait returned invalid status; stopping the hotkey thread");
            break;
        }

        // Drain the queue (non-blocking) so no hotkey press is dropped and no
        // spin occurs. The queue is only non-empty after a wake, so this is
        // bounded work.
        loop {
            let has_message = unsafe {
                // SAFETY: `PeekMessageW` fills the caller-allocated `MSG`.
                PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE)
            }
            .as_bool();
            if !has_message {
                break;
            }
            if let Some(current) = current {
                if is_toggle_hotkey(msg.message, msg.wParam.0, current.id) {
                    on_event(HotkeyEvent::Activated);
                }
            }
        }

        // Service any pending binding-change commands (FIFO, in arrival
        // order). `try_recv` is non-blocking; the request event guarantees a
        // wake for at least one item, and anything else that arrived in the
        // meantime is handled here in the same pass.
        while let Ok(command) = command_rx.try_recv() {
            match command {
                HotkeyCommand::Register { choice, response } => {
                    // The requested choice is already the registered binding:
                    // a no-op. It must NOT be re-registered: the combination
                    // is still held under the current id, and `RegisterHotKey`
                    // rejects a held combination process-wide (Win32 1409),
                    // which would surface as a false "already in use" error.
                    if current.map(|current| current.choice) == Some(choice) {
                        let _ = response.send(Ok(()));
                    } else {
                        let id = next_id();
                        match register_hotkey(choice, id) {
                            Ok(()) => {
                                // The new binding is active now; retire the old
                                // one (if any) on this, its registering, thread.
                                if let Some(previous) =
                                    current.replace(CurrentBinding { choice, id })
                                {
                                    let _ = unsafe {
                                        // SAFETY: the previous binding's own id,
                                        // on the same (registering) thread as its
                                        // registration.
                                        UnregisterHotKey(None, previous.id)
                                    };
                                }
                                let _ = response.send(Ok(()));
                            }
                            Err(code) => {
                                // Registration failed (e.g. the combination is
                                // taken by another application): the previous
                                // binding, if any, was NOT retired, so it is
                                // still the active hotkey and the app keeps
                                // working with the last good binding. Only the
                                // new one is unavailable.
                                let report = RegistrationReport {
                                    message: describe_registration_failure(
                                        choice.display,
                                        code,
                                        current.is_some(),
                                    ),
                                    active: current.map(|current| current.choice),
                                };
                                let _ = response.send(Err(report));
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(current) = current {
        let _ = unsafe {
            // SAFETY: the active binding's own id, on the same (registering)
            // thread as its registration.
            UnregisterHotKey(None, current.id)
        };
    }
}

/// Returns `true` when the message is the hotkey pressed for the binding
/// registered under `id`.
fn is_toggle_hotkey(message: u32, wparam: usize, id: i32) -> bool {
    message == WM_HOTKEY && wparam as i32 == id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_the_matching_hotkey_message_and_id() {
        assert!(is_toggle_hotkey(WM_HOTKEY, 7usize, 7));
        assert!(!is_toggle_hotkey(WM_HOTKEY, 8usize, 7)); // stale id is ignored
        assert!(!is_toggle_hotkey(0, 7usize, 7));
    }
}
