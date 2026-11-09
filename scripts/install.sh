#!/usr/bin/env bash
# Build and install masterkeysd (daemon) and masterkeys (GUI) for the current user.
set -euo pipefail
cd "$(dirname "$0")/.."
build="cargo build --release -p masterkeysd -p masterkeys"
if [[ -e /run/.containerenv ]] || pkg-config --exists fontconfig xkbcommon 2>/dev/null; then
    $build
else
    # The host lacks the GUI's build headers (Bazzite); build in the rexbuild distrobox.
    distrobox enter rexbuild -- bash -lc "cd '$PWD' && $build"
fi
install -Dm755 target/release/masterkeysd ~/.local/bin/masterkeysd
install -Dm755 target/release/masterkeys ~/.local/bin/masterkeys
install -Dm644 assets/masterkeysd.service ~/.config/systemd/user/masterkeysd.service
install -Dm644 assets/masterkeys.desktop ~/.local/share/applications/masterkeys.desktop
install -Dm644 assets/masterkeys-tray.desktop ~/.config/autostart/masterkeys-tray.desktop
systemctl --user daemon-reload
echo "Installed. Start the daemon now and at every login with:"
echo "  systemctl --user enable --now masterkeysd"
echo "If it was already running, restart it: systemctl --user restart masterkeysd"
echo "The tray icon starts at login; to stop that, delete ~/.config/autostart/masterkeys-tray.desktop"
