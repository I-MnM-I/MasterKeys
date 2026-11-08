# MasterKeys — Design

Date: 2026-11-08
Status: approved in conversation, pending written-spec review

## Goal

A Linux desktop app, written in Rust, that fully replaces SteelSeries GG for
one keyboard: the **SteelSeries Apex 3, full size, US ANSI** (USB VID `0x1038`,
PID `0x161a`). Target system: Bazzite (Fedora Kinoite), KDE Plasma on Wayland.

Success: the user can uninstall/never need GG. Lighting, effects, profiles with
auto-switch, key remapping, macros, tray control and GameSense all work on the
real keyboard.

**Function first.** v1 uses Slint's stock widgets and a plain layout. Visual
polish, custom theme and PhotoCraft-made assets are a later pass and out of
scope here.

## Decisions

| Topic | Decision |
|---|---|
| Shape | Background daemon owns the keyboard; GUI is a control panel |
| GUI toolkit | Slint |
| Base library | Vendored fork of `Ven0m0/steelseriesgg-rs` (MIT), pinned to a commit, trimmed to Apex 3 only |
| IPC | D-Bus via `zbus` (session bus) |
| Config | TOML files in `~/.config/masterkeys/` |
| Auto-switch trigger | Process start/exit (focus-based switching deferred) |
| Keyboard layout | US ANSI, data-driven so it can be swapped |

## Architecture

Cargo workspace, four crates:

```
crates/
  steelseries-gg/    vendored fork (library only)
  masterkeys-core/   shared types + D-Bus interface definitions
  masterkeysd/       daemon (systemd user service)
  masterkeys/        Slint GUI + tray icon
```

### `steelseries-gg` (vendored fork)

Copied from upstream at a recorded commit (noted in `crates/steelseries-gg/UPSTREAM.md`
with the commit hash and list of local changes).

Keep: device discovery, HID report encoding, Apex 3 keyboard driver, zone
mapping, RGB effects, GameSense server, input remapper (evdev/uinput), key
definitions, error types, profile primitives where useful.

Remove: other keyboards (Apex 7, Apex Pro TKL 2023, M750), mice, headsets,
audio/Sonar, upstream CLI, web GUI (`desktop` feature), upstream daemon,
benchmarks, fuzzing, diagnostics binaries, release tooling. Drop the
dependencies only those used.

Our Apex 3 protocol fixes are made directly in this crate.

### `masterkeys-core`

- `Profile`: name, zone colors (10), brightness, effect + params, linked
  executables (for auto-switch), key remaps, macros.
- `Effect`: static, breathing, spectrum, wave, reactive, off — with speed/colour
  params. Animated effects are computed by the daemon (reactive uses the input task).
- `Macro`: ordered list of key down/up events with delays.
- `KeyId` + keyboard layout model: loaded from `assets/layouts/apex3-ansi.toml`
  (per key: id, label, x, y, width, height, zone).
- Profile storage: load/save/list/rename/delete under
  `~/.config/masterkeys/profiles/*.toml`; `settings.toml` holds the default
  profile and global options.
- D-Bus interface trait definitions (`zbus` proxy + names) shared by daemon
  and GUI.

### `masterkeysd` (daemon)

The only process that opens the keyboard. Runs as a systemd user service
(`assets/masterkeysd.service`).

Tasks (tokio):
- **Device task**: open Apex 3 via hidapi, apply profile, reconnect on
  unplug/replug (poll + retry with backoff).
- **Effect loop**: computes frames for animated effects and sends them at a
  fixed rate; idle when the effect is static/hardware-driven.
- **Process watcher**: scans `/proc` every ~2 s; when a linked executable
  starts, switch to its profile; when it exits, revert to the default profile.
  Manual selection overrides until the next game event.
- **Input task**: reads the keyboard's evdev nodes. Applies remaps and plays
  macros via a uinput virtual keyboard (grabbing the real device only when the
  active profile has remaps/macros). Broadcasts key press/release events over
  D-Bus **only while at least one client has subscribed**.
- **GameSense server**: the fork's GameSense-compatible HTTP server; game
  events override lighting while active.
- **D-Bus service** `org.masterkeys.Daemon` at `/org/masterkeys/Daemon`.

D-Bus interface (methods / signals):
- `GetState() -> state` (connected?, active profile, current lighting)
- `SetZoneColor(zone, rgb)`, `SetAllColor(rgb)`, `SetBrightness(0-100)`,
  `SetEffect(effect)`
