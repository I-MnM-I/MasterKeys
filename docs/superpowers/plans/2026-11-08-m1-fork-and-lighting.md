# Milestone 1: Fork + Lighting on Hardware — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Cargo workspace containing a vendored, Apex-3-only fork of steelseriesgg-rs whose small `apex3` API sets all-zone colour, per-zone colours and brightness on the real keyboard, proven by a smoke test.

**Architecture:** Prove the wire protocol with raw hidraw writes first (cheapest way to find out if upstream's untested Apex 3 frames are right). Then vendor upstream at a pinned commit as `crates/steelseries-gg`, trim it to Apex 3, and add a synchronous `apex3` module that later milestones (daemon) build on.

**Tech Stack:** Rust 1.99.1 (edition 2024), `hidapi`, upstream steelseriesgg-rs @ `84a015293606db70f39dc1e857b264648db3e489`.

**Spec:** `docs/superpowers/specs/2026-11-08-masterkeys-design.md` (milestone 1)

## Global Constraints

- Target device only: USB VID `0x1038`, PID `0x161A`, **interface 3** (`/dev/hidraw4` on this machine; never hard-code the node, find it by VID/PID/interface).
- Interface 3 report descriptor has **no report IDs**: every write is 33 bytes = `0x00` + 32-byte payload.
- Upstream frames (to be verified in Task 1): colours `00 0B 00 <R G B ×10>`; brightness `00 0A 00 <0..=100>`. **Never send `00 09 00`** (save_to_flash) in this milestone.
- Package name `steelseries-gg`, lib name `steelseries_gg`; workspace edition 2024, `rust-version = "1.99.1"`.
- Build deps (hidapi needs libudev): build in the `rexbuild` distrobox or on host if `libudev` headers exist. All project files stay inside `~/Documents/Apps-Games/MasterKeys/`.
- The user already has rw on the Apex 3 hidraw nodes via an ACL (uaccess); no udev install is required to run this milestone.

## Review Focus

1. **Keyboard unplugged / not found** → `Apex3Device::open()` returns `Error::DeviceNotFound`-style error naming VID:PID, never panics. (Task 4 test)
2. **Permission denied on hidraw** → error message says to install `assets/70-masterkeys.rules`. (Task 4 test on error mapping)
3. **Wrong interface** → only interface 3 is ever opened; interfaces 0–2 (typing, media) are never written. (Task 4 test on device selection)
4. **Brightness out of range (>100)** → rejected with an error, nothing sent. (Task 4 test)
5. **Unplug mid-session** → a write after unplug returns an error, not a panic, and `is_connected`-style state can be re-opened. (Task 5 manual smoke step)

---

### Task 1: Prove the protocol with raw hidraw writes

**Files:**
- Create: `scripts/probe_apex3.py`
- Create: `docs/apex3-protocol.md`

**Interfaces:**
- Produces: confirmed frame formats and the **zone index → physical area** table in `docs/apex3-protocol.md`, used by Task 4 and by milestone 3's layout file.

- [ ] **Step 1: Write `scripts/probe_apex3.py`** — stdlib only, run with `python3 -I`. Finds the hidraw node whose `/sys/class/hidraw/*/device/uevent` has `HID_ID=0003:00001038:0000161A` and `HID_PHYS` ending `input3`; opens it `wb`; subcommands:
  - `all RRGGBB` → writes `00 0B 00` + colour ×10, padded to 33 bytes
  - `zone N RRGGBB` → zone N (0–9) gets the colour, others black
  - `brightness N` → `00 0A 00 N`, padded to 33 bytes
  - `walk` → for N in 0..10: light zone N white, others black, print N, wait for Enter
- [ ] **Step 2: Run** `python3 -I scripts/probe_apex3.py all ff0000`. Expected: whole keyboard turns red. If nothing changes, try in order and record which works: (a) payload without the leading `0B 00` padding byte layout `00 0B <RGB×10>`, (b) sending GG's driver-mode frame `00 03 00 00 01` first, (c) a feature report (`HIDIOCSFEATURE`, 515 bytes). Stop and report to the user if none works.
- [ ] **Step 3: Run** `brightness 30`, then `brightness 100`. Expected: visible dim then bright.
- [ ] **Step 4: Run** `walk` with the user watching; record which physical area each zone index lights.
- [ ] **Step 5: Write `docs/apex3-protocol.md`** with the working frames (hex), the zone table, and anything that differed from upstream.
- [ ] **Step 6: Commit**

```bash
git add scripts/probe_apex3.py docs/apex3-protocol.md
git commit -m "Probe Apex 3 lighting protocol on hardware"
```

### Task 2: Workspace + vendored fork

**Files:**
- Create: `Cargo.toml` (workspace root), `crates/steelseries-gg/` (upstream copy), `crates/steelseries-gg/UPSTREAM.md`
- Modify: `.gitignore` (add `/target`)
- Remove from index: stale staged CLI draft (`src/*.rs`, root `Cargo.toml`/`Cargo.lock`, `assets/70-masterkeys.rules`) via `git rm --cached`

**Interfaces:**
- Produces: workspace member `crates/steelseries-gg`, package `steelseries-gg`, lib `steelseries_gg`.

- [ ] **Step 1:** Ensure toolchain ≥ 1.99.1 (`rustup update stable`; host currently has 1.99.0). Confirm `cargo --version`.
- [ ] **Step 2:** `git rm --cached -r src Cargo.toml Cargo.lock assets` (they are already absent from the working tree).
- [ ] **Step 3:** Copy upstream at commit `84a0152…` (clone into scratchpad, `git archive` the commit into `crates/steelseries-gg/`), excluding `.git`, `.github`, `.claude`, `.vscode`, `PKGBUILD`, `ssgg.install`, release-please and renovate files.
- [ ] **Step 4:** Rename the package to `steelseries-gg` in its `Cargo.toml`; root `Cargo.toml` is a workspace (`resolver = "3"`, `members = ["crates/*"]`, shared `edition = "2024"`, `rust-version = "1.99.1"`). Move its `rust-toolchain.toml`/`rustfmt.toml` to the root.
- [ ] **Step 5:** Write `UPSTREAM.md`: source URL, full commit hash, date vendored, MIT notice, and an empty "Local changes" list.
- [ ] **Step 6: Verify** `cargo test -p steelseries-gg` (default features). Expected: builds and upstream tests pass. Record any pre-existing failures in `UPSTREAM.md` rather than fixing them here.
- [ ] **Step 7: Commit** `"Vendor steelseriesgg-rs @84a0152 as crates/steelseries-gg"`.

### Task 3: Trim the fork to Apex 3

**Files:**
- Delete in `crates/steelseries-gg/src/`: `audio/`, `bin/`, `cli.rs`, `main.rs`, `gui.rs`, `daemon.rs`, `commands/`, `diagnostics_export.rs`, `performance.rs`, `pollrate.rs`, `devices/headsets/`, `devices/mice/`, `devices/fuzz.rs`, `devices/keyboards/{apex_7.rs,apex_7_profile.rs,apex_m750.rs,apex_pro_tkl_2023.rs,apex_pro_tkl_2023_profile.rs,onboard.rs}`; also `examples/`, `tools/`, `scripts/`, `PKGBUILD`-era docs not about Apex 3, and benchmark/fuzz tests under `tests/`.
- Modify: `lib.rs`, `devices/mod.rs`, `devices/keyboards/{mod.rs,models.rs,apex.rs}`, `devices/discovery.rs`, `devices/zone_mapping.rs`, `devices/hid_reports.rs`, `Cargo.toml` (drop features `audio`, `sonar`, `desktop` and their deps; keep `remap`).
- Keep: `config`, `device_state`, `devices` (Apex 3 paths), `error`, `fs_utils`, `gamesense`, `input`, `profiles`, `rgb`, `validation`.

**Interfaces:**
- Consumes: Task 2 crate.
- Produces: crate exposing only Apex 3 keyboard support; `devices::product_ids::APEX_3 == 0x161A` still public; `Apex3Command` still public.

- [ ] **Step 1:** Delete the listed files/modules and fix every resulting compile error by removing the code paths for other devices (not by stubbing them).
- [ ] **Step 2: Verify** `cargo build -p steelseries-gg --features remap` and `cargo test -p steelseries-gg --features remap` pass.
- [ ] **Step 3: Verify** `grep -rniE "apex.?7|apex.?pro|m750|nova|arctis|rival|aerox|sonar|1622" crates/steelseries-gg/src` returns nothing (TKL included — we support only `0x161A`).
- [ ] **Step 4: Verify** upstream tests `apex_3_zone_frame_fills_missing_zones_with_the_last_colour` and the `Apex3Command` serialization tests still exist and pass.
- [ ] **Step 5:** Append to `UPSTREAM.md` "Local changes": "Trimmed to Apex 3 (0x161A) only; removed audio/sonar/desktop/CLI/daemon".
- [ ] **Step 6: Commit** `"Trim steelseries-gg to Apex 3 only"`.

### Task 4: `apex3` synchronous API

**Files:**
- Create: `crates/steelseries-gg/src/apex3.rs`
- Modify: `crates/steelseries-gg/src/lib.rs` (`pub mod apex3;`), `crates/steelseries-gg/src/devices/hid_reports.rs` only if Task 1 found a different frame format
- Test: unit tests inside `apex3.rs`

**Interfaces:**
- Consumes: `crate::rgb::Color` (`r,g,b: u8`, consts `RED`, `BLUE`, `BLACK`), `crate::devices::hid_reports::Apex3Command`, `crate::Error`.
- Produces (used by milestone 2 daemon):
  - `pub const ZONE_COUNT: usize = 10;`
  - `pub const REPORT_LEN: usize = 33;`
  - `pub fn colors_report(colors: &[Color; ZONE_COUNT]) -> [u8; REPORT_LEN]`
  - `pub fn brightness_report(percent: u8) -> crate::Result<[u8; REPORT_LEN]>`
  - `pub fn is_apex3_control(vendor: u16, product: u16, interface: i32) -> bool`
  - `pub struct Apex3Device` with `pub fn open() -> crate::Result<Self>`, `pub fn set_zones(&mut self, colors: &[Color; ZONE_COUNT]) -> crate::Result<()>`, `pub fn set_all(&mut self, color: Color) -> crate::Result<()>`, `pub fn set_brightness(&mut self, percent: u8) -> crate::Result<()>`
  - Errors: device absent → `Error::DeviceNotFound(String)` containing `"1038:161a"`; hidraw permission denied → `Error::PermissionDenied(String)` mentioning `70-masterkeys.rules` (add these variants to `error.rs` if missing).

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn colors_report_layout() {
    let mut z = [Color::BLACK; ZONE_COUNT];
    z[0] = Color::RED; z[9] = Color::BLUE;
    let r = colors_report(&z);
    assert_eq!(r.len(), 33);
    assert_eq!(&r[..3], &[0x00, 0x0B, 0x00]);
    assert_eq!(&r[3..6], &[0xFF, 0x00, 0x00]);
    assert_eq!(&r[30..33], &[0x00, 0x00, 0xFF]);
}

#[test]
fn brightness_report_layout() {
    let r = brightness_report(55).unwrap();
    assert_eq!(&r[..4], &[0x00, 0x0A, 0x00, 55]);
    assert!(r[4..].iter().all(|&b| b == 0));
}

#[test]
fn brightness_over_100_rejected() {
    assert!(brightness_report(101).is_err());
}

#[test]
fn only_interface_3_of_apex3_selected() {
    assert!(is_apex3_control(0x1038, 0x161A, 3));
    for i in 0..3 { assert!(!is_apex3_control(0x1038, 0x161A, i)); }
    assert!(!is_apex3_control(0x1038, 0x1622, 3));
}

#[test]
fn no_report_starts_with_save_to_flash() {
    let r = colors_report(&[Color::BLACK; ZONE_COUNT]);
    assert_ne!(&r[..2], &[0x00, 0x09]);
}
```

(Adjust expected bytes to whatever Task 1 proved, and say so in `UPSTREAM.md` if they differ from upstream.)

- [ ] **Step 2: Run** `cargo test -p steelseries-gg apex3` → FAIL (module missing).
- [ ] **Step 3: Implement** `apex3.rs`: report builders delegate to `Apex3Command::serialize`; `open()` enumerates `hidapi::HidApi::device_list()`, picks the first entry passing `is_apex3_control`, opens it by path; maps `hidapi` open errors whose text contains "Permission denied" to `PermissionDenied`. `set_*` call `HidDevice::write(&report)`.
- [ ] **Step 4: Run** `cargo test -p steelseries-gg apex3` → PASS. Then full `cargo test -p steelseries-gg --features remap` → PASS.
- [ ] **Step 5:** Append to `UPSTREAM.md` "Local changes": "Added `apex3` sync API".
- [ ] **Step 6: Commit** `"Add apex3 sync API with report tests"`.

### Task 5: Smoke test on hardware + udev rule

**Files:**
- Create: `crates/steelseries-gg/examples/apex3_smoke.rs`, `scripts/smoke.sh`, `assets/70-masterkeys.rules`

**Interfaces:**
- Consumes: `steelseries_gg::apex3::{Apex3Device, ZONE_COUNT}`, `Color`.
- Produces: `scripts/smoke.sh`, reused and extended by later milestones.

- [ ] **Step 1:** `apex3_smoke.rs` — args `all RRGGBB` | `zones RRGGBB×10` | `brightness N`; prints the error and exits 1 on failure (no panics).
- [ ] **Step 2:** `scripts/smoke.sh` — builds the example, then runs: all red → all green → zones rainbow (10 hues) → brightness 20 → brightness 100 → all white; after each, prompts `ok? [y/n]` and exits non-zero on `n`.
- [ ] **Step 3:** `assets/70-masterkeys.rules`: `TAG+="uaccess"` for `hidraw` with `ATTRS{idVendor}=="1038", ATTRS{idProduct}=="161a"`, for `input` event nodes of the same device, and for `KERNEL=="uinput"`. Header comment with the install command (`sudo cp … /etc/udev/rules.d/ && sudo udevadm control --reload && sudo udevadm trigger`). Not installed in this milestone.
- [ ] **Step 4: Run** `scripts/smoke.sh` with the user watching → every step answered `y`.
- [ ] **Step 5: Review-focus checks (manual):** unplug keyboard, run `apex3_smoke all ff0000` → prints a not-found error naming `1038:161a`, exit 1; replug and rerun → works.
- [ ] **Step 6: Commit** `"Add Apex 3 smoke test and udev rule"`.

---

## Later milestones

Milestones 2–6 of the spec each get their own plan, written once the previous milestone is done (they depend on what this one proves).
