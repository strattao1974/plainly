# Plainly

Free Windows tray app (Rust + egui) that explains the PC in plain English: change timeline, "why is it slow?", "is it me or the internet?". Meant for anyone to download, not sold. See README.md for features and the file map.

## Build & test

- If `cargo` isn't on PATH, use `%USERPROFILE%\.cargo\bin\cargo.exe` or prepend that folder to PATH.
- Installer: `iscc installer\plainly.iss` (Inno Setup 6; a per-user install puts ISCC in `%LOCALAPPDATA%\Programs\Inno Setup 6`).
- Releasing: bump the version in `Cargo.toml` and `installer\plainly.iss`, build both, tag `vX.Y.Z`, and attach `PlainlySetup.exe` to a GitHub release. The in-app update check reads the latest release's tag.
- `cargo test` (fast unit tests) · `cargo test -- --ignored --nocapture` (live reads of this PC/network).
- Debug builds have a console, so panics in background threads show on stderr. Release builds use the windows subsystem.

## Architecture

Two processes from one exe (see README "How it works"): the background watcher owns the tray and is the only writer of `timeline.jsonl`/`status.json`; the window (`--window`) owns `settings.json` and exits on close. Keep the watcher free of egui/GPU work, because its small footprint is a feature.

## Conventions

- Pinned crates: eframe 0.31, windows 0.58, winreg 0.52. Check exact signatures in `~/.cargo/registry/src/*/windows-0.58.0` rather than guessing.
- Everything must work without admin rights.
- All user-facing text is plain English for non-technical people. Explain *why*, and offer a fix where one exists.
- Windows' own routine churn is marked `noise` (hidden by default), never dropped.
- No telemetry; the only network calls are the documented internet checks (README "Privacy").
