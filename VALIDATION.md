# Build and validation checklist

Run commands from the project root in PowerShell.

## Build checks

Run these during implementation:

```powershell
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release