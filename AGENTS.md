# SinClicker — maintainer and agent guide

A small Windows-only autoclicker: Rust + Slint UI, `windows` crate for Win32.
Left-clicks at the pointer at 1–500 CPS, toggled from the window or a global
hotkey (default Ctrl+Alt+F6). Settings persist in `HKCU\Software\SinClicker`.

## Layout

| Path | What lives there |
| --- | --- |
| `src/main.rs` | App wiring only: loads settings, spawns the worker and hotkey thread, connects UI callbacks, orderly shutdown. |
| `src/clicker.rs` | Click cadence (pure, unit-tested deadline math) and the worker thread that owns the running state. |
| `src/hotkey.rs` | Hotkey thread: registration, message pump, re-registration commands, unregistration. |
| `src/settings.rs` | Hotkey binding model and CPS/hotkey persistence in the registry. |
| `src/win_*.rs` | Thin Win32 surfaces, one per concern: `win_input` (`SendInput`, own-window check), `win_hotkey` (`RegisterHotKey`, failure messages), `win_scheduler` (waitable-timer wake backend), `win_event` (owned event handle). |
| `ui/app.slint` | The whole UI; compiled by `build.rs`. |
| `tests/` | Integration tests. `cps_persistence.rs` touches the real HKCU value and restores it. |
| `docs/SPEC.md` | The original product requirements (v1). |
| `docs/HISTORY.md` | Append-only milestone log: decisions, measurements, manual verification. Read only the section you need. |

## Validate before finishing

Run from the project root:

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

UI changes also need a look at the running release build: launch it, check it, and close it. Report anything you could not run.

## Rules that must hold

- **Threads:** the UI stays on Slint's thread, and background threads reach it only through `upgrade_in_event_loop`/`invoke_from_event_loop`. Hotkey message handling and the click scheduler stay off the UI thread.
- **Running state:** the worker is the single authority. Hotkey presses and the Start/Stop button route through `worker.toggle()`.
- **Scheduling:** sleep until the next deadline or a command. Never busy-wait or poll, and never change the system timer resolution.
- **Shutdown order:** hotkey thread first (unregisters, so no more toggles), then the worker. Join both.
- **Input:** use only `SendInput` left down/up. Never claim a click `SendInput` rejected. No drivers, injection, or elevation; document UIPI limits instead.
- **Safety in the UI:** clicking state is always visible, Stop is always reachable, and the app stops when closed. If hotkey registration fails, show the error and leave clicking as it is; never silently fall back to another hotkey.
- **CPS range:** enforced in both the UI and Rust (`clicker::validate_cps`). The UI gets the range from Rust.
- **Windows code:** keep it in the `win_*` modules. Keep `unsafe` blocks minimal, each with a `// SAFETY:` comment.
- **Dependencies:** keep them minimal. No async runtime, WebView, database, tray/service, or theme polling. Enable only the `windows` features the code uses.
- **Appearance:** use the Slint fluent style and follow the system light/dark theme through `Palette`. No forced light or dark theme, and no continuous animation or redraw.

## Known pitfalls

- Slint is pinned to 1.18.1, and the "latest" docs online describe a newer API. Check names against the crate source in `~/.cargo/registry/src/*/i-slint-*-1.18.1/`.
- Slint is built with the software renderer only (`default-features = false`). Measured: about 22 MB working set versus about 74 MB with the GPU renderer.
- A `FocusScope` above a `TouchArea` swallows pointer events. Put the `TouchArea` inside the `FocusScope`.
- Clicks are skipped while the pointer is over SinClicker's own window, so UI tests that press Start with the mouse will not see clicks land on the app.
- A registered global hotkey is delivered to the hotkey thread, never to the focused window.
- Never send a real hotkey or click in automated checks unless the pointer is parked somewhere harmless.

## Working style

- Make focused patches, and don't rewrite files that don't need it. Match the existing comment density: comments explain *why* and the Win32 contracts.
- Read only the files relevant to the change. Don't dump lockfiles, dependency trees, or long build logs.
- After a substantial change, add a short dated entry to `docs/HISTORY.md`: what changed, checks run, and anything left unverified.
