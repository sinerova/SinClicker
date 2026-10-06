# Project instructions for OpenCode

## Goal

Build a small, Windows-only autoclicker in Rust with a Slint UI. Follow
`SPEC.md` and `VALIDATION.md` as the requirements.

## Working method

1. Inspect the repository and the installed Rust toolchain before making
   assumptions.
2. Start by summarizing a short implementation plan.
3. Implement the app in small, buildable steps.
4. After each substantial step, run the appropriate Cargo check or test.
5. Before finishing, run every validation command in `VALIDATION.md` that is
   supported by the current environment. Clearly report anything that could not
   be run.

Do not stop after writing a plan or a scaffold. Continue through implementation,
compilation, and fixes.

## Technical requirements

- Use stable Rust and Slint for the UI.
- Use Slint's standard widget style and prefer its system light/dark behavior.
  Do not hard-code a light-only or dark-only style.
- Use the `windows` crate for Windows APIs. Enable only the API features the
  implementation actually needs.
- Verify Windows API names, modules, signatures, and feature flags against
  current Rust for Windows documentation or compiler diagnostics. Do not guess
  APIs or copy unverified examples.
- Keep the user interface on Slint's UI thread. Send background-thread updates
  to it using Slint's supported event-loop invocation mechanism.
- Use a global hotkey registered through Windows. Keep its message handling off
  the Slint UI thread so a blocked message loop cannot freeze the interface.
- Use `SendInput` for ordinary left mouse down/up events. Do not use drivers,
  kernel code, process injection, or privilege escalation.
- Keep the click scheduler off the UI thread. Use a sleeping/timed wait, not a
  busy loop or a rapidly polling timer.
- Make shutdown orderly: stop clicking, release or unregister the hotkey, and
  join background threads where practical.

## Resource and implementation discipline

- This is a small utility. Avoid Tokio, a web frontend/WebView, databases,
  service processes, plugins, and unnecessary dependencies.
- Do not add background polling for UI state or system theme.
- Avoid needless animations, transparency, blur, continuous redraws, or
  decorative effects.
- Prefer simple, readable code to unsafe or complicated micro-optimizations.
- Keep Windows-specific code in a small module, separate from UI state and
  click scheduling.
- Keep `unsafe` blocks as small as possible and explain the safety assumptions
  immediately next to them.
- Do not request administrator privileges. Document that Windows may block
  simulated input into higher-integrity applications.

## Safety and behavior

- Clicking must be visibly enabled or disabled in the UI.
- The global hotkey must toggle clicking and be shown in the UI.
- Provide an obvious in-window Stop control.
- Stop immediately when the user closes the app.
- Enforce the CPS range in both the UI and Rust logic.
- If the hotkey cannot be registered, show a useful error and leave clicking
  stopped.
- Never claim a click was sent if `SendInput` reports failure.

## Context and output discipline

- Work in small milestones. Do not implement the whole app in one giant change.
- At the start of a milestone, read only the relevant project files and this
  milestone's requirements.
- Prefer targeted file reads and concise command output. Do not dump entire
  dependency trees, lockfiles, generated files, or long build logs into chat.
- Make focused patches; do not rewrite files that do not need changes.
- After each milestone, report briefly:
  - what changed,
  - what checks were run and their result,
  - any remaining issue,
  - the next milestone.
- Keep a short progress note in `PROGRESS.md` if work spans multiple sessions.
  Update it with decisions and current build status; do not copy full source
  files into it.