#!/usr/bin/env bash
# Build and install masterkeysd for the current user.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release -p masterkeysd
install -Dm755 target/release/masterkeysd ~/.local/bin/masterkeysd
install -Dm644 assets/masterkeysd.service ~/.config/systemd/user/masterkeysd.service
systemctl --user daemon-reload
echo "Installed. Start it now and at every login with:"
echo "  systemctl --user enable --now masterkeysd"
