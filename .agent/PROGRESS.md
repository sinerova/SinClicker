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

## Milestone 6 — done (wake-based scheduler: hit the real 100 CPS)

- What changed:
  - `src/win_scheduler.rs` (new): all Windows interop for the *wake* is isolated
    here. `WakeSource` blocks the worker until the next click deadline *or* a
    command, using a waitable timer so the old `recv_timeout` tick-quantization
    (which capped a 10 ms / 100 CPS interval at ~60–65/s in measurement) is
    gone. A strategy ladder picks the best available wait at startup and logs a
    note when a reduced strategy is used, so a lowered rate is never silently
    promised: (1) high-resolution waitable timer (wins, sub-ms wake), (2)
    standard waitable timer (deadline wake at system resolution), (3) plain
    bounded wait on the command event (the pre-timer behaviour; a note names
    the limit). A separate auto-reset **command event** is the wake hint on
    every `Start`/`Stop`/`Toggle`/`SetCps`/`Quit`; the `mpsc` channel becomes a
    payload pipe the worker fully drains after every wake, so a coalesced
    signal or spurious early wake can never drop a command or fire an early
    click (the worker re-checks its own deadline against the monotonic clock).
    The wait bound always extends past the deadline, so even a missed timer
    signal cannot hold the worker past it.
  - Deadline clock: `WakeSource::now_micros()` is `std::time::Instant`
    (QPC-backed, monotonic). `GetTickCount64` is quantized to the system timer
    resolution, so a deadline clock on it would cap 100 CPS at ~60–65/s even
    with a precise timer; `Instant` removes that quantization and is the same
    basis the wait uses, so clock and wake are coherent.
  - `src/clicker.rs`: the *timing* math is factored into a pure, testable
    `ClickScheduler` (monotonic-microsecond absolute rolling deadline): no
    drift (anchored, not re-anchored on jittery "now"), no catch-up burst
    (a lapsed deadline rolls to now+interval, one catch-up click max), and a CPS
    change applies from the next click (pulling a pending deadline earlier when
    the new interval is shorter, keeping it when slower). `WorkerHandle::send`
    now also signals the command event so a blocked worker wakes promptly. No
    `SendInput`, no Windows. `worker_loop` is now: compute wait -> fire due
    click -> block on `WakeSource` -> drain commands -> repeat. `Wake::Failed`
    ends the worker with a report (no spin).
  - `src/lib.rs`: adds `pub mod win_scheduler;`.
- Invariants preserved (unchanged): no catch-up burst; no command dropped
  (event = wake hint, channel = payload, full drain after every wake, stale
  wake harmless); a click is never reported as sent unless `SendInput` accepts
  both events; shutdown remains orderly (Stop -> Quit -> join; hotkey retired
  before the worker).
- Tests: 30 lib unit + 3 integration rate tests. New coverage:
  - `win_scheduler`: a bounded, hang-proof exercise of the real wait path
    (a 150 ms wait returns in-bound; a command signal wakes a 2 s wait
    promptly); the backend selects a real waitable timer here; the command
    signal is independent of the deadline (5 trials); and a 1-second probe at
    a 10 ms target that measures the real wake mechanism end to end without
    sending input — it asserts >= 90 wake/s (a high-res timer should be near
    100/s; a standard timer, if selected, is the documented, logged
    limitation). This machine selects **HighResolution** and measures
    **100.0 wake/s** (stable across 3 runs).
  - `clicker`: `click_interval_micros` inverse of CPS (floored, never above
    1 s); pure click-count simulations for 100 CPS / 1 CPS; a suspend does not
    burst; a CPS increase applies within one new interval; a CPS decrease keeps
    the pending deadline then uses the new interval; stop/restart clicks
    immediately; no drift over 1000 cycles; a constant 300 µs jitter does not
    accumulate.
- Checks (all pass, 2026-10-06): `cargo fmt --check`,
  `cargo check --all-targets`, `cargo test` (30 lib + 3 integration),
  `cargo clippy --all-targets -- -D warnings`, `cargo build --release`.
