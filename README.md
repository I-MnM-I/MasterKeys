# MasterKeys

A Linux replacement for SteelSeries GG for the **SteelSeries Apex 3** (full size, US ANSI): lighting
and effects, profiles that switch with your games, key remapping and macros, a tray icon, and
GameSense lighting from games.

- `masterkeysd` — the daemon (systemd user service); the only process that talks to the keyboard.
- `masterkeys` — the control panel and tray icon (`masterkeys --tray` starts in the tray).

## Install

```bash
scripts/install.sh
systemctl --user enable --now masterkeysd
sudo cp assets/70-masterkeys.rules /etc/udev/rules.d/ && sudo udevadm control --reload && sudo udevadm trigger
```

On Bazzite the GUI builds inside the `rexbuild` distrobox (the script does this when the host lacks
the build headers). The tray starts at login; delete `~/.config/autostart/masterkeys-tray.desktop`
to stop that. Settings and profiles live in `~/.config/masterkeys/`.

## GameSense

The daemon runs a GameSense-compatible server on `127.0.0.1:51340` and writes its address to
`coreProps.json` so games find it:

- `/tmp/steelseries-engine/coreProps.json`
- every Steam Proton prefix, in every Steam library (refreshed every 30 s):
  `…/steamapps/compatdata/<id>/pfx/drive_c/ProgramData/SteelSeries/SteelSeries Engine 3/coreProps.json`

Games run through other launchers (Heroic, Lutris, Bottles) need a copy of that file in their own
prefix, at `drive_c/ProgramData/SteelSeries/SteelSeries Engine 3/coreProps.json`, containing
`{"address":"127.0.0.1:51340"}`.

Game colours show over your profile's lighting and fall back to it 15 s after the game goes quiet.
Turn it off with "GameSense lighting" in the window.

## Remapping safety

While a profile has remaps or macros the daemon grabs the keyboard. Holding **both Ctrls and both
Shifts for 2 seconds** always stops remapping.
