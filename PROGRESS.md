# Progress

## Milestone 1 — done (window scaffold)

- Scaffold: `Cargo.toml`, `build.rs`, `ui/app.slint`, `src/main.rs`, `.gitignore`, `Cargo.lock` (generated).
- Toolchain: `stable-x86_64-pc-windows-msvc` (rustc 1.99.0).
- Versions: `slint 1.18.1`, `slint-build 1.18.1` (only dependencies).
- No style suffix set: default `fluent` style, which per 1.18.1 sources follows the
  system light/dark setting at window creation and on winit `ThemeChanged` events
  (i-slint-backend-winit `winitwindowadapter.rs`).
- `Cargo.lock` kept.
- Checks: `cargo check`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo build --release` all pass (see session report 2026-10-05).
- Smoke-tested: app launches and stays alive (system was in dark mode).
- UI is static: Start/Stop button is a no-op, CPS is a SpinBox (1–100, default 10)
  bound two-way to the `cps` window property, hotkey shown as text (Ctrl+Alt+F6).

## Notes / decisions

- The "latest" Slint docs (next release) do not match 1.18.1's API: use
  `VerticalBox`/`HorizontalBox` (not `*Layout`), `wrap: word-wrap` (not
  `wrap-mode`), `FontWeight.semi-bold`, SpinBox `minimum`/`maximum`/`step-size`,
  `horizontal-stretch: 1`, `slint::PlatformError` (not `slint::Result`).
  Verified against the v1.18.1 crate sources in the cargo registry.
- `cps` is typed `int` in the UI (SpinBox `value` is `int`).
- `main.rs` returns `slint::PlatformError`.

## Milestone 2 — done (click-state + scheduler)

- What changed:
  - `Cargo.toml`: added `windows = "0.62.2"` (features `Win32_Foundation`,
    `Win32_UI_Input_KeyboardAndMouse` only).
  - `src/lib.rs` (new): crate root exposing `clicker` and `win_input` so the
    unit tests + integration test can import the pure logic; the binary
    (`main.rs`) uses the lib.
  - `src/win_input.rs` (new): one function `left_mouse_down_up()` sends a left
    down+up pair in a single `SendInput` call at the current pointer position
    (zero coords, no move flags). Returns `Err(WindowsError{code,message})`
    when `SendInput` reports fewer sent events than asked; the caller must not
    claim a click succeeded in that case.
  - `src/clicker.rs` (new): `Worker` (worker thread + `WorkerHandle`).
    - `command_rx.recv_timeout(interval)` — sleeps between clicks; blocks
      (no busy-wait / high-frequency polling). Stopped state waits a very long
      `recv_timeout` so a `Stop`/`Toggle`/`Quit` wakes it immediately.
    - `validate_cps` clamps to 1..=100 (Rust-side enforcement even though the
      SpinBox already restricts it). `click_interval(cps)=1/cps` seconds.
    - Commands `Start`/`Stop`/`Toggle`/`SetCps`/`Quit`; `SetCps` takes effect on
      the next scheduled click without a restart.
    - On `SendInput` failure it sets `running=false` and emits
      `WorkerEvent::Error` — clicking stops and a message is shown.
    - `WorkerEvent::{Started,Stopped,Error}` reported through the `report`
      closure; `worker.shutdown()` stops, quits and joins the thread.
  - `src/main.rs`: wires `on_toggle_clicking`->`handle.toggle()`,
    `on_cps_changed`->`handle.set_cps`, and the worker's events into the UI via
    `slint::invoke_from_event_loop` + a weak window ref (the supported
    cross-thread mechanism; strong ref only on the UI thread). After
    `window.run()` returns (window closed) it calls `worker.shutdown()`.
  - `ui/app.slint`: added `in input-error` string + `visible` error text;
    callbacks `toggle-clicking()` (Button) and `cps-changed(value)` (SpinBox
    `edited`). `running` + status text + button label are driven only from
    Rust via worker reports, so status and label stay in sync.
- Design/API decisions:
  - The running state lives on the worker thread (authoritative); the UI never
    toggles it directly — it only mirrors `Started`/`Stopped`/`Error`, so the
    Start/Stop button and status can't drift out of sync.
  - Cross-thread UI updates use `slint::invoke_from_event_loop` with
    `window.as_weak().clone()` captured per event; `Weak::upgrade()` returns
    `None` off the UI thread so the upgrade is done inside the invocation.
  - `windows` 0.62.2 `SendInput(&[INPUT], cbsize) -> u32` returns the number of
    events sent; we compare against 2 and map `GetLastError` to a message.
- Tests: `src/clicker.rs` (unit: range clamps, interval = 1/cps, default 10)
  and `tests/rate_validation.rs` (range, full 1..=100 interval inverse,
  bounds). 8 pass. No test performs real mouse clicks.
- Checks (all pass): `cargo fmt --check`, `cargo check --all-targets`,
  `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo build --release`. Smoke: app launches/stays alive.
- Limitations / not yet done: global hotkey (Ctrl+Alt+F6) is still display-only
  text; the in-window Start/Stop is the only way to toggle for now. `SendInput`
  into higher-integrity apps can be blocked by UIPI (documented in SPEC.md).

## Milestone 3 — done (global hotkey)

- What changed (built on the previous session's in-flight edits, which were
  inspected rather than redone):
  - `src/hotkey.rs`: dedicated `sinclicker-hotkey` thread owns the whole
    hotkey lifecycle. Registers Ctrl+Alt+F6 (`RegisterHotKey(None, id,
    MOD_CONTROL|MOD_ALT|MOD_NOREPEAT, VK_F6)`) **on that thread**, pumps the
    per-thread message queue with `MsgWaitForMultipleObjectsEx` (1 event,
    `QS_HOTKEY | QS_ALLINPUT`, `MWMO_NONE`) + non-blocking `PeekMessageW`
    drain — it blocks, never polls. `Activated` only reports; the caller
    (`main.rs`) routes it to the worker's `Toggle` command so the worker stays
    authoritative. `UnregisterHotKey` runs on the same thread before exit.
    Registration failures become `RegistrationFailed(message)`: conflict
    (Win32 1409) is reported as "in use by another application", other codes
    are reported by code only — no cause is asserted (matches the SendInput
    posture). Clicking stays stopped because no `Start` is ever sent.
  - Shutdown event handle: small `EventHandle(HANDLE)` wrapper with a
    documented `unsafe impl Send` (kernel object, no thread affinity; single
    logical owner; `CloseHandle` only in `Hotkey::shutdown` after
    `thread.join()`). No polling to wake the wait — the event is the wake-up.
  - `src/main.rs`: after `window.run()` returns, `hotkey.shutdown()` first
    (set event → join → unregister done in-thread → close handle), then
    `worker.shutdown()`, so no hotkey message can reach the worker during
    teardown and nothing outlives the window close. The hotkey closure gets a
    **cloned** `WorkerHandle` (the worker handle is moved into the button
    callback). The registration-failure delivery to the UI went from
    silently-ignored `let _ =` to an `eprintln!` on `Err`, since before
    `window.run()` a failed delivery would otherwise be invisible.
  - `src/win_input.rs` (Milestone-2 recheck): `GetLastError` was read after
    the best-effort release, so the reported code could have belonged to the
    release call. It is now captured immediately after the failed
    `SendInput`; the best-effort left-up release still runs before returning
    `Err`, and no success is ever claimed when `sent != 2`. No UIPI or other
    cause is asserted in the message.
- Design/API decisions (all verified against the checked-out crate sources,
  not docs of a different version):
  - `windows` 0.62.2: `Win32_System_Threading::CreateEventW`'s signature
    names `Win32_Security::SECURITY_ATTRIBUTES`, so the `Win32_Security`
    feature is required (kept). `SendInput(&[INPUT], cbsize) -> u32`
    (sent count), `RegisterHotKey/UnregisterHotKey(Option<HWND>, i32, ...) ->
    Result<()>`, `MsgWaitForMultipleObjectsEx(Option<&[HANDLE]>, ...)`,
    `PeekMessageW(*mut MSG, Option<HWND>, 0, 0, PM_REMOVE)`, `WM_HOTKEY = 786`
    confirmed in `Win32_UI_WindowsAndMessaging`.
  - `slint::invoke_from_event_loop` is valid **before** `window.run()` on the
    winit backend: the backend (and winit's `EventLoop`/`EventLoopProxy`) is
    created when the global context first builds the platform — i.e. during
    `MainWindow::new()` — and the proxy is installed in `set_platform`
    (verified in `i-slint-backend-winit` `build()`/`new_event_loop_proxy` and
    `i-slint-core` `platform.rs`), so events queued pre-`run()` are delivered
    by the loop once it starts. The remaining `Err` (loop already terminated)
    is now logged, so no important startup failure is lost silently.
  - `Weak<Window>` `Send` is provided by Slint itself; the hotkey thread
    never holds a strong window reference.
- Tests: 15 pass (12 lib unit incl. 4 pure hotkey-logic tests: hotkey
  message/id matcher, conflict message, no-code message, other-code message;
  3 integration rate tests). No test registers a real hotkey or sends real
  mouse input. Two stale `win_input` test assertions (expected substrings
  missing the word "code") were fixed to match the actual, intended message
  text — pre-existing, surfaced only now because they had never been run
  green in this session.
- Smoke checks: release binary stays alive 6s (hotkey thread live, no stderr).
  Two concurrent instances: both stay up, no stderr — the second's
  `RegisterHotKey` conflict is handled (its message routed to the UI, no
  crash/silent drop). Note: this machine state proves the path runs; the
  in-window conflict text was not captured in a screenshot.
- Checks (all pass, 2026-10-05): `cargo fmt --check`,
  `cargo check --all-targets`, `cargo test`,
  `cargo clippy --all-targets -- -D warnings`, `cargo build --release`.
- Limitations: no real-toggle smoke performed (would move the cursor and
  click); hotkey conflict in-window text visually unverified; `SendInput`
  into higher-integrity apps can still be blocked by UIPI (documented in
  error text and SPEC.md; the app never asserts the cause).

## Milestone 4 — done (hotkey customization + persistence)

- `src/settings.rs` (new): the supported-binding catalog — 23 Ctrl+Alt-based
  choices (F6–F16 minus F12, Insert/Delete/Home/End/PageUp/PageDown, three
  Ctrl+Shift+Alt variants, Space, Backspace, Period, A), canonical display
  parse/format, and persistence of the chosen display string under
  `HKCU\Software\SinClicker\Hotkey` (REG_SZ). Unknown/stale stored values fall
  back to the default rather than crashing.
- `src/win_hotkey.rs` (split out of `hotkey.rs`): the thin Windows surface —
  `RegisterHotKey`/`UnregisterHotKey` on the calling thread, a monotonically
  increasing hotkey-id pool (each registration gets a fresh id, so a
  `WM_HOTKEY` queued by a superseded binding is ignored instead of acting as a
  stray toggle), `MOD_NOREPEAT`, and failure messages that report only what the
  API establishes (1409 → conflict; anything else by code).
- `src/hotkey.rs`: the thread now services `Register` commands:
  register-new-then-retire-old, so a failed re-registration leaves the previous
  binding active (it is never retired before the new one is confirmed). A
  same-binding request is a no-op fast-path — the combination is still held
  process-wide, and re-registering it would fail Win32 1409 and surface as a
  false "already in use" error. `set_binding` blocks (bounded) for the outcome
  and returns the authoritative still-active binding in its report.
- `src/main.rs`: the persisted choice is loaded at startup (fallback:
  default), the hotkey is a ComboBox (model = the catalog itself, so the UI and
  Rust never drift apart), and `on_hotkey_changed` validates the display, calls
  `set_binding`, persists on `Ok`, and on `Err` shows the failure text and
  restores the selection to the binding that is really active.
- `ui/app.slint`: text hotkey label replaced by the ComboBox.
- `SPEC.md`: reconciled with what shipped — "hotkey customization is out of
  scope" replaced by "switchable among a supported list of Ctrl+Alt
  combinations (arbitrary combinations rejected), choice persists across
  launches"; "hotkey rebinding" in the out-of-scope list is qualified to
  rebinding outside the supported list.
- Tests: 22 lib unit + 3 integration (settings catalog/parse/format and
  win_hotkey messages added). No test registers a real hotkey or sends real
  input.
- Checks (all pass, 2026-10-05): `cargo fmt --check`,
  `cargo check --all-targets`, `cargo test`,
  `cargo clippy --all-targets -- -D warnings`, `cargo build --release`.

## Milestone 5 — done (end-to-end manual verification)

- A disposable UIA-driven smoke harness (outside the repo, C# / .NET 4.0 x64
  `csc`, under `%TEMP%`) drives the release binary: real hotkey presses via
  `SendInput`, real click counting with a `WH_MOUSE_LL` hook, registry reads
  via P/Invoke, and a parameterized `holder.exe` that takes a hotkey to create
  known conflicts. Full run, 2026-10-05: **45/45 checks pass**. Coverage:
  - Launch: no console, window found; exit code 0 on close.
  - Start/Stop: 22 down/up pairs in 2.5 s at 10 CPS (balanced), status and
    button label track the state, no clicks after Stop.
  - Hotkey `Ctrl+Alt+F6` toggles ON and OFF globally.
  - Change to a free binding (F9): persisted, old binding retired (its key no
    longer toggles), new key works both directions.
  - Change to a taken binding (holder holds F8): error names the taken binding
    and says the previous binding stays active; selection rolls back to the
    still-active F9; the taken binding is not persisted; the holder's key does
    nothing; F9 keeps working; the error clears on the next Start/Stop.
  - Same-value re-selection (arrow key clamped at a model edge still fires
    Slint's `selected()`): the no-op fast-path absorbs it — no false 1409
    "already in use", combo and registry unchanged.
  - Orderly close: registry choice survives; a fresh holder can then register
    the same key (proves the app unregistered on exit).
  - Relaunch: persisted binding is loaded, clicking not auto-started.
  - Launch with the persisted binding held: app starts Stopped, shows the
    conflict naming the held binding plus "No global hotkey is active", and
    keeps the persisted choice displayed.
- Not run (reasons): UIPI input into a higher-integrity application —
  exercising it safely needs an elevated target process, which the harness is
  deliberately not (the behavior is documented in-app and in SPEC.md instead);
  live toggling of the Windows light/dark setting while the app is open —
  follow-behavior was verified against the 1.18.1 backend source in Milestone
  1, not by live re-toggle in this pass.
- Checks (all pass, re-run this session after the harness work): `cargo fmt
  --check`, `cargo check --all-targets`, `cargo test` (25 pass), `cargo
  clippy --all-targets -- -D warnings`, `cargo build --release`.

## Status

Milestones 1–5 complete: SPEC.md v1 scope (window, clicker, global hotkey)
plus supported-list hotkey customization with persistence are implemented and
verified end-to-end against the release binary. No further milestones planned
unless new requirements arrive.