- `ListProfiles()`, `ActivateProfile(name)`, `SaveProfile(profile)`,
  `DeleteProfile(name)`, `RenameProfile(old, new)`
- `SetRemap(key, target)`, `ClearRemap(key)`, `SetMacro(key, macro)`,
  `ClearMacro(key)`
- `StartMacroRecording()`, `StopMacroRecording() -> macro`
- `SubscribeKeys()`, `UnsubscribeKeys()`
- Signals: `StateChanged`, `ProfileChanged(name)`, `DeviceConnected(bool)`,
  `KeyEvent(key, pressed)`, `Error(message)`

Logs to journald via `tracing`.

### `masterkeys` (GUI + tray)

Slint, stock widgets. Talks to the daemon only through D-Bus.

Main window:
- **Keyboard graphic** (centrepiece): drawn from the layout file; each key is a
  clickable element tinted with its zone's current colour; keys light up live
  as they are physically pressed (via `KeyEvent`); keys with a remap or macro
  show a marker. Subscribes to key events only while the window is open.
- **Lighting panel**: zone colour pickers (clicking a key selects its zone),
  set-all colour, brightness slider, effect selector + speed/colour params.
- **Profiles panel**: list, activate, create, rename, delete; link/unlink game
  executables (pick from running processes or browse for a file).
- **Key panel** (opens when a key is clicked, or via "select by pressing" then
  pressing the physical key): remap to another key, assign/record macro, reset
  to default.
- **Status bar**: daemon offline / keyboard not found / OK, with a "start
  daemon" button (`systemctl --user start masterkeysd`).
- **Setup prompt**: if remapping is used and permissions are missing, show the
  one-time command to install the udev rule.

Tray (StatusNotifierItem via `ksni`): active profile, quick profile switch,
open window, quit GUI. Can run tray-only (window closed).

## Permissions & install

- `assets/70-masterkeys.rules`: udev rule granting the logged-in user access to
  the Apex 3 hidraw nodes, its evdev nodes, and `/dev/uinput`
  (`TAG+="uaccess"`). Installed to `/etc/udev/rules.d/` (writable on Bazzite).
- `assets/masterkeysd.service`: systemd user unit.
- `assets/masterkeys.desktop`: launcher entry.
- `scripts/install.sh`: copies binaries to `~/.local/bin`, installs the unit and
  desktop file, prints the one sudo command for the udev rule.
- Build happens inside a distrobox (Bazzite is immutable); Slint/hidapi build
  deps (`libudev-devel`, `fontconfig-devel`, etc.) documented in `README.md`.

## Error handling

- GUI never crashes on daemon errors; they appear in the status bar in plain
  words.
- Daemon survives keyboard unplug (reconnects), a malformed profile file
  (skipped, `Error` signal + log), and missing uinput permissions (remap
  disabled, reported).
- A second daemon instance refuses to start (D-Bus name already owned).

## Testing

- Unit tests: effect frame maths, profile TOML round-trip, layout file parse,
  macro timing model, process-watcher matching logic.
- Byte-level tests: Apex 3 HID report encoding for colour, per-zone and
  brightness commands match expected bytes.
- D-Bus: integration test running the service against a mock device.
- Hardware smoke script (`scripts/smoke.sh`) run on the real keyboard at the
  end of each milestone.

## Milestones

1. **Fork + lighting on hardware.** Vendor and trim the fork into the
   workspace; prove static colour, per-zone colour and brightness work on the
   real Apex 3; fix protocol code if not. (Highest risk — done first.)
2. **Daemon.** D-Bus service, effect loop, reconnect, systemd unit.
3. **GUI.** Keyboard graphic with live key presses and zone tint, lighting
   panel, status bar.
4. **Profiles + auto-switch.** Profile CRUD, executable linking, process
   watcher.
5. **Remap + macros.** Input task, uinput, click-a-key panel, macro recording,
   udev setup prompt.
6. **Tray + GameSense.** Tray icon, GameSense server wired in.

## Out of scope (v1)

Custom visual theme and PhotoCraft assets; focus-based profile switching (KWin
script); other SteelSeries devices; ISO layout; audio/Sonar; firmware updates;
onboard (on-keyboard) profile storage.

## Existing material

An earlier CLI draft (`src/device.rs`, `protocol.rs`, `effects.rs`, etc.) is
staged in the git index but absent from the working tree. It is superseded by
this design; it may be consulted for protocol notes during milestone 1, then
dropped.