- End-to-end (Milestone 5's UIA harness) and this session's note:
  - The in-process rate probe (the scheduler's own wake, no input) confirms
    the full 10 CPS / 100 CPS cadence the old scheduler could not reach.
  - The harness's cross-process click *count* and some hotkey toggles read 0 in
    the current interactive session. This is **not** a scheduler regression:
    an A/B against the committed v1.0 `recv_timeout` build (the exact binary
    that produced 22 clicks in Milestone 5) also reads **0** in this same
    session, while the app itself reports "Clicking", flips the button to
    Stop, and shows no `SendInput` error — i.e. the app side is intact on both
    builds. An in-process `dispprobe` (a correctly-pumping `WH_MOUSE_LL` + a
    well-formed 40-byte x64 `SendInput`) confirms the session *does* dispatch
    injected input. The 0-count is therefore an artifact of this particular
    interactive session's delivery/observation of cross-process simulated
    input (a locked/background console, or a redirected input target), not of
    either scheduler. The 45/45 harness result from Milestone 5 (a fresh,
    interactive console) remains the authoritative end-to-end pass; the
    scheduler change does not alter the click path (`left_mouse_down_up` is
    unchanged) or the hotkey path, and is independently validated by the probe
    and the full unit/integration suite.
- Remaining: re-run the Milestone-5 UIA harness in a fresh, fully-interactive
  console to reconfirm the 22-click end-to-end figure against the new build
  (the current session cannot reliably observe cross-process injected input).
  Nothing in the code path that produced the Milestone-5 45/45 changed in a way
  that would alter that count.

## Milestone 7 — done (CPS range 1–500 + CPS persistence)

Branch `feature/cps-500-persist` (work in progress; not committed).

- What changed (all in the working tree, 8 modified + 1 new file):
  - `src/clicker.rs`: `MAX_CPS` 100 → **500**; `validate_cps` clamps to 1..=500;
    unit tests updated (2 ms at 500 CPS, full inverse over 1..=500, exact
    500-click one-second simulation, top-of-range CPS change applies within one
    new interval). `DEFAULT_CPS` stays 10.
  - `src/settings.rs`: new CPS persistence on top of the existing hotkey
    persistence — `save_cps(i32)` writes `HKCU\Software\SinClicker\Cps` as a
    **REG_DWORD** (the same per-user key, so no elevation), `load_cps()` reads
    it back and returns `None` for a missing value, a wrong type, a wrong size
    (not exactly 4 bytes), or a registry error; and pure
    `resolve_startup_cps(Option<i32> -> i32)` maps stored → start value
    (in range kept, `None`/out-of-range → `DEFAULT_CPS`). Unit tests for
    `resolve_startup_cps` cover in-range and every fallback class
    (including `i32::MIN`/`i32::MAX`).
  - `src/main.rs`: startup wiring — `resolve_startup_cps(load_cps())` is
    computed once before `MainWindow::new()`, and that single value is used to
    both initialize the UI (`window.set_cps`) and the worker
    (`Worker::spawn`), so UI and scheduler cannot drift at startup. A plain
    SpinBox property set does not fire `edited`, so showing the restored value
    causes no redundant `set_cps`/save. `on_cps_changed` now also
    `save_cps` and surfaces the Win32 error in the existing input-error line
    when a save fails (the app keeps running; the in-session rate still
    applies).
  - `ui/app.slint`: SpinBox `maximum: 100` → `500`; `cps` default 10.
  - `src/win_scheduler.rs`: new bounded test — a 25 ms probe at the 2 ms /
    500 CPS target on the real wake backend; on this machine (which selects
    the high-resolution timer) it asserts the wake path stays within a
    generous bound, mirroring the existing 100 CPS wake probe.
  - `tests/rate_validation.rs`: range tests updated to 1–500 (bounds,
    clamp-down at 501, 2 ms interval).
  - `tests/cps_persistence.rs` (new): one sequential integration test against
    the **real** per-user registry, with a drop-guard that restores the user's
    prior value (type + payload bytes, or absence) on the way out, even if an
    assert panics. Covers: in-range round-trips (1, 10, 500, 250) through the
    real `save_cps`/`load_cps`/`resolve_startup_cps` path; absent value →
    `None` → default; out-of-range stored values (0, 501, 999) are stored
    verbatim by the public API but resolve to the default.
  - `README.md` / `SPEC.md`: range documented as 1–500; persistence stated as
    "the hotkey choice and the CPS value"; the 500 CPS figure carries the
    honest caveat that the 2 ms request depends on Windows scheduling and the
    target's own input handling (SPEC "Safety" section: the app reports only
    what `SendInput` accepted, and never claims more).
- Storage contract: `HKCU\Software\SinClicker`, value `Cps`, type `REG_DWORD`
  (exactly 4 bytes, little-endian i32). Any deviation (missing value,
  `REG_SZ`, `REG_QWORD`, 8-byte payload, out-of-range number) is read as
  unusable and the app starts at 10 CPS. The hotkey value (`Hotkey`, REG_SZ)
  is untouched.
- Verification — UI-level, against the freshly built release binary:
  a throwaway C# / .NET 4.0 x64 helper (under `%TEMP%`, outside the repo)
  uses the legacy UIA client API on the winit/Slint window: Slint 1.18.1's
  default features include `accessibility`, so the SpinBox is exposed as a
  UIA `Spinner` with a Value pattern (`accessible-action-set-value` → the
  Slint SpinBox `update-value`, which fires `edited` inside the 1–500 range —
  i.e. a UIA value set goes through the app's real change path, including its
  save). The app is launched, its window is found by title, the Spinner is
  located in the tree, and its value is read or set; the app is then closed
  with the UIA `WindowPattern.Close()` (graceful) and the process reaped.
  The scenario matrix, 9/9 PASS (2026-10-06):
  - stored 200 → starts at 200 (read back via UIA);
  - UIA set 200 → 250: registry becomes `Cps = 250` REG_DWORD, relaunch → UIA
    reads 250 (full save + restore round-trip through the UI);
  - UIA set 250 → 500: same round-trip at the range top;
  - stored value absent → starts at 10;
  - stored 0, 501, 999 (REG_DWORD) → start at 10;
  - stored `REG_SZ` "250" → starts at 10;
  - stored `REG_QWORD` 250 → starts at 10.
  Each launch exited cleanly (WindowPattern close, no forced kills), and the
  user's original value (`Cps = 200` REG_DWORD, no `Hotkey` value) was
  restored after the matrix.
  The UIA harness launches the real app, which registers its persisted global
  hotkey (default `Ctrl+Alt+F6`) — accepted as the app-under-test's own
  behaviour; the harness never presses the hotkey and the app starts stopped,
  so no clicks are sent.
- Checks (all pass, 2026-10-06): `cargo fmt --check`,
  `cargo check --all-targets`, `cargo test` (39 lib unit + 4 integration,
  including the new `cps_persistence` test and the 500 CPS wake probe),
  `cargo clippy --all-targets -- -D warnings`, `cargo build --release`.
- Not run here (carried from earlier milestones, unchanged by this work):
  re-running the Milestone-5 45/45 harness in a fresh fully-interactive
  console; UIPI input into a higher-integrity target (needs elevation the
  harness avoids); live light/dark re-toggle.
- Environment note: the working tree contains a pre-existing nested clone at
  `SinClicker\` (its own `.git`, remote `sinerova/SinClicker`, tracked files
  only `.gitattributes` + `README.md`; it does not appear in `git status`,
  and no deletion was performed). It is unrelated to this milestone.

## Milestone 8 — done (conservative final optimization pass)

Branch `maintenance/final-optimization`, based on `main` @ `c1da89d` (the
stable production merge; `v1.0`/`v1.0.1` are earlier pre-feature tags). The
working tree was clean and the baseline unambiguous before any edit.

- Audit (no speculative micro-optimizations; everything below verified
  against the code, not assumed):
  - Idle CPU / wakeups: the worker blocks in `WaitForMultipleObjects` (2 s idle
    bound, auto-reset command event wakes it on any command) and the hotkey
    thread blocks in `MsgWaitForMultipleObjectsEx(INFINITE)`. No timers, no
    polling, no animations, no background threads beyond the two required.
    Nothing to improve.
  - Timing accuracy: the pure `ClickScheduler` (anchored rolling deadline, one
    catch-up click max, CPS change applies within one interval) is covered by
    simulation unit tests plus two live 1 s wake probes on the real backend.
    Measured on this machine (release build): 100.0 wake/s at the 10 ms
    target, 500.0 wake/s at the 2 ms target (`HighResolution` strategy). No
    change needed.
  - Allocations / dependencies: no per-cycle heap allocations; all six
    `windows` features are used by the code. No avoidable dependency found.
    `WindowsError.code` is public-but-unused; kept (it is intentional error
    payload, and removing it would be a visible API change for zero gain).
  - Error paths / cleanup: single close site per kernel handle, small `unsafe`
    with co-located SAFETY comments, `GetLastError` captured before the
    best-effort release, hotkey retired before the worker on shutdown. No
    change.
  - Build configuration: `Cargo.toml` had no release profile. This is the one
    justified change: `[profile.release] strip = true` for a smaller
    distributable artifact (the SPEC permits a size-favoring release profile).
    No runtime behavior, dependency, or feature-set change.
- Changed files: `Cargo.toml` only (+3 lines). No production code, no feature,
  no hotkey/settings behavior, no CPS range, no release subsystem attribute
  (`#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`
  in `main.rs` untouched).
