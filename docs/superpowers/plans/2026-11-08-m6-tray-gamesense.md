# Milestone 6: Tray and GameSense Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:**
- Games that speak SteelSeries GameSense light the Apex 3 through the daemon.
- The GUI lives in the system tray, with quick profile switching, and can run with its window closed.

**Architecture:**
- **GameSense server in the daemon.** The daemon runs the fork's GameSense HTTP server on `127.0.0.1`. Its output reaches the daemon through a channel: zone colours, heartbeats, and removed games.
- **Overlay.** A per-zone overlay lays game colours over the profile's lighting in the output loop. Each zone falls back to the profile when its game goes quiet (15 s) or is removed.
- **Game discovery.** Games find the server through `coreProps.json`. The daemon writes it to the fork's `/tmp` path and into every Steam Proton prefix, so Windows games under Proton find it.
- **Tray.** The GUI gets a `ksni` StatusNotifierItem tray, closing the window hides it, and a `--tray` flag starts hidden.
- **One GUI at a time.** A second launch shows the running window, over a small `org.masterkeys.Gui` D-Bus name.

**Tech Stack:** Rust 2024, the vendored `steelseries-gg::gamesense` (axum 0.8), zbus 6, Slint 1.18, `ksni` 0.3.

**Spec:** `docs/superpowers/specs/2026-11-08-masterkeys-design.md` (milestone 6; GameSense server; Tray).

## Global Constraints

- **Spec, verbatim:**
  - "game events override lighting while active"
  - tray shows "active profile, quick profile switch, open window, quit GUI"
  - "Can run tray-only (window closed)"
  - key events are subscribed "only while the window is open"
- The GameSense server binds `127.0.0.1` only, never `0.0.0.0`.
  - It tries the fixed port **51340** first, so `coreProps.json` stays valid across restarts. If that port is taken, it uses an ephemeral port.
- Never send `00 09 00` (save to flash).
- GameSense never changes or saves the profile. It is an overlay only, and `GetState` keeps reporting the profile's lighting.
- The GUI builds inside the `rexbuild` distrobox. Every other crate builds on the host.
- Function first: stock widgets only, and the theme icon `input-keyboard` for the tray.
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **A game stops sending events, or crashes without `remove_game`.** Expected: each of its zones falls back to the profile 15 s after its last event or heartbeat, and nothing stays coloured forever. (Task 2 test.)
2. **Two games, or two handlers, colour overlapping zones.** Expected: the latest event wins per zone. Removing one game clears only its zones. (Task 2 test.)
3. **A malformed or unknown handler in `bind_game_event`** (an unknown `device-type`, a bad colour). Expected: that binding is refused with HTTP 400 and the server keeps working. It never panics. (Task 1 test.)
4. **A Steam library on a second drive, or a prefix with no `drive_c`.** Expected: every library in `libraryfolders.vdf` is covered, and prefixes without `pfx/drive_c` are skipped without error. (Task 3 test.)
5. **GUI started twice, or the tray Quit pressed while the daemon is down.** Expected: the second launch only raises the first window and exits. Quit always ends the GUI process and leaves the daemon running. (Task 4 manual check; Task 6.)

---

### Task 1: GameSense server output channel and zoned devices (fork)

**Files:**
- Modify: `crates/steelseries-gg/src/gamesense/mod.rs`, `crates/steelseries-gg/src/gamesense/server.rs`, `crates/steelseries-gg/UPSTREAM.md`
- Test: the inline tests in `server.rs`, which already serve the router on an ephemeral port

**Interfaces:**
- Produces:
  - `#[derive(Clone, Debug, PartialEq)] pub enum GameSenseOutput`, with three variants:
    - `Color { game: String, zone: String, color: crate::rgb::Color }`
    - `Heartbeat { game: String }`, sent for `game_heartbeat` and for every `game_event`
    - `GameRemoved { game: String }`
  - `GameSenseServer::set_output(&self, tx: tokio::sync::mpsc::UnboundedSender<GameSenseOutput>)`. It **replaces** `set_rgb_callback`.
  - `pub fn router(&self) -> axum::Router` (was private)
  - `pub async fn serve(&self, listener: tokio::net::TcpListener) -> Result<()>`. It serves without writing `coreProps.json`. `run()` keeps writing it and calls `serve`.
  - `Handler::RgbZonedDevice { zone: String, color: ColorHandler, mode: Option<String> }`, serialised as `"device-type": "rgb-zoned-device"`. It is processed like `Keyboard`.

