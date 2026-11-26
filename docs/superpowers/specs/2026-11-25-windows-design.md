# MasterKeys for Windows 11 — design

## Purpose

A friend of the user runs Windows 11 with a Redragon K745 PRO keyboard and an M612 mouse and wants to use MasterKeys. This phase brings the device-independent features to Windows: key remaps, macros (all playback modes and macro text), profiles and auto-switch by running program. Redragon lighting and the mouse come in a later phase, after the friend captures their USB traffic.

The friend is the only Windows tester. He downloads builds from GitHub and gets updates from inside the app.

## Success criteria

- He downloads `MasterKeys-windows.zip` from a GitHub Release, unzips it and runs `masterkeys.exe`. SmartScreen warns once because the exe is unsigned.
- Remaps and macros he sets up work in normal desktop programs, and the escape chord releases everything.
- A profile linked to a program's `.exe` turns on when that program starts.
- When a newer Release exists, the app offers **Update**, which installs it and restarts.
- The Linux build and its behaviour are unchanged, and all existing tests still pass.

## Constraints

- One codebase. Platform code sits behind `cfg(windows)` / `cfg(target_os = "linux")` seams, and shared logic stays shared.
- Profiles, binding key names and macro text are identical on both systems, so a profile file works on either.
- The public repo is `github.com/I-MnM-I/MasterKeys`. The forked steelseries-gg code keeps its MIT license, and the README credits it.
- Before the first push, history is rewritten so commits use `343607561+I-MnM-I@users.noreply.github.com`, and the repo's git config is set to that address.
- Never send `00 09 00` to an Apex 3 (unchanged).

## Architecture

### Process shape

| | Linux (unchanged) | Windows |
|---|---|---|
| Programs | `masterkeysd` (systemd user service) + `masterkeys` GUI | one `masterkeys.exe`, living in the tray |
| GUI ↔ core | D-Bus session bus | the same zbus interface over an in-process peer-to-peer connection on a loopback TCP pair |
| Core runs | always, as a service | while the app runs; quitting from the tray stops remaps |

The daemon crate's library (`DaemonCore`, `DaemonService`, the engine, profiles and persistence) is started inside the GUI on Windows, on its own tokio runtime thread. `client.rs` and `service.rs` keep their D-Bus interface, so the GUI code is shared. Key subscription already handles a peer-to-peer caller (`P2P_PEER`).

### Platform seams (Windows side)

- **Key input:** a `WH_KEYBOARD_LL` hook on a dedicated thread with a message loop. It turns hook events into the same `KeyEvent`s the engine consumes. While the profile has bindings, it works like the Linux grab: it swallows every physical key and replays it through the engine, so unbound keys come out unchanged and the escape chord is seen. If the core stops answering (heartbeat older than 1 s), the hook passes keys straight through, so the keyboard never goes dead. With no bindings, nothing is swallowed. The escape chord (both Ctrls + both Shifts for 2 s) is detected the same way it is on Linux.
- **Key output:** `SendInput` with scan codes. Injected events carry a marker in `dwExtraInfo` so the hook ignores its own output.
- **Key codes:** a table maps Windows scan codes (including extended keys) to the evdev codes the engine and layout already use, in both directions. Unknown codes pass through untouched.
- **Running programs:** `CreateToolhelp32Snapshot` + `QueryFullProcessImageNameW` fill `ProcessInfo` (exe path; cmdline left empty). The existing `matches` / `display_name` work unchanged.
- **Tray:** the `tray-icon` crate on Windows replaces ksni. It has the same menu (active profile, switch profile, open window, quit).
- **Single instance:** a named mutex. A second launch signals the first (named event) to show its window, then exits.
- **Start with Windows:** a checkbox in the GUI writes or removes `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\MasterKeys`, pointing at the exe with `--tray`.
- **Config and log:** `%APPDATA%\MasterKeys` holds profiles and `masterkeys.log`, which is rotated at 1 MB.
- **Hidden on Windows:** the lighting panel, Apex 3 output and GameSense, since no supported device is present. The keyboard picture (full-size Apex 3 layout) stays for picking keys.

### Updates

- At startup, and then every 24 h while running, the app fetches `https://api.github.com/repos/I-MnM-I/MasterKeys/releases/latest` and compares its tag to the build's `CARGO_PKG_VERSION` (semver).
- If the tag is newer, a bar shows "Version X is available".
  - **Windows:** the bar has an **Update** button. It downloads the release's `MasterKeys-windows.zip` and `SHA256SUMS`, checks the hash, extracts the new exe next to the current one, renames the running exe to `masterkeys.exe.old`, moves the new one into place, starts it and quits. The next launch deletes `.old`.
  - **Linux:** the bar links to the release page only.
- Any failure (offline, rate limit, bad hash) leaves the current version untouched. A failed check shows nothing; a failed Update shows its error in the bar.

### Build and release (GitHub Actions)

- **CI on every push and PR:**
  - Linux job: `cargo clippy --workspace --all-targets` + `cargo test --workspace`, with the system packages Slint and evdev need.
  - Windows job (`windows-latest`, MSVC): clippy + tests for the crates that build on Windows.
- **Release on a `v*` tag:**
  - The Windows job builds `--release` and zips `masterkeys.exe`, `README-windows.txt` and `LICENSE` into `MasterKeys-windows.zip`.
  - It writes `SHA256SUMS` and publishes both to a GitHub Release named after the tag.
- **Versioning:** the workspace version in Cargo.toml matches the tag. A CI step fails the release if they differ.

## Testing

- **Shared logic** keeps its Linux tests.
- **New unit tests, runnable on Linux:**
  - the scan-code ↔ evdev table round-trips every key in the layout
  - version comparison and the update decision
  - `SHA256SUMS` parsing and verification
  - choosing which hook events to swallow from a set of bindings
  - the Run-key command line
- **Windows-only code** (hook, SendInput, tray, registry, process snapshot) is compiled and tested on the Windows CI runner. What can't be automated goes on a checklist the friend runs for each Release, shipped in `README-windows.txt`.

## Known limits (go in README-windows.txt)

- Games that read the keyboard directly (raw input, anti-cheat) may ignore remaps.
- Programs run as administrator see remaps only if MasterKeys also runs as administrator.
- The exe is unsigned, so SmartScreen warns on first run.

## Out of scope for this phase

- Redragon K745 PRO and M612 lighting, and mouse button remaps.
- An installer, code signing, and Linux packaging changes.
