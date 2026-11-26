MasterKeys for Windows 11
=========================

Key remaps, macros and profiles that switch with your games, for any keyboard.

GETTING STARTED
1. Unzip this folder somewhere you'll keep it (for example Documents\MasterKeys).
2. Run masterkeys.exe. Windows SmartScreen will warn that the app is unrecognised,
   because it is not signed: click "More info", then "Run anyway".
3. MasterKeys lives in the system tray (the keyboard icon by the clock). Closing the
   window keeps it running there; choose Quit from the tray icon to stop it.
4. Tick "Start with Windows" at the bottom of the window to have it start at login.
5. Click a key in the picture (or "Select by pressing") to remap it or record a macro.

IF REMAPPING GOES WRONG
Hold both Ctrl keys and both Shift keys for 2 seconds: remapping stops and every key
types normally again until you change a binding or switch profile.

UPDATES
When a new version is out, a bar at the top of the window says so. Click Update and
MasterKeys downloads it, checks it and restarts.

KNOWN LIMITS
- Some games read the keyboard directly (often ones with anti-cheat) and ignore remaps.
- Programs running as administrator only see remaps if MasterKeys is also run as
  administrator.
- Lighting control for Redragon keyboards and mice is not in this version yet.

WHERE THINGS ARE
Profiles and the log file are in %APPDATA%\MasterKeys (paste that into Explorer's
address bar). If something goes wrong, send masterkeys.log.

TESTER CHECKLIST (please try each and report what happens)
1. Remap a key (for example Caps Lock to Escape) and check it in Notepad.
2. Record a short macro. Try each mode: Once, Repeat while held, Toggle repeat
   (a second press stops it) and Repeat N times.
3. With a remap active, hold both Ctrls and both Shifts for 2 s: remapping stops and
   typing still works.
4. Make a profile, link it to notepad.exe, and check it turns on when Notepad starts.
5. Close the window: the tray icon stays. Quit from the tray: remaps stop.
6. Launch masterkeys.exe again while it runs: the existing window comes up.
7. Tick Start with Windows, restart the PC: MasterKeys is in the tray.
8. When an update is offered, click Update: it restarts on the new version.
