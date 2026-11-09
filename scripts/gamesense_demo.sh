#!/usr/bin/env bash
# Pretend to be a GameSense game: zone 1 fades red→green, the F-keys turn blue, then the game quits.
set -euo pipefail
url="http://$(python3 -c 'import json;print(json.load(open("/tmp/steelseries-engine/coreProps.json"))["address"])')"
post() { curl -s -o /dev/null -H 'Content-Type: application/json' -d "$2" "$url/$1"; }
post game_metadata '{"game":"DEMO","game_display_name":"MasterKeys demo","developer":"MasterKeys"}'
post bind_game_event '{"game":"DEMO","event":"HEALTH","handlers":[{"device-type":"rgb-zoned-device","zone":"one","color":{"gradient":{"zero":{"red":255,"green":0,"blue":0},"hundred":{"red":0,"green":255,"blue":0}}}}]}'
post bind_game_event '{"game":"DEMO","event":"AMMO","handlers":[{"device-type":"keyboard","zone":"function-keys","color":{"red":0,"green":0,"blue":255}}]}'
echo "F-keys blue, leftmost zone fading red → green…"
post game_event '{"game":"DEMO","event":"AMMO","data":{"value":1}}'
for v in 0 20 40 60 80 100; do
    post game_event "{\"game\":\"DEMO\",\"event\":\"HEALTH\",\"data\":{\"value\":$v}}"
    sleep 1
done
echo "Holding for 5 s, then the game quits and your profile comes back."
sleep 5
post remove_game '{"game":"DEMO"}'
echo "Done."
