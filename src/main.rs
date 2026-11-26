// Release builds are windowed (no console); debug builds keep the console so
// `cargo run` logs stay visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use slint::{SharedString, VecModel};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_SHIFT,
};

use sinclicker::clicker::{
    target_interval_display, validate_cps, Worker, WorkerEvent, MAX_CPS, MIN_CPS,
};
use sinclicker::hotkey::{Hotkey, HotkeyEvent};
use sinclicker::settings::{self, HotkeyChoice};
use sinclicker::win_hotkey::vk_from_key_text;

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

    show_hotkey(&window, initial_hotkey);
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

    // Mirrors the UI's `hotkey-recording` for the hotkey thread. While a new
    // shortcut is being recorded, pressing the *current* one never reaches the
    // window (Windows delivers it as the global hotkey instead), so it ends
    // recording, keeping that binding, rather than toggling clicking.
    let recording = Arc::new(AtomicBool::new(false));

    let hotkey = Hotkey::spawn(initial_hotkey, {
        let weak = weak.clone();
        let worker = worker.handle().clone();
        let recording = Arc::clone(&recording);
        // Runs on the hotkey thread. A press is routed through the worker so
        // the worker stays the single authority on the running state.
        move |event| match event {
            HotkeyEvent::Activated if recording.load(Ordering::Acquire) => {
                let _ = weak.upgrade_in_event_loop(|window| window.set_hotkey_recording(false));
            }
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

    window.on_hotkey_recording_changed({
        let recording = Arc::clone(&recording);
        move |value| recording.store(value, Ordering::Release)
    });

    window.on_hotkey_captured({
        let weak = weak.clone();
        let hotkey = hotkey.handle();
        move |text, ctrl, alt, shift, meta| {
            let Some(window) = weak.upgrade() else { return };
            let modifiers = [(ctrl, MOD_CONTROL), (alt, MOD_ALT), (shift, MOD_SHIFT)]
                .into_iter()
                .filter(|(held, _)| *held)
                .fold(HOT_KEY_MODIFIERS(0), |all, (_, m)| all | m);
            let choice = vk_from_key_text(&text)
                .ok_or_else(|| "That key can't be used for the hotkey.".to_owned())
                .and_then(|vk| HotkeyChoice::new(modifiers, vk, meta));
            let choice = match choice {
                Ok(choice) => choice,
                Err(problem) => {
                    // Not a usable binding: say why and keep recording so the
                    // user can try another combination.
                    window.set_hotkey_problem(problem.into());
                    return;
                }
            };
            window.set_hotkey_recording(false);
            match hotkey.set_binding(choice) {
                Ok(()) => {
                    // The new binding is active: show it, clear any earlier
                    // hotkey error, then persist it (a failed save is only
                    // reported).
                    show_hotkey(&window, choice);
                    let error = match settings::save_hotkey_display(&choice.display()) {
                        Ok(()) => String::new(),
                        Err(error) => format!("The hotkey could not be saved: {error}"),
                    };
                    window.set_input_error(error.into());
                }
                Err(report) => {
                    // The previous binding (if any) is still the active one,
                    // and the field still shows it; report why the new one
                    // could not be registered.
                    window.set_input_error(report.message.into());
                    if let Some(active) = report.active {
                        show_hotkey(&window, active);
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

/// Shows `choice` in the shortcut field.
fn show_hotkey(window: &MainWindow, choice: HotkeyChoice) {
    let keys: Vec<SharedString> = choice.parts().into_iter().map(Into::into).collect();
    window.set_hotkey_keys(VecModel::from_slice(&keys));
    window.set_hotkey_display(choice.display().into());
}
