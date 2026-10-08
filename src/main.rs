// Release builds are windowed (no console); debug builds keep the console so
// `cargo run` logs stay visible. The attribute is stable on this toolchain.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use slint::SharedString;
use slint::Weak;

use sinclicker::clicker::{target_interval_display, validate_cps, Worker, WorkerEvent};
use sinclicker::hotkey::{Hotkey, HotkeyEvent};
use sinclicker::settings::{self, DEFAULT_HOTKEY_DISPLAY};

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    // Resolve the binding to start with: the persisted one when present and
    // recognized, otherwise the default.
    let initial = settings::load_hotkey_display()
        .as_deref()
        .and_then(settings::choice_for_display)
        .unwrap_or_else(|| {
            settings::choice_for_display(DEFAULT_HOTKEY_DISPLAY)
                .expect("the default hotkey must be supported")
        });

    // Resolve the CPS to start with: the persisted value when it is in range,
    // otherwise the default. The same value initializes the UI (below) and the
    // worker (further below), so the two can never drift apart at startup.
    let initial_cps = settings::resolve_startup_cps(settings::load_cps());

    let window = MainWindow::new()?;
    let weak = window.as_weak();
    if let Some(index) = ui_index_for_display(initial.display) {
        window.set_hotkey_index(index);
    }
    // Showing the persisted value (or the default) in the SpinBox is a plain
    // property set; it does not fire the SpinBox's `edited` callback, so no
    // redundant worker update or save happens at startup.
    window.set_cps(initial_cps);
    // The target interval is derived from the same resolved value, so the
    // displayed interval matches the rate the worker starts with.
    window.set_target_interval(SharedString::from(target_interval_display(initial_cps)));
    // The ComboBox model is the supported catalog itself, in the same order
    // `ui_index_for_display` uses, so the two can never drift apart.
    let model: Vec<SharedString> = settings::supported_hotkey_displays()
        .iter()
        .map(|display| SharedString::from(*display))
        .collect();
    window.set_hotkey_model(slint::VecModel::from_slice(&model));

    let worker = Worker::spawn(initial_cps, {
        let weak = weak.clone();
        // Forward every worker state change onto the Slint UI thread using the
        // supported cross-thread event-loop invocation mechanism. The weak
        // reference is only upgraded on the UI thread.
        move |event| {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = weak.upgrade() {
                    match event {
                        WorkerEvent::Started => {
                            window.set_running(true);
                            window.set_input_error(SharedString::default());
                        }
                        WorkerEvent::Stopped => {
                            window.set_running(false);
                            window.set_input_error(SharedString::default());
                        }
                        WorkerEvent::Error(message) => {
                            window.set_running(false);
                            window.set_input_error(SharedString::from(format!(
                                "Clicking stopped: {message}"
                            )));
                        }
                    }
                }
            });
        }
    });

    let toggle_handle = worker.handle().clone();
    let hotkey_handle = worker.handle().clone();
    let cps_handle = worker.handle().clone();
    let hotkey = Hotkey::spawn(initial, {
        let weak = weak.clone();
        let handle = hotkey_handle;
        // The hotkey thread is a plain message pump: it only reports events.
        // `Activated` is routed through the worker's command channel so the
        // worker stays authoritative for the running state;
        // `RegistrationFailed` is mirrored into the UI (only the error
        // text — the running state is untouched, so it cannot drift).
        move |event| match event {
            HotkeyEvent::Activated => {
                handle.toggle();
            }
            HotkeyEvent::RegistrationFailed { message, active } => {
                let weak = weak.clone();
                let log_message = message.clone();
                // Surface the registration failure in the UI. This runs
                // before `window.run()`: the event-loop proxy only exists
                // once the backend is initialized, so a delivery failure
                // means the UI will never show the message — log it so loss
                // is visible.
                if let Err(error) = slint::invoke_from_event_loop(move || {
                    if let Some(window) = weak.upgrade() {
                        window.set_input_error(SharedString::from(message));
                        // Nothing (or not the selected one) is registered:
                        // the UI must not claim a binding that is not
                        // active.
                        if let Some(choice) = active {
                            if let Some(index) = ui_index_for_display(choice.display) {
                                window.set_hotkey_index(index);
                            }
                        }
                    }
                }) {
                    eprintln!("hotkey registration failed and the UI could not be notified ({error}): {log_message}");
                }
            }
        }
    });

    let binding_handle = hotkey.handle();
    window.on_hotkey_changed({
        let weak = weak.clone();
        move |display: SharedString| {
            // Validate before touching the hotkey service: no arbitrary
            // combination is ever registered.
            let display = display.as_str();
            let Some(index) = ui_index_for_display(display) else {
                restore_selection(&weak);
                return;
            };
            let Some(choice) = settings::SUPPORTED_HOTKEYS.get(index as usize).copied() else {
                restore_selection(&weak);
                return;
            };

            // The worker is authoritative for the running state, so a binding
            // change never touches it; only the error text may change.
            match binding_handle.set_binding(choice) {
                Ok(()) => {
                    // The new binding is active: persist it. Persisting cannot
                    // fail the binding itself; report the error only.
                    if let Err(error) = settings::save_hotkey_display(choice.display) {
                        let weak = weak.clone();
                        let message = format!("The hotkey could not be saved: {error}");
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(window) = weak.upgrade() {
                                window.set_input_error(SharedString::from(message));
                            }
                        });
                    }
                }
                Err(report) => {
                    let weak = weak.clone();
                    let log_message = report.message.clone();
                    // The new binding could not be registered: the previous
                    // (last working) binding, if any, is still active — the
                    // hotkey service retires it only after a successful
                    // re-registration, and `report.active` says which one that
                    // is (authoritative, from the hotkey thread). The worker
                    // state is unchanged (no toggle was sent); the failure
                    // text is shown and the selection goes back to the
                    // binding that is really active.
                    if let Err(error) = slint::invoke_from_event_loop(move || {
                        if let Some(window) = weak.upgrade() {
                            window.set_input_error(SharedString::from(report.message));
                            if let Some(active) = report.active {
                                if let Some(index) = ui_index_for_display(active.display) {
                                    window.set_hotkey_index(index);
                                }
                            }
                        }
                    }) {
                        eprintln!("hotkey binding change failed and the UI could not be notified ({error}): {log_message}");
                    }
                }
            }
        }
    });

    window.on_toggle_clicking(move || toggle_handle.toggle());
    window.on_cps_changed({
        let weak = weak.clone();
        move |value: i32| {
            // Validate into the supported range before sending and storing:
            // the same clamped value reaches the worker and the registry, so
            // a stored value is always in range.
            let cps = validate_cps(value);
            cps_handle.set_cps(cps);
            // Keep the displayed target interval in step with the exact value
            // the worker is about to use. The callback already runs on the UI
            // thread, so the property set needs no event-loop hop.
            if let Some(window) = weak.upgrade() {
                window.set_target_interval(SharedString::from(target_interval_display(cps)));
            }
            // Persist the new rate. A failed save must not break clicking:
            // report it in the UI and say exactly that it was not saved,
            // rather than claiming it was.
            if let Err(error) = settings::save_cps(cps) {
                let weak = weak.clone();
                let message = format!("The click rate could not be saved: {error}");
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(window) = weak.upgrade() {
                        window.set_input_error(SharedString::from(message));
                    }
                });
            }
        }
    });

    let run_result = window.run();

    // Orderly shutdown once the window is closed:
    // 1. Stop the hotkey: wake its thread (which unregisters the active
    //    hotkey on the registering thread before exiting) and join it.
    //    After this, no hotkey message can reach the worker and no global
    //    hotkey remains registered.
    hotkey.shutdown();
    // 2. Stop the click scheduler and join its thread.
    worker.shutdown();

    run_result
}

/// The ComboBox index for a canonical display string, or `None` when it is
/// not one of the supported choices.
fn ui_index_for_display(display: &str) -> Option<i32> {
    settings::SUPPORTED_HOTKEYS
        .iter()
        .position(|choice| choice.display == display)
        .map(|index| index as i32)
}

/// Puts the ComboBox back on the persisted (last known-good) choice so the
/// UI never claims a binding that is not active. Runs only on the UI thread.
fn restore_selection(weak: &Weak<MainWindow>) {
    let display = settings::load_hotkey_display()
        .and_then(|stored| settings::choice_for_display(&stored))
        .map(|choice| choice.display.to_owned())
        .unwrap_or_else(|| DEFAULT_HOTKEY_DISPLAY.to_owned());
    if let Some(index) = ui_index_for_display(&display) {
        if let Some(window) = weak.upgrade() {
            window.set_hotkey_index(index);
        }
    }
}