- [ ] **Step 1: Write the failing tests:**
  - `bind` + `game_event` with a `keyboard`/`function-keys` static red handler → the receiver gets `Heartbeat{game}` then `Color{game, zone: "function-keys", color: RED}`.
  - A binding with `"device-type": "rgb-zoned-device", "zone": "three"` and a gradient → the receiver gets `Color{zone: "three", …}` at value 50 with the midpoint colour.
  - `game_heartbeat` → `Heartbeat`. `remove_game` → `GameRemoved`.
  - `bind_game_event` with an unknown `device-type` → HTTP 4xx. A later valid event still works.
- [ ] **Step 2: Run** `cargo test -p steelseries-gg gamesense`. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - Keep `rgb_callback` out: one output path.
  - `send` on a closed channel is ignored.
  - Record the change in `UPSTREAM.md`.
- [ ] **Step 4: Run** `cargo test -p steelseries-gg`. Expected: PASS. `cargo clippy -p steelseries-gg --all-targets` gives no warnings.
- [ ] **Step 5: Commit** `"GameSense server: output channel, zoned devices, serve without coreProps"`.

### Task 2: Overlay model and zone mapping

**Files:**
- Create: `crates/masterkeysd/src/gamesense.rs`, `crates/masterkeys-core/src/gszones.rs`
- Modify: `crates/masterkeysd/src/output.rs`, `crates/masterkeysd/src/lib.rs`, `crates/masterkeys-core/src/lib.rs`
- Test: inline tests, plus `output.rs` tests (fake `Lighting`)

**Interfaces:**
- Consumes: `GameSenseOutput` (Task 1); `Layout` (`apex3_ansi()`, `KeyCap.code`, `.zone`).
- Produces:
  - `pub fn zones_for(name: &str, layout: &Layout) -> Vec<usize>` in core, sorted and deduplicated. It returns:
    - `"one"`..`"ten"` → `[0]`..`[9]`
    - `"all"`, `"main-keyboard"` → all 10
    - `"function-keys"` → zones of F1–F12 (codes 59–68, 87, 88)
    - `"number-keys"` → zones of the 1–0 row (codes 2–11)
    - `"arrow-keys"` → codes 103, 105, 106, 108
    - `"nav-cluster"` → codes 102, 104, 107, 109, 110, 111
    - `"keypad"` → codes 55, 71–83, 96, 98
    - `"logo"`, `"wheel"`, unknown names → `[]`
  - `pub type OverlayFrame = [Option<Color>; ZONE_COUNT]`.
  - `pub struct Overlay`:
    - `apply(&mut self, out: GameSenseOutput, now: Instant) -> bool`. True if the shown overlay changed. For `Color`, the zones from `zones_for` get the colour, owned by that game, stamped `now`. `Heartbeat` refreshes that game's zones' stamps. `GameRemoved` clears that game's zones.
    - `expire(&mut self, now: Instant) -> bool` clears zones older than `GAME_TIMEOUT = 15 s`.
    - `frame(&self) -> OverlayFrame`
    - `active_game(&self) -> Option<String>`: the game owning the most recently set zone.
    - `next_expiry(&self) -> Option<Instant>`
    - `clear(&mut self) -> bool`
  - `Shared` (output loop) gains `overlay: watch::Receiver<OverlayFrame>`.
    - `drive` lays each `Some` zone over the rendered frame before `set_zones`, and also re-sends when `overlay` changes.
    - `shared.frame` keeps the **profile** frame (without the overlay), so `SetZoneColor` is unaffected.

- [ ] **Step 1: Write the failing tests:**
  - `zones_for`: `"one"`→`[0]`, `"ten"`→`[9]`, `"all"`→`0..10`, `"keypad"` ⊆ `{8, 9}` and not empty, `"arrow-keys"` not empty, `"logo"`→`[]`, `"bogus"`→`[]`.
  - Overlay:
    - `color_then_expire` (focus 1): set `"all"` red at t0, `expire(t0+14s)` → unchanged, `expire(t0+15s)` → all `None`, returns true.
    - `heartbeat_extends`: a heartbeat at t0+10s keeps the zones at t0+20s.
    - `latest_wins_and_remove_clears_own_zones` (focus 2): game A sets `"all"` red, then B sets `"one"` blue. Zone 0 is blue, the rest red, and `active_game` is `Some("B")`. Remove A → only zone 0 is still set.
    - `unknown_zone_changes_nothing` → returns false.
  - Output: `overlay_is_laid_over_frame`. Static white state, overlay zone 2 = red → the fake device's last `set_zones` has zone 2 red and the others white. `shared.frame` stays all white. Changing the overlay back to all `None` sends all white.
- [ ] **Step 2: Run** `cargo test -p masterkeys-core gszones` and `cargo test -p masterkeysd --lib`. Expected: FAIL.
- [ ] **Step 3: Implement.** Update every `Shared { … }` construction, in `main.rs` and the tests, with a channel defaulting to all `None`.
- [ ] **Step 4: Run** `cargo test -p masterkeysd -p masterkeys-core`. Expected: PASS, and clippy is clean.
- [ ] **Step 5: Commit** `"Add GameSense overlay and zone mapping"`.

### Task 3: GameSense in the daemon (server, coreProps, D-Bus)

**Files:**
- Create: `crates/masterkeysd/src/coreprops.rs`
- Modify: `crates/masterkeysd/src/gamesense.rs`, `crates/masterkeysd/src/main.rs`, `crates/masterkeysd/src/service.rs`, `crates/masterkeys-core/src/state.rs`, `crates/masterkeys-core/src/profile.rs` (`Settings`), `crates/masterkeys-core/src/dbus.rs`, `crates/masterkeysd/src/profiles.rs`
- Test: inline tests, plus `crates/masterkeysd/tests/dbus.rs`

**Interfaces:**
- Consumes: Tasks 1–2.
- Produces:
  - `coreprops.rs`:
    - `pub fn steam_libraries(home: &Path) -> Vec<PathBuf>` returns `<home>/.local/share/Steam` and `<home>/.steam/steam` (each only if it exists, deduplicated by canonical path), plus every `"path"` value in each one's `steamapps/libraryfolders.vdf`. Parse with a simple line match on `"path"\s+"(.*)"` and unescape `\\` to `\`.
    - `pub fn targets(home: &Path) -> Vec<PathBuf>` returns `/tmp/steelseries-engine/coreProps.json` plus, for each library and each `steamapps/compatdata/*/pfx/drive_c` that exists, `…/drive_c/ProgramData/SteelSeries/SteelSeries Engine 3/coreProps.json`.
    - `pub fn write_all(home: &Path, port: u16) -> Vec<String>` writes `{"address":"127.0.0.1:<port>"}` to each target. It creates missing parent folders: `0700` for the `/tmp` one, normal mode inside prefixes. It refuses a symlinked `/tmp/steelseries-engine`, the same check as the fork's `write_secure_json`. It returns warnings and skips existing files that are already identical.
  - `gamesense.rs`:
    - `pub async fn run_gamesense(core: DaemonCore, conn: zbus::Connection, overlay_tx: watch::Sender<OverlayFrame>)`. It binds `127.0.0.1:51340`, else `127.0.0.1:0`, logs the port, and serves.
    - `pub async fn follow(core: DaemonCore, conn: zbus::Connection, rx: mpsc::UnboundedReceiver<GameSenseOutput>, overlay_tx: watch::Sender<OverlayFrame>)` is the loop `run_gamesense` runs after binding (tests drive it directly). It feeds `Overlay` from the output channel while `core` reports GameSense enabled, and calls `expire` at `next_expiry`.
    - It publishes `overlay.frame()` and sets `core.gamesense`, announcing `StateChanged` when the active game changes.
    - Every 30 s, and at start, it calls `write_all(home, port)`.
  - `Settings` gains `#[serde(default)] gamesense_disabled: bool`. `Profiles` gains `gamesense_enabled()` and `set_gamesense_enabled(bool)`, which pushes `StoreOp::Settings`.
  - `StateReport` gains `#[serde(default)] gamesense: GameSenseStatus { enabled: bool, game: Option<String>, port: u16 }`. `DaemonCore` gains `gamesense: Arc<watch::Sender<GameSenseStatus>>`.
  - D-Bus: `SetGameSenseEnabled(enabled: bool)`. Turning it off clears the overlay immediately. The setting is persisted and emits `StateChanged`.