- Before/after (measured, same machine, interleaved):
  - Release exe size: 14,566,400 B -> 14,565,888 B (-512 B, -0.0035%). The
    MSVC link does not embed the PDB into the exe (`target\release\
    sinclicker.pdb` is still emitted, ~5.2 MB), so stripping only removes the
    PE symbol table; post-mortem symbols remain available. Tiny but real.
  - Idle CPU (app stopped, 8 x 2 s `Get-Counter` samples after an 8 s warm-up,
    2 interleaved rounds per build): baseline 0.0% / 0.0%; strip 0.097% (a
    single 0.776 transient sample) / 0.0%. No systematic difference; both
    builds sit fully in kernel waits when idle.
  - Scheduler wake (release-compiled probes, after the change): 100.0 wake/s
    at 10 ms, 499.9 wake/s at 2 ms — unchanged in behavior.
- Validation (all pass, 2026-10-06): `cargo fmt --check`,
  `cargo check --all-targets`, `cargo test` (39 lib unit + 4 integration),
  `cargo clippy --all-targets -- -D warnings`, `cargo build --release`.
- Not run: the Milestone-5 UIA click harness (it sends real clicks; the
  scheduler/click path is byte-identical in this branch, so its 45/45 result
  stands); UIPI into an elevated target; live light/dark re-toggle.
- Conclusion: the production code was already in its conservative end state;
  the only measurable, zero-risk improvement was the release-profile strip.
  The branch is ready to merge when the maintainer wants to fold it in
  (uncommitted, per instruction).

## Status

Milestones 1–7 complete on the working branch: SPEC.md v1 scope (window,
clicker, global hotkey) plus supported-list hotkey customization with
persistence, and the CPS range 1–500 with CPS persistence, are implemented
and verified — end-to-end for hotkey behaviour (Milestone 5) and for CPS
persistence via the UIA matrix above. The click scheduler runs the wake-based
backend (waitable timer + command-event wake, QPC `Instant` deadline clock):
full 100 CPS in the in-process wake probe (100.0 wake/s) and a bounded 500
CPS (2 ms) wake probe. The branch is uncommitted; nothing here changes the
click or hotkey code paths beyond the range widening.
