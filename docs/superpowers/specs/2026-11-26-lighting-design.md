# Per-LED lighting, Apex 3 on Windows, Redragon K745 PRO — design

## Purpose

Lighting today is built around the Apex 3's 10 zones, and the Windows build hides the lighting panel. This phase makes lighting device-neutral, so every supported device gets the same effects drawn on its own LEDs. It also adds the friend's Redragon K745 PRO over the USB cable. The Apex 3 becomes the first driver of the new layer, on Linux and on Windows.

## Success criteria

- On Windows, the lighting panel appears when a supported device is connected and is hidden when none is.
- The friend's K745 PRO, connected by cable, shows every MasterKeys effect, and he can colour single keys.
- The user's Apex 3 behaves as before on Linux, and also works on Windows.
- Profiles saved by 0.2.0 load unchanged, and their custom zone colours still show on the Apex 3.
- Devices can be plugged in and unplugged while MasterKeys runs.
- All existing tests pass, and Linux behaviour is unchanged apart from the new device picker.

## Constraints

- Never send `00 09 00` to an Apex 3 (unchanged).
- The K745 driver sends only the volatile lighting frame (feature report `0x06`, command `0x06`). It never sends save, flash, remap or firmware commands. OpenRGB disabled its SinoWealth keyboard driver after it bricked some Redragon boards that share USB IDs, so this rule is absolute.
- The K745 driver sends at most one frame every 35 ms.
- No code is copied from GPL projects. Protocol facts come from public descriptions and from Redragon's own installer files, read without running them.
- Same codebase and platform seams as the Windows phase.

## Device facts (from Redragon's K745 PRO installer, `Redragon_K745-RGB-PRO_Setup_v2.0_20250523.exe`)

- `Dev/kb/KB.ini`:
  - wired `VID=0x258a PID=0x010c`, wireless receiver `0x3554:0xfa09`
  - `Name=Redragon K745-RGB-PRO`
  - firmware `Fw=24`
- Each `[KEY]` line, e.g. `K1=20,23,43,48, 0x02,0x1B,0x00,0`, has these fields:
  - the key's rectangle in the vendor's key picture (left, top, right, bottom)
  - the type
  - the Windows virtual-key code
  - a second code
  - the LED index
- The LED indices run from 0 to 124 and fit the 126-LED frame below.
- Wired lighting frame. This is published for the MCHOSE G75, which uses the same `258a:010c` chip; Redragon's tool also uses `HidD_SetFeature`.
  - Transport: a 520-byte HID feature report, report ID `0x06`, on interface 1, usage page `0xFF00`. On Windows it is the `Col06` collection.
  - Byte layout:
    - `[0]=0x06`
    - `[1]=0x06`
    - `[2..4]=0`
    - `[4]=0x01` (direct mode)
    - `[5]=0`
    - `[6]=0x80`
    - `[7]=0x01`
    - `[8..134]` = red for LEDs 0–125
    - `[134..260]` = green
    - `[260..386]` = blue
    - the rest is zero
  - There is no checksum, no handshake and no commit.
  - The G75's onboard animation must be stopped with Fn + \\. Whether the K745 needs this is unknown.

## Architecture

### Device model (`masterkeys-core::devices`)

- `struct Led { key: Option<u16> /* evdev code */, x: f32, y: f32 /* 0.0–1.0 across the device */ }`
- `struct DeviceInfo { id: String /* stable, e.g. "apex3", "k745pro" */, name: String, kind: DeviceKind /* Keyboard | Mouse */, leds: Vec<Led> }`
- Daemon-side trait `LedDevice`:
  - `info() -> &DeviceInfo`
  - `write(&mut self, colors: &[Color], brightness: u8) -> Result<()>`
  - `colors.len() == info().leds.len()`
- A driver registry lists the supported models. Each entry has a matcher and an opener: VID/PID, interface, usage page and, where needed, the feature report length.

### Drivers

- **Apex 3:**
  - It wraps the existing `Apex3Device`: 10 LEDs at the centres of its 10 zones, with `key: None`.
  - Brightness and zone writes are unchanged.
  - It opens on Windows through hidapi's windows-native backend, picking the usage page `0xFFC0` collection.
- **K745 PRO, wired:**
  - Its 126 LEDs are a const table generated once from `KB.ini`. Each LED's position is the centre of its key rectangle, normalised over the whole board. Its key is the evdev code converted from the virtual-key code.
  - The generator script and the extracted `KB.ini` facts are committed under `tools/`, so the table can be checked again.
  - The driver refuses to open unless the device's descriptor has a 520-byte feature report with ID `0x06` on interface 1, usage page `0xFF00`.
  - Brightness scales the colours, since the frame has no brightness byte.

### Rendering

- `Renderer` takes a `&DeviceInfo` and returns one colour per LED.
- The effect engine computes each effect from LED positions:
  - wave and gradient run along x
  - spectrum, breathing and static are uniform
  - custom colours come from the per-key map, with LEDs that have no colour shown black
- The Apex 3 gets the same result as today, because its zone centres run left to right.

### Profiles

- `LightingState` keeps `effect` and `brightness`.
- `Custom` colours become a map from device id to a list of LED colours.
- Loading a 0.2.0 profile whose `Custom { colors }` has 10 entries puts them under `"apex3"`.
- Saving writes the new form, and old builds ignore what they don't know. Old builds will lose K745 colours, which is acceptable.

### Runtime

- The output task scans for supported devices every 3 s.
- It keeps one driver per connected device and renders the shared effect for each one.
- If a write fails, it drops that device and finds it again on the next scan.
- The `device: bool` switch from the Windows phase becomes "drive lighting devices", and the embedded core on Windows now passes true.
- Tests keep a fake registry so no test touches real hardware.

### D-Bus additions

- `devices() -> String`: JSON list of `DeviceInfo` for connected devices.
- `set_led_color(device: &str, led: u16, color: &str)`.
- `set_zone_color` stays for the Apex 3.
- `StateReport.connected` becomes true when any device is connected, and the report lists the connected device ids.

### GUI

- The lighting panel shows when at least one device is connected. This replaces the fixed `lighting-available=false` on Windows.
- A device picker is shown when more than one device is connected.
- The keyboard picture shows the selected device: the existing Apex 3 layout, or the K745 layout drawn from its key rectangles. Clicking a key sets its custom colour.
- A "Test: flash red" button shows solid red on the selected device for 2 s, then goes back to the profile's effect. It uses the normal frame path.

## Testing

- Unit tests, all runnable on Linux:
  - the K745 table has 126 LED slots, unique indices and every `KB.ini` key, and Esc is LED 0
  - the K745 frame bytes for a known colour set, compared byte for byte
  - the descriptor check rejects the wrong report length, ID or interface
  - wave and gradient on positions, and the Apex 3 output is unchanged from 0.2.0 for every effect
  - 0.2.0 profiles migrate their custom colours
  - the hotplug scan adds and drops fake devices
- Windows CI builds and tests the Windows side.
- Hardware checklist added to `README-windows.txt`:
  - the "Test: flash red" button turns the K745 red
  - whether Fn + \\ is needed first
  - what the keyboard shows after Quit
  - a per-key colour change
  - unplugging and replugging

## Out of scope

- K745 2.4 GHz and Bluetooth (a different packet format).
- The Redragon M612 mouse: its protocol is only guessed in public, so it gets its own phase after Redragon's M612 software is read.
- GameSense on non-Apex devices, and GameSense on Windows.
- Per-key animated effects beyond position-based ones.
