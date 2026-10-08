# Autoclicker — product and implementation specification

## Purpose

Build a compact Windows desktop autoclicker using Rust and Slint. The first
version should provide one global hotkey to toggle clicking and one control for
clicks per second (CPS).

This is a normal user-mode utility. It must not try to bypass Windows security,
anti-cheat systems, or application input protections.

## Target platform

- Windows 11, x64.
- Stable Rust toolchain.
- Rust application logic and Slint UI.
- Use the current compatible stable Slint and `windows` crate versions.
- The app must not require administrator rights.

Keep the initial implementation Windows-only. Do not add cross-platform
backends unless needed by Slint itself.

## User experience

The window should be modern, compact, clear, and keyboard accessible.

It should include:

1. App name and a short description.
2. A prominent status indicator:
   - `Stopped` when clicking is off.
   - `Clicking` when clicking is on.
3. A clear Start/Stop button.
4. A CPS control with a visible numeric value.
5. A display of the global toggle hotkey.
6. A concise hint that clicking happens at the current pointer position.

The global hotkey toggles clicking on and off. The in-window button must do the
same. The hotkey should work while the app window is not focused.

Use a fixed initial hotkey of **Ctrl+Alt+F6**. Show it in the interface. If it
is already registered by another application, report that clearly and do not
start clicking. The user may switch to another binding from a supported list of
Ctrl+Alt combinations; only those supported bindings can ever be registered,
and arbitrary combinations are not accepted. The chosen binding persists
across launches.

Do not add a system-tray icon, startup-on-login, profiles, click-location
selection, or multiple mouse buttons in this version. The settings that are
persisted are the hotkey choice and the CPS value.

## Click rate

- Provide a CPS range of **1–500**.
- Enforce the range in Rust even if the UI control already restricts it.
- Initialize the app to **10 CPS** and restore the user's last CPS value on
  launch; a stored value that is missing, malformed, or out of range falls
  back to the 10 CPS default.
- At a CPS value `n`, schedule one left click approximately every `1/n`
  seconds. At the 500 CPS maximum the requested interval is 2 ms; whether the
  system actually clicks that fast is subject to Windows scheduling (see
  below).
- Send a left-button down/up pair with Windows `SendInput`.
- Send each down/up pair together where the API permits, to reduce the chance
  of a stuck button or interleaving input.
- Do not busy-wait.
- Do not run the click scheduler on the UI thread.
- Changing CPS while clicking should update the rate without requiring a
  restart.
- The actual timing is subject to Windows scheduling and is not guaranteed to
  have real-time precision. At high rates the 2 ms request at 500 CPS depends
  on the scheduler being able to wake every 2 ms: with the high-resolution
  waitable timer this is within reach on modern Windows, but on a system
  whose waitable timers are limited to the coarser system timer resolution
  (or under UIPI/scheduling load) the delivered rate can fall below the
  requested one. Nothing here promises that every target application will
  observe exactly 500 clicks per second.

Clicks should happen at the current pointer position; do not move the pointer.

## Global hotkey and threading

Register Ctrl+Alt+F6 using the Windows global-hotkey API.

Use a dedicated background thread for the hotkey message loop. Do not block the
Slint UI thread waiting for hotkey messages. Send hotkey events and registration
errors back to the UI using Slint's supported event-loop invocation mechanism.

Use a separate click-scheduler worker, or another design that remains
non-blocking and easy to shut down. The scheduler should sleep or wait until the
next click or a command arrives; it must not spin or poll continuously.

On application close:

1. Set the active state to stopped.
2. Stop the click scheduler.
3. Unregister the global hotkey.
4. Shut down and join worker threads where practical.

If registration fails, clicking stays stopped and the user sees a useful error.
Do not silently fall back to a different hotkey.

## Light and dark appearance

Use Slint's standard widget style and allow it to follow the current Windows
light/dark preference. Do not force a `-light` or `-dark` style.

Prefer built-in styling and a small amount of layout customization over a large
custom theme. Avoid adding a theme-polling thread. First verify the behavior
against the current Slint version and test both Windows light and dark settings.
If Slint does not respond to a theme change while the app is already open,
document the observed behavior rather than introducing an unverified registry
watcher.

## Resource goals

Keep the app idle-friendly:

- No busy loops or high-frequency polling.
- No unnecessary background threads or recurring timers.
- No continuous animation or redraw when the UI is idle.
- No web runtime, browser frontend, database, or heavyweight async runtime.
- Use only dependencies needed for the UI, Windows APIs, and tests.

Use Slint's normal supported backend and renderer first. Do not switch to a
software renderer or disable default features just because it sounds lighter;
make such a change only if it builds correctly and measured results justify it.

A release profile may favor a smaller binary, but do not describe binary-size
settings as guaranteed runtime or memory optimizations.

## Suggested project layout

The implementation may choose a slightly different layout if there is a
concrete reason.

    Cargo.toml
    Cargo.lock
    build.rs
    ui/
      app.slint
    src/
      main.rs
      app.rs
      clicker.rs
      hotkey.rs
      win_input.rs
    tests/
      rate_validation.rs
    README.md

Keep Windows API calls and `unsafe` code localized. Add comments describing
safety assumptions around each unsafe block.

## Windows input limitations

`SendInput` is subject to Windows User Interface Privilege Isolation (UIPI).
The app must not elevate itself or claim it can click into every application.
Explain that Windows can block input to a higher-integrity application.

At the 500 CPS maximum, each left down/up pair is still sent with `SendInput`,
and the target application's own input processing (and any UIPI filtering)
determines how many of the requested clicks per second it actually records.
The app reports only what `SendInput` accepted; it never claims more.

## Out of scope

- Click location selection or pointer movement.
- Right-click, middle-click, or multi-button patterns.
- Randomized intervals.
- Macro recording or playback.
- Hotkey rebinding outside the supported Ctrl+Alt list.
- Tray mode or background service.
- Installer, auto-updater, or auto-start.
- Attempts to evade anti-cheat or application protections.

## References

Use current documentation and verify exact APIs against the selected crate
version.

- Slint widget styles:
  https://docs.slint.dev/latest/docs/slint/reference/std-widgets/style/
- Slint Rust Cargo features:
  https://docs.slint.dev/latest/docs/rust/slint/docs/cargo_features/
- Rust for Windows overview:
  https://learn.microsoft.com/en-us/windows/dev-environment/rust/rust-for-windows
- Windows `SendInput` and UIPI behavior:
  https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput
- Windows `RegisterHotKey`:
  https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerhotkey