# Milestone 5: Key Remapping and Macros Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remap keys and play macros on the Apex 3, set up by clicking a key on the keyboard graphic.

**Architecture:**
- Each profile stores remaps and macros by key name. The active profile's set is compiled into the daemon's input engine.
- The engine is a pure state machine: the fork's `Remapper`, plus macro playback.
- While the active profile has any binding, the daemon grabs the Apex 3's letter-key evdev nodes and replays every event through a uinput virtual keyboard. Otherwise it reads keys without grabbing, as in M3, and only while something needs them.
- Macro recording happens in the daemon, timestamped from the same key stream.
- The GUI gets a Key panel, binding markers on the keyboard, "select by pressing", and a setup prompt when uinput isn't allowed.

**Tech Stack:** Rust 2024, the vendored `steelseries-gg::input` (`Remapper`, `BindingSet`, `Action`, `InputKey`), evdev 0.13 (`uinput::VirtualDevice`, `grab`), zbus 6, Slint 1.18.

**Spec:** `docs/superpowers/specs/2026-11-08-masterkeys-design.md` (milestone 5; Input task; Key panel; Setup prompt; Error handling).

## Global Constraints

- Only the Apex 3 (VID `0x1038`, PID `0x161a`) is read or grabbed.
- The daemon grabs the real device "only when the active profile has remaps/macros".
- Key events go over D-Bus "only while at least one client has subscribed". They carry **physical** key codes, not remapped ones.
- Missing uinput permission means "remap disabled, reported", and the daemon keeps running.
- Never send `00 09 00` (save to flash) to the keyboard. Never leave a key stuck down: every path that stops the virtual device first releases every key it pressed.
- The GUI builds inside the `rexbuild` distrobox. Every other crate builds on the host.
- Function first: stock Slint widgets only, no mockups.
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **A key held while the grab starts or stops** (for example, Enter that triggered the profile switch) → it isn't left stuck down or repeating. Expected: wait up to 5 s for all keys to come up before grabbing, and release every virtual key before ungrabbing. (Task 4 test on the plan/sequence logic; hardware check in Task 6.)
2. **Switching profile while a remapped key is held** → its release still releases what its press emitted. (Task 2 test.)
3. **The keyboard is unplugged while grabbed** → the virtual keys are released and the grab is retried after replug, with no stuck keys. (Task 4 test using a fake device stream; hardware check.)
4. **A macro key pressed again, or held, while its macro plays** → the macro plays once per press, auto-repeat never retriggers it, and other keys pass through during playback. (Task 2 test.)
5. **Escape chord** (both Ctrls and both Shifts held 2 s) → the grab is released, and the state reports remapping stopped until the bindings change or the daemon restarts. (Task 2 and Task 4 tests.)

---

### Task 1: Binding model in profiles

**Files:**
- Create: `crates/masterkeys-core/src/bindings.rs`
- Modify: `crates/masterkeys-core/src/profile.rs`, `crates/masterkeys-core/src/state.rs`, `crates/masterkeys-core/src/lib.rs`
- Test: inline `#[cfg(test)]` in `bindings.rs` and `profile.rs`

**Interfaces:**
- Produces:
  - In `bindings.rs`:
    - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub enum MacroStep { Down(String), Up(String), Delay(u32) }`, serialised as externally tagged lowercase: `{down = "a"}`, `{delay = 50}`. `Delay` is in milliseconds.
    - `#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)] pub struct Macro { pub steps: Vec<MacroStep> }`.
    - `pub const MAX_MACRO_STEPS: usize = 500; pub const MAX_DELAY_MS: u32 = 10_000;`
    - `pub fn parse_target(text: &str) -> Result<Action, String>`: a key name such as `"b"`, or keys joined with `+` such as `"left_ctrl+c"`. Surrounding spaces are trimmed. Case-insensitive via `InputKey::parse`. One key gives `Action::Key`; two to `MAX_COMBO_LEN` keys give `Action::Combo`.
    - `pub fn validate_macro(m: &Macro) -> Result<(), String>` checks:
      - 1 to `MAX_MACRO_STEPS` steps
      - every key name parses
      - delays are ≤ `MAX_DELAY_MS`
      - every `Down` has a later `Up`, and no `Up` comes without an earlier `Down`
    - `#[derive(Clone, Debug, Default)] pub struct Compiled { pub remaps: BindingSet, pub macros: HashMap<InputKey, Vec<ResolvedStep>> }`, where `pub enum ResolvedStep { Down(InputKey), Up(InputKey), Delay(Duration) }`, with `pub fn is_empty(&self) -> bool`.
    - `pub fn compile(remaps: &BTreeMap<String, String>, macros: &BTreeMap<String, Macro>) -> (Compiled, Vec<String>)`: every invalid entry is skipped, with one warning string naming its key.
  - `Profile` gains `pub remaps: BTreeMap<String, String>` (source key name → target text) and `pub macros: BTreeMap<String, Macro>` (source key name → macro).
    - `ProfileFile` stores them as `[remaps]` and `[macros.<key>]` tables, with `#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]`.
    - Source names are stored canonically, as `InputKey::name()` of the parsed key.
  - `#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)] #[serde(tag = "kind", content = "message")] pub enum InputStatus { #[default] Off, Reading, Remapping, Stopped(String), Unavailable(String) }` in `state.rs`.
  - `StateReport` gains `remaps`, `macros` (the active profile's) and `input: InputStatus`, all `#[serde(default)]`.

- [ ] **Step 1: Write the failing tests:**
  - `parse_target("b")` gives `Action::Key(InputKey::parse("b"))`.
  - `parse_target(" Left_Ctrl + c ")` gives `Combo([LEFT_CTRL, c])`.
  - `parse_target("")`, `parse_target("nope")` and `parse_target("a+b+c+d+e")` give `Err`.
  - `validate_macro` rejects: an empty macro; 501 steps; `Delay(10_001)`; `[Down a]` with no `Up`; `[Up a]` first.
  - `validate_macro` accepts `[Down a, Delay 30, Up a]`.
  - `compile` with remaps `{caps_lock: "esc", f13: "bogus"}` and macros `{f1: valid}` returns 1 remap and 1 macro, plus one warning containing `"f13"`.
  - A remap from a key to itself is skipped with a warning (`BindingSet` refuses it).
  - TOML round-trip of a `Profile` with one remap and one macro: save, load, equal.
  - A profile file with no `remaps`/`macros` loads with empty maps, and saving it writes no such tables.
  - `StateReport` JSON without `remaps`/`macros`/`input` still parses, with `input == Off`.
- [ ] **Step 2: Run** `cargo test -p masterkeys-core`. Expected: FAIL (the items don't exist).
- [ ] **Step 3: Implement** as specified. `compile` builds the `BindingSet` with `BindingSet::from_pairs`, one pair at a time, keeping pairs that pass on their own. On `Err`, warn and drop that pair.
- [ ] **Step 4: Run** `cargo test -p masterkeys-core`. Expected: PASS. Then run `cargo test -p masterkeysd`: it must still compile. Fix struct literals by adding `..Default::default()` or the empty maps.
- [ ] **Step 5: Commit** `"Add key remaps and macros to profiles"`.

### Task 2: Input engine and macro recorder (pure)

**Files:**
- Create: `crates/masterkeysd/src/engine.rs`
- Modify: `crates/masterkeysd/src/lib.rs`
- Test: inline in `engine.rs`

**Interfaces:**
- Consumes: `Compiled`, `ResolvedStep`, `Macro`, `MacroStep` (Task 1); the fork's `Remapper`, `RemapperConfig`, `KeyEvent`, `KeyValue`, `InputKey`.
- Produces:
  - `pub struct Engine`:
    - `Engine::new(compiled: &Compiled) -> Engine`, using `RemapperConfig::default()` (2 s escape chord).
    - `handle(&mut self, ev: KeyEvent, now: Instant) -> Vec<KeyEvent>`
    - `tick(&mut self, now: Instant) -> Vec<KeyEvent>`
    - `next_deadline(&self) -> Option<Instant>`: the earliest of the remapper's deadline and the next macro step.
    - `replace(&mut self, compiled: &Compiled)`: keys already held keep their old output. A playing macro finishes.
    - `release_all(&mut self) -> Vec<KeyEvent>`: aborts playback and releases every key that is down, from remaps or macros.
    - `is_active(&self) -> bool`: false after the escape chord.
  - Macro semantics:
    - A **press** of a macro key queues its steps starting at `now`. Its repeat and release are swallowed and never reach the remapper.
    - A press while that key's own macro is still playing is ignored.
    - Steps run in order. `Delay` pushes back later steps. Consecutive `Down`/`Up` steps with no delay come out in the same call.
    - Other keys go through the remapper as normal during playback.
  - `pub struct Recorder`:
    - `Recorder::start(now: Instant)`
    - `feed(&mut self, code: u16, pressed: bool, at: Instant)`
    - `finish(self) -> Macro`
  - Recorder rules:
    - Delays between events are rounded to whole ms and capped at `MAX_DELAY_MS`. There is no leading delay. Delays under 1 ms are dropped.
    - Codes with no `InputKey::name` are skipped. A release whose press wasn't recorded is skipped.
    - Recording stops taking events after `MAX_MACRO_STEPS` steps.
    - `finish` appends `Up` for keys still held.

- [ ] **Step 1: Write the failing tests (engine):**
  - `unbound_keys_pass_through`: press, repeat and release of `a` come out unchanged.
  - `remap_and_combo`: `caps_lock→esc` gives esc down/up. `f13→left_ctrl+c` gives ctrl down, c down, then on release both up.
  - `release_after_rebind_releases_original_output` (focus 2): press caps (esc down), `replace` with an empty set, release caps → esc up.
  - `macro_plays_with_delays`: `f1 → [Down a, Delay 50, Up a, Down b, Up b]`.
    - Press at t0 → `[a down]`.
    - `next_deadline == t0+50ms`.
    - `tick(t0+50ms)` → `[a up, b down, b up]`.
    - Release of f1 → `[]`.
  - `macro_key_repeat_and_repress_dont_retrigger` (focus 4): repeat events and a second press during playback add nothing.
  - `other_keys_pass_during_macro`: pressing `x` mid-delay → `[x down]`.
  - `release_all_mid_macro_releases_its_keys`: after `[a down]`, `release_all` contains `a up`, and later ticks emit nothing.
  - `escape_chord_stops_engine` (focus 5): hold both Ctrls and both Shifts, `tick` 2 s later → releases, then `is_active() == false`.
- [ ] **Step 2: Write the failing tests (recorder):**
  - Feed a down at t0, a up at t0+30ms, b down at t0+30ms, b up at t0+80ms → `[Down a, Delay 30, Up a, Down b, Delay 50, Up b]`.
  - A 20 s gap is capped at `Delay(10_000)`.
  - A held key gets an `Up` on `finish`.
  - An orphan release is skipped.
  - Feeding past 500 steps stops at 500.
  - The `Macro` from `finish` passes `validate_macro`.
- [ ] **Step 3: Run** `cargo test -p masterkeysd --lib engine`. Expected: FAIL.
- [ ] **Step 4: Implement** `Engine` and `Recorder`. Keep the macro queue as `VecDeque<(Instant, KeyEvent)>` with absolute due times. Track keys a macro pressed so `release_all` can release them.
- [ ] **Step 5: Run** the same tests. Expected: PASS. Then run `cargo clippy -p masterkeysd --all-targets` with no warnings.
- [ ] **Step 6: Commit** `"Add input engine with macro playback and recorder"`.

### Task 3: Bindings over D-Bus

**Files:**
- Modify: `crates/masterkeysd/src/profiles.rs`, `crates/masterkeysd/src/service.rs`, `crates/masterkeysd/src/keys.rs`, `crates/masterkeys-core/src/dbus.rs`, `crates/masterkeysd/src/main.rs`
- Test: `crates/masterkeysd/tests/dbus.rs`, plus `profiles.rs` unit tests

**Interfaces:**
- Consumes: Tasks 1 and 2.
- Produces:
  - `Profiles` methods. All act on the **active** profile, push `StoreOp::Save(active)`, and reject invalid input with `ProfileError::Invalid`. Source names are canonicalised.
    - `set_remap(&mut self, key: &str, target: &str)`: validates with `parse_target` and refuses `key == target`. Removes any macro on `key`.
    - `clear_remap(&mut self, key: &str)`
    - `set_macro(&mut self, key: &str, m: Macro)`: validates with `validate_macro`. Removes any remap on `key`.
    - `clear_macro(&mut self, key: &str)`
  - `DaemonCore` gains:
    - `pub bindings: Arc<watch::Sender<Compiled>>`: recompiled from the active profile on every bindings edit and every profile switch.
    - `pub input: Arc<watch::Sender<InputStatus>>`
    - `pub recorder: Arc<Mutex<Option<Recorder>>>`
  - `report_json` includes the active profile's remaps/macros and `input`.
  - D-Bus methods, with the proxy in `dbus.rs`. Each edit emits `StateChanged`.
    - `SetRemap(key: &str, target: &str)`
    - `ClearRemap(key: &str)`
    - `SetMacro(key: &str, macro_json: &str)`
    - `ClearMacro(key: &str)`
    - `StartMacroRecording()`: replaces any recording in progress. Turns the key reader on (`KeyHub` treats an active recording like a subscriber).
    - `StopMacroRecording() -> String`: the `Macro` as JSON. `InvalidArgument` if not recording or nothing was recorded.
    - Recording also stops by itself after 60 s. `StopMacroRecording` then returns what was captured.
  - `emit_keys` feeds each physical transition to the recorder if one is active, with `Instant::now()`, before emitting `KeyEvent`.
  - `KeyHub` wanted-state: the reader runs while `subscribers ∪ recording` is non-empty **or** `bindings` is non-empty. Expose `pub fn want(&self) -> watch::Receiver<bool>` unchanged in shape.

- [ ] **Step 1: Write the failing tests:**
  - `profiles.rs`:
    - `set_remap` then `set_macro` on the same key leaves only the macro, and vice versa.
    - `set_remap("a","a")`, `set_remap("nope","b")` and an invalid macro are all `Invalid`.
    - Edits push `Save(active)`.
  - `dbus.rs`:
    - `remap_round_trip`: `SetRemap("caps_lock","esc")` → `GetState` JSON has `remaps.caps_lock == "esc"`, and the bindings watch has 1 remap. `ClearRemap` → empty.
    - `macro_round_trip`: `SetMacro` with valid JSON → in the state. Invalid JSON → `InvalidArgument`.
    - `bindings_follow_profile_switch`: set a remap, create and activate profile B → the bindings watch is empty. Activate the first profile → 1 remap again.
    - `recording_captures_keys`: `StartMacroRecording`, push `(30,true)`, `(30,false)` through the harness key channel, `StopMacroRecording` → steps `[Down "a", …, Up "a"]`.
    - `stop_without_start_is_invalid`
    - `bindings_survive_restart`: set a remap, restart the harness on the same config dir → the remap is still there.
- [ ] **Step 2: Run** `cargo test -p masterkeysd`. Expected: FAIL.
- [ ] **Step 3: Implement.** The 60 s auto-stop is a spawned task that checks the recording's start time, so a newer recording isn't cut short.
- [ ] **Step 4: Run** `cargo test -p masterkeysd -p masterkeys-core`. Expected: PASS. Run clippy.
- [ ] **Step 5: Commit** `"Add remap, macro and recording methods to the daemon"`.

### Task 4: Grab and uinput input task

**Files:**
- Create: `crates/masterkeysd/src/input.rs`
- Modify: `crates/masterkeysd/src/keys.rs` (`run_reader` becomes the input task's read-only mode), `crates/masterkeysd/src/main.rs`, `crates/masterkeysd/Cargo.toml` (evdev is already present)
- Test: inline in `input.rs`

**Interfaces:**
- Consumes: `Engine` (Task 2); `DaemonCore.bindings` and `DaemonCore.input` (Task 3); `KeyHub` want (Task 3).
- Produces:
  - `pub enum Mode { Off, Read, Grab }` and `pub fn mode(want_read: bool, bindings_empty: bool, stopped: bool, uinput_ok: bool) -> Mode`:
    - `Grab` if bindings are non-empty, the engine hasn't been stopped by the chord, and uinput is ok.
    - Otherwise `Read` if `want_read` is true, or bindings are non-empty (keys still go to the GUI).
    - Otherwise `Off`.
  - `pub trait KeySink { fn emit(&mut self, events: &[KeyEvent]) -> io::Result<()>; }`. It's implemented by a wrapper over `evdev::uinput::VirtualDevice`, and by a `Vec` recorder in tests.
  - `pub async fn run_input(core: DaemonCore, hub: KeyHub, events: mpsc::Sender<(u16, bool)>)` replaces `run_reader` in `main.rs`.
    - It re-evaluates `mode` whenever `hub.want`, `core.bindings` or `core.connected` changes.
    - It sets `core.input` to `Off`, `Reading`, `Remapping`, `Stopped("remapping stopped by Ctrl+Ctrl+Shift+Shift; change a binding or switch profile to resume")` or `Unavailable("<error> — install assets/70-masterkeys.rules (see the GUI)")`.
    - A change to `core.bindings` clears `Stopped`.
  - **Grab sequence:**
    1. Open the virtual device ("MasterKeys virtual keyboard"). It declares every key code from 1 to 248 plus `KEY_*` codes in the fork's table. On failure → `Unavailable`, and fall back to `Read`.
    2. Wait up to 5 s, polling `get_key_state()` every 50 ms, until none of the target nodes reports a key down.
    3. `grab()` each letter-key node. On failure, ungrab the nodes already grabbed and report `Unavailable`.
    4. Sleep 300 ms so the desktop notices the device.
    5. Pump events: every `EV_KEY` goes to `engine.handle` → `sink.emit`, and its physical transition also goes to `events` for the GUI. `EV_LED` written to the virtual device is forwarded to the physical node (Caps/Num/Scroll lock lights). Deadlines run `engine.tick`.
    6. On leaving `Grab` (mode change, unplug, chord, error): `sink.emit(engine.release_all())`, then ungrab, then drop the device.
  - Nodes that aren't letter-key nodes (media/consumer) are only read, never grabbed.

- [ ] **Step 1: Write the failing tests:**
  - `mode` truth table:
    - (false, true, _, _) → Off
    - (true, true, _, _) → Read
    - (false, false, false, true) → Grab
    - (false, false, true, true) → Read
    - (false, false, false, false) → Read
  - `pump_releases_on_stream_end` (focus 3): drive the pump with a fake event stream (a channel) and a `Vec` sink. Send caps press with `caps→esc`, then close the stream. The sink ends with `esc up`, and the function returns `Ended::Unplugged`.
  - `pump_releases_on_mode_change` (focus 1): same, but flip a stop watch instead → `esc up` emitted before return.
  - `pump_reports_chord_stop` (focus 5): hold the chord past 2 s using a paused tokio clock (`start_paused`) → returns `Ended::Chord`, with releases emitted.
  - `held_keys_wait` is a pure helper `fn all_released(states: &[Vec<u16>]) -> bool`, tested on empty and non-empty sets.
- [ ] **Step 2: Run** `cargo test -p masterkeysd --lib input`. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - Split the I/O from the pump: `async fn pump(stream: impl Stream of (type, code, value), engine, sink, events, stop) -> Ended`, where `enum Ended { Unplugged, Stopped, Chord, Error(String) }`.
  - Real device code wraps evdev's `EventStream`.
  - Reuse `PressTracker` for the GUI transitions.
  - Keep the 1 s retry after `Unplugged`.
- [ ] **Step 4: Run** `cargo test --workspace --all-features` in rexbuild, plus clippy. Expected: PASS, no warnings.
- [ ] **Step 5: Commit** `"Grab the keyboard and replay through uinput when bindings are set"`.

### Task 5: GUI Key panel

**Files:**
- Modify: `crates/masterkeys/src/client.rs`, `crates/masterkeys/src/main.rs`, `crates/masterkeys/src/view.rs`, `crates/masterkeys/ui/main.slint`, `crates/masterkeys/ui/keyboard.slint`
- Test: `crates/masterkeys/tests/client.rs`, plus `view.rs` unit tests

**Interfaces:**
- Consumes: the Task 3 proxy methods; `StateReport.remaps`, `.macros` and `.input`; `InputKey::name`/`from_code`; the layout `KeyCap.code`.
- Produces:
  - Client:
    - Commands: `SetRemap(String, String)`, `ClearRemap(String)`, `SetMacro(String, Macro)`, `ClearMacro(String)`, `StartRecording`, `StopRecording(String /* key to assign */)`. `StopRecording` calls `StopMacroRecording`, then `SetMacro(key, …)`.
    - `Event::State` becomes `State(Box<StateReport>)`, replacing `(LightingState, String)`. Update the M4 call sites.
  - `view.rs`:
    - `pub fn binding_text(code: u16, remaps, macros) -> String` returns:
      - `"Default"`
      - `"Remapped to <target>"`
      - `"Macro: <n> steps, <secs:.1> s"`, where n counts only `Down`/`Up` steps and secs is the sum of delays
      - `"Not remappable"` when `InputKey::from_code(code).name()` is `None`
    - `pub fn marked(code: u16, remaps, macros) -> bool`
  - Keyboard: `KeyView` gains `marked: bool`, drawn as a small dot in the key's top-right corner.
  - The Key panel is a `GroupBox` titled "Key: <label>", shown when a key is selected. Clicking a key still selects its zone too. It contains:
    - the `binding_text`
    - a target `LineEdit` (placeholder `e.g. esc or left_ctrl+c`) with a **Remap** button
    - **Record macro**, which toggles to **Stop recording** while recording. The status text reads "Recording… press keys, then Stop".
    - **Reset key**, which sends `ClearRemap` or `ClearMacro`, whichever applies
    - **Select by pressing**: the next physical press (`Event::Key(code, true)`) selects that key, then the mode ends
  - Status bar: when `input` is `Unavailable(msg)`, show msg and a read-only `LineEdit` with `sudo cp assets/70-masterkeys.rules /etc/udev/rules.d/ && sudo udevadm control --reload && sudo udevadm trigger`, so it can be copied. When `input` is `Stopped(msg)`, show msg.

- [ ] **Step 1: Write the failing tests:**
  - client:
    - `SetRemap("caps_lock","esc")` → a `State` event whose `remaps` has it
    - `StartRecording`, keys 30 down/up through the harness, `StopRecording("f1")` → a `State` event with `macros.f1`
  - view: `binding_text` for each of the four cases; `marked` true for a remap or macro, false otherwise.
- [ ] **Step 2: Run** (rexbuild) `cargo test -p masterkeys`. Expected: FAIL.
- [ ] **Step 3: Implement** the client, view helpers, Slint UI and wiring.
- [ ] **Step 4: Run** `cargo test --workspace --all-features` and clippy in rexbuild. Expected: PASS, no warnings.
- [ ] **Step 5: Commit** `"Add Key panel for remaps and macros"`.

### Task 6: Hardware check

- [ ] **Step 1:** Run `scripts/install.sh`, `systemctl --user restart masterkeysd`, and relaunch the GUI.
- [ ] **Step 2: With the user:**
  - Remap Caps Lock → `esc`. Check in a text field that Caps types nothing and Esc behaves as Esc, and that other keys type normally. Check the Caps Lock light, and the volume wheel and media keys.
  - Record a macro on F1 (type `hi`). Pressing F1 in a text field types `hi`. Holding F1 types it once.
  - Hold Enter while switching to a profile with bindings. No stuck or repeating key.
  - Unplug and replug while remapped. Remapping resumes and nothing is stuck.
  - Escape chord: hold both Ctrls and both Shifts for 2 s. Keys go back to normal, and the status bar says remapping stopped.
  - **Reset key** on both keys. With no bindings left, the grab is released (`journalctl --user -u masterkeysd` shows the ungrab).
- [ ] **Step 3:** Commit any fixes, each with its failing test first.