- [ ] **Step 1: Write the failing tests:**
  - `coreprops`, with a temp dir as home:
    - `libraryfolders.vdf` listing `<tmp>/lib2` → `steam_libraries` contains both. Compatdata `100/pfx/drive_c` in lib2 and `200/` without `pfx` → `targets` has the `/tmp` path plus exactly one prefix path (focus 4).
    - `write_all` to a temp target writes valid JSON with the port. A second call doesn't rewrite it (mtime unchanged).
  - `dbus`:
    - `gamesense_toggle_persists`: `SetGameSenseEnabled(false)` → `GetState.gamesense.enabled == false`. Restart on the same config → still false.
    - `gamesense_reports_active_game`: spawn `follow(core, conn, rx, overlay_tx)` with a test channel. Send `Color{game:"G", zone:"all"}` → `GetState.gamesense.game == Some("G")`, and the overlay watch has 10 `Some`. With GameSense disabled, the same send leaves the overlay empty.
- [ ] **Step 2: Run** `cargo test -p masterkeysd`. Expected: FAIL.
- [ ] **Step 3: Implement.** In `main.rs`, spawn `run_gamesense` with the overlay sender, and pass the receiver into `Shared`. `HOME` comes from the environment; if it's missing, write only `/tmp`.
- [ ] **Step 4: Run** `cargo test -p masterkeysd -p masterkeys-core` and clippy. Expected: PASS.
- [ ] **Step 5: Commit** `"Serve GameSense from the daemon and publish coreProps for Proton games"`.

### Task 4: GUI tray, hide-to-tray, single instance

**Files:**
- Create: `crates/masterkeys/src/tray.rs`, `crates/masterkeys/src/instance.rs`
- Modify: `crates/masterkeys/Cargo.toml` (add `ksni = "0.3"`; pick the async runtime feature that matches tokio, per its docs), `crates/masterkeys/src/client.rs`, `crates/masterkeys/src/main.rs`, `crates/masterkeys/src/lib.rs`, `crates/masterkeys/ui/main.slint`
- Test: `crates/masterkeys/tests/client.rs`, plus `tray.rs` unit tests

**Interfaces:**
- Produces:
  - Client:
    - `Command::KeysWanted(bool)`. The client subscribes and unsubscribes to match, and re-applies it after reconnecting. The default stays `true`, because the window starts open unless `--tray`.
    - `Command::SetGameSense(bool)` calls `SetGameSenseEnabled`.
  - `tray.rs`:
    - `pub struct TrayModel { pub profile: String, pub profiles: Vec<String>, pub connected: bool }`
    - `pub fn menu_items(m: &TrayModel) -> Vec<TrayItem>`, where `pub enum TrayItem { Header(String), Profile { name: String, active: bool }, Separator, Open, Quit }`. The order is: header `"MasterKeys — <profile>"` (or `"MasterKeys — daemon not running"` when not connected), the profiles, a separator, Open, Quit.
    - `pub fn spawn_tray(…)` returns a handle with `update(TrayModel)`.
    - The `ksni::Tray` impl maps `TrayItem`s to ksni menu items. Profile items are radio-style or checkmark items, and clicking one sends `Command::ActivateProfile`. Open, and a left click on the icon, raise the window. Quit ends the event loop.
    - Icon name: `input-keyboard`. Title: "MasterKeys".
  - `instance.rs`:
    - `pub fn claim_or_raise() -> Option<GuiInstance>`, using `zbus::blocking`. It requests `org.masterkeys.Gui` with `DO_NOT_QUEUE`.
    - If the name is taken, it calls `org.masterkeys.Gui1.Show()` on the owner and returns `None`, and `main` exits 0.
    - Otherwise it serves `Show()`, which calls a registered `Box<dyn Fn() + Send + Sync>` that does `invoke_from_event_loop(show window)`.
    - If the session bus is missing, it returns `Some` with no name, so the GUI still runs.
  - `main.rs`:
    - `--tray` starts with the window hidden.
    - Closing the window hides it (`CloseRequestResponse::HideWindow`) and sends `KeysWanted(false)`. Showing it sends `KeysWanted(true)`.
    - The event loop is `slint::run_event_loop_until_quit()`. Tray Quit calls `slint::quit_event_loop()`.
    - The 33 ms UI timer keeps running while hidden, so tray state stays current.
  - Lighting panel: a `CheckBox` "GameSense lighting" bound to `state.gamesense.enabled`. The status bar shows `"GameSense: <game>"` while a game is active.

