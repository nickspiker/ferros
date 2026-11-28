#!/bin/bash
# ferros's own pucks as kists (docs/PUCKS.md; owner, 2026-11-17: "plus a tide puck", "GPS, compass, shit that's useful! Pucks! I'm DOWN!"; 2026-11-20 the thermometer; 2026-11-28 the battery and the timer): tide, sun, moon, compass, GPS, weather, the thermometer, the battery and the timer, each built masked (tools/ferros-masked-build.sh: the network stower refuses native code) and signed by the ferros project key with every egg (kistcat kist), since ferros is a commons and its own pucks are ferros's (memory kist_keys_follow_ownership; KISTS.md "Whose key signs a kist").
# Each capsule declares `puck` (its circle: never asked) and `gps` (its place, asked once, default off); the weather `udp` as well; the thermometer `puck` alone (its mailbox is open to all: owner, 2026-11-20), the battery `puck` alone (the facts are anyone's), and the timer `puck` alone (its state is in its own vault domain). Written to $OUT/<name>.capsule.vsf.
# Not stowed by this script: a device runs a puck only after its display, map, init, kists and kernel know pucks (PUCKS.md "What changes on the device"); stow each with `cargo run -p ferros-send -- send <capsule>` once those are on the device.
# The options are each puck's identity across versions (ORACLE.md section 5: minted once, 2026-11-17, kept; the battery's 2026-11-28, blake3 of "ferros puck battery", and the timer's the same day, blake3 of "ferros puck timer"); never change them. Raise --version with every capsule published.
# Environment: FERROS_PROJECT_KEY (default keys/ferros.key.vsf, label FERROS_PROJECT; the path is passed, the key never leaves kistcat's process), OUT (default target/kists), and the masked build's FERROS_REPO, FERROS_MASKED_OUT and FERROS_MASK for a build from a worktree.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
key=${FERROS_PROJECT_KEY:-/mnt/Harbor/Code/keys/ferros.key.vsf}
out=${OUT:-$here/target/kists}
masked=${FERROS_MASKED_OUT:-${FERROS_REPO:-$here}/target/masked}
mkdir -p "$out"
version=${VERSION:-0}
# name, bin, option, wants (right and reason pairs; reasons printable ASCII: the device's stower is no_std).
pucks=(
    "Tide|puck_tide|7b261f33bc9346c951bdf8e82562629e0a213becee2e0a40fca416690a14fbb3|gps|the tide at the station nearest you"
    "Sun|puck_sun|8f495fd0e63cc61f67fc28a178ce6003d2ca223ac736546d1b70be2956c15b94|gps|sunrise and sunset where you are"
    "Moon|puck_moon|b43570d83571f88f8c64e5195453f3c6b8dca61aac4c8bcb717e46115ced459b|gps|moonrise and moonset where you are"
    "Compass|puck_compass|82b4ac98cf81bc82246b00ebb7198f48afce4555b71c878bb7ed71828b05da74|gps|true north where you are"
    "GPS|puck_gps|a90c8ba8a5bb470f6c9f08495f31d0faf88f63a9ab997d83fbfad020576d8b5b|gps|the satellites and how well they place you, while you look"
    "Weather|puck_weather|889a46078ac08d6a7905ee1bbf41866c1ed248cb5b769cf77e4aa718c86ded8c|gps|the weather where you are, to 10 km|udp|to ask fgtw.org for the forecast"
    "Thermometer|puck_thermo|9103301d7ac9e379a867c3cf413e5a3ed016848712a6fb30993632b6b0518278"
    "Battery|puck_battery|bc974417c04be4f3ee7a8ca423a2492529cea089c2860ad2d4de7be4fbecfa9e"
    "Timer|puck_timer|8332ee5dc9f6cd5c529e4d73d3f10bc5770a49cd5b6b9969a2aebc5543fc0f65"
)
for p in "${pucks[@]}"; do
    IFS='|' read -r name bin option rest <<< "$p"
    tools_dir="$here/tools"
    if [ -z "${SKIP_BUILD:-}" ]; then
        "$tools_dir/ferros-masked-build.sh" pucks "$bin"
    fi
    wants=(--want puck "its circle on the deck")
    IFS='|' read -r -a extra <<< "$rest"
    for ((i = 0; i < ${#extra[@]}; i += 2)); do
        wants+=(--want "${extra[i]}" "${extra[i+1]}")
    done
    cargo run -q --release --manifest-path "$here/tools/kistcat/Cargo.toml" -- kist --key "$key" \
        --masked "$masked/$bin/$bin.bin" --text-len "$(cat "$masked/$bin/$bin.textlen")" \
        --name "$name" --option "$option" --version "$version" "${wants[@]}" \
        --out "$out/$bin.capsule.vsf"
    echo "pucks-kist: $name -> $out/$bin.capsule.vsf"
done
