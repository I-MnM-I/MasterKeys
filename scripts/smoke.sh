#!/usr/bin/env bash
# Hardware smoke test for the Apex 3: watch the keyboard and answer each prompt.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -q -p steelseries-gg --example apex3_smoke
bin=target/debug/examples/apex3_smoke

step() {
    local expect=$1; shift
    "$bin" "$@"
    read -rp "$expect ok? [y/n] " answer
    [[ $answer == y* ]] || { echo "FAILED: $expect"; exit 1; }
}

step "all red"            all ff0000
step "all green"          all 00ff00
step "rainbow left→right" zones ff0000 ff8000 ffff00 80ff00 00ff00 00ff80 00ffff 0080ff 0000ff 8000ff
step "dim (20%)"          brightness 20
step "bright (100%)"      brightness 100
step "all white"          all ffffff
echo "smoke test passed"