- [ ] **Step 1: Write the failing tests:**
  - `tray.rs`:
    - `menu_items` for `{profile: "B", profiles: ["A", "B"], connected: true}` → `[Header("MasterKeys — B"), Profile{A, false}, Profile{B, true}, Separator, Open, Quit]`.
    - Not connected → the header says "daemon not running" and there are no profile items.
  - client:
    - `keys_wanted_false_stops_key_events`: after `KeysWanted(false)`, a key sent through the harness doesn't arrive within 300 ms. After `KeysWanted(true)`, it does.
    - `gamesense_toggle_reaches_daemon` → `State` with `gamesense.enabled == false`.
- [ ] **Step 2: Run** (rexbuild) `cargo test -p masterkeys`. Expected: FAIL.
- [ ] **Step 3: Implement** in this order: the client, `tray.rs` (model plus ksni), `instance.rs`, then the `main.rs` lifecycle and UI.
- [ ] **Step 4: Run** (rexbuild) `cargo test --workspace --all-features` and clippy. Expected: PASS. Then a manual smoke test:
  - `masterkeys --tray` shows a tray icon and no window.
  - A second `masterkeys` raises the window, and the second process exits.
- [ ] **Step 5: Commit** `"Add tray icon, hide-to-tray and single GUI instance"`.

### Task 5: Install and autostart

**Files:**
- Create: `assets/masterkeys-tray.desktop`
- Modify: `scripts/install.sh`, `README.md` (the GameSense and tray notes)

**Interfaces:**
- `assets/masterkeys-tray.desktop`: `Exec=masterkeys --tray`, `X-GNOME-Autostart-enabled=true`, `NoDisplay=true`.
- `install.sh` copies it to `~/.config/autostart/` (the GUI tray starts at login) and prints how to turn that off: delete the file.
- README states:
  - Games under Steam Proton are covered automatically. Other launchers (Heroic, Lutris) need a copy of `coreProps.json` in their prefix, and the README gives the path.

- [ ] **Step 1:** Write the files. Run `bash -n scripts/install.sh`. Expected: no output.
- [ ] **Step 2:** Run `scripts/install.sh`. Expected: `~/.config/autostart/masterkeys-tray.desktop` exists.
- [ ] **Step 3: Commit** `"Autostart the tray at login"`.

### Task 6: Hardware check

- [ ] **Step 1:** Restart the daemon, quit any GUI, and run `masterkeys --tray`.
- [ ] **Step 2: GameSense, simulated with `curl` against the port in `coreProps.json`:**
  - Register game `TEST`.
  - Bind `health` to `rgb-zoned-device` zone `one` (gradient red→green), and `ammo` to `keyboard`/`function-keys` (static blue).
  - Send `health=10`, then `health=90`. Zone 0 changes colour on the keyboard, and the rest keep the profile.
  - Stop sending. After about 15 s the profile comes back.
  - `remove_game` clears it at once.
  - Turn off "GameSense lighting" and resend → nothing changes.
  - Check that a Proton prefix contains `coreProps.json`.
- [ ] **Step 3: Tray, with the user:**
  - The icon appears in the KDE tray, and its menu shows the active profile.
  - Switching a profile from the tray changes the lighting.
  - Open shows the window. Closing the window leaves the tray.
  - Launching MasterKeys from the app menu raises the existing window.
  - Quit ends the GUI, and the daemon keeps running.
- [ ] **Step 4:** If the user has a GameSense-capable game installed, launch it and look for reactive lighting. This is optional.
- [ ] **Step 5:** Commit any fixes, each with its failing test first.
