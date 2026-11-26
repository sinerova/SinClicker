// Release builds are windowed (no console); debug builds keep the console so
// `cargo run` logs stay visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use slint::{SharedString, VecModel};

use sinclicker::clicker::{
    target_interval_display, validate_cps, Worker, WorkerEvent, MAX_CPS, MIN_CPS,
};
use sinclicker::hotkey::{Hotkey, HotkeyEvent};
use sinclicker::settings::{self, HotkeyChoice, SUPPORTED_HOTKEYS};

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    // The persisted settings when present and valid, otherwise the defaults.
    // The same values initialize the UI, the worker and the hotkey thread, so
    // they cannot disagree at startup.
    let initial_hotkey =
        settings::resolve_startup_hotkey(settings::load_hotkey_display().as_deref());
    let initial_cps = settings::resolve_startup_cps(settings::load_cps());

    let window = MainWindow::new()?;
    let weak = window.as_weak();

    // The ComboBox model is the catalog itself, in catalog order, so a UI
    // index and a catalog index are the same thing.
    let model: Vec<SharedString> = SUPPORTED_HOTKEYS.iter().map(|c| c.display.into()).collect();
    window.set_hotkey_model(VecModel::from_slice(&model));
    select_hotkey(&window, initial_hotkey);
    window.set_min_cps(MIN_CPS);
    window.set_max_cps(MAX_CPS);
    // Plain property sets do not fire the SpinBox's `edited` callback, so no
    // redundant worker update or save happens here.
    window.set_cps(initial_cps);
    window.set_target_interval(target_interval_display(initial_cps).into());

    let worker = Worker::spawn(initial_cps, {
        let weak = weak.clone();
        // Runs on the worker thread: hop onto the UI thread to update it.
        move |event| {
            let _ = weak.upgrade_in_event_loop(move |window| {
                let (running, error) = match event {
                    WorkerEvent::Started => (true, String::new()),
                    WorkerEvent::Stopped => (false, String::new()),
                    WorkerEvent::Error(message) => (false, format!("Clicking stopped: {message}")),
                };
                window.set_running(running);
                window.set_input_error(error.into());
            });
        }
    });

    let hotkey = Hotkey::spawn(initial_hotkey, {
        let weak = weak.clone();
        let worker = worker.handle().clone();
        // Runs on the hotkey thread. A press is routed through the worker so
        // the worker stays the single authority on the running state.
        move |event| match event {
            HotkeyEvent::Activated => worker.toggle(),
            HotkeyEvent::RegistrationFailed(message) => {
                let log_message = message.clone();
                if let Err(error) =
                    weak.upgrade_in_event_loop(move |window| window.set_input_error(message.into()))
                {
                    eprintln!("hotkey registration failed and the UI could not be notified ({error}): {log_message}");
                }
            }
        }
    });

    // The callbacks below run on the UI thread, so they update the window
    // directly. They hold a weak reference to avoid a window -> callback ->
    // window reference cycle.
    window.on_toggle_clicking({
        let worker = worker.handle().clone();
        move || worker.toggle()
    });

    window.on_cps_changed({
        let weak = weak.clone();
        let worker = worker.handle().clone();
        move |value| {
            let Some(window) = weak.upgrade() else { return };
            // The same clamped value reaches the worker, the display and the
            // registry.
            let cps = validate_cps(value);
            worker.set_cps(cps);
            window.set_target_interval(target_interval_display(cps).into());
            // A failed save must not break clicking; say it was not saved.
            if let Err(error) = settings::save_cps(cps) {
                window
                    .set_input_error(format!("The click rate could not be saved: {error}").into());
            }
        }
    });

    window.on_hotkey_changed({
        let weak = weak.clone();
        let hotkey = hotkey.handle();
        move |display| {
            let Some(window) = weak.upgrade() else { return };
            // The model is the catalog, so this only fails if they drift apart.
            let Some(choice) = settings::choice_for_display(&display) else {
                return;
            };
            match hotkey.set_binding(choice) {
                Ok(()) => {
                    // The new binding is active: clear any earlier hotkey
                    // error, then persist it (a failed save is only reported).
                    let error = match settings::save_hotkey_display(choice.display) {
                        Ok(()) => String::new(),
                        Err(error) => format!("The hotkey could not be saved: {error}"),
                    };
                    window.set_input_error(error.into());
                }
                Err(report) => {
                    // The previous binding (if any) is still the active one;
                    // put the selection back on it so the UI does not claim a
                    // binding that is not registered.
                    window.set_input_error(report.message.into());
                    if let Some(active) = report.active {
                        select_hotkey(&window, active);
                    }
                }
            }
        }
    });

    let run_result = window.run();

    // Orderly shutdown once the window is closed: first the hotkey thread
    // (unregisters the hotkey, so no more toggles can reach the worker), then
    // the click worker.
    hotkey.shutdown();
    worker.shutdown();

    run_result
}

/// Points the ComboBox at `choice`.
fn select_hotkey(window: &MainWindow, choice: HotkeyChoice) {
    if let Some(index) = SUPPORTED_HOTKEYS.iter().position(|c| *c == choice) {
        window.set_hotkey_index(index as i32);
    }
}
