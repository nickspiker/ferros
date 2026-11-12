#!/bin/bash
# Mahere as an installed kist (KISTS.md): its development image (tools/mahere-build.sh; `--no-build` takes the last one) signed into a capsule that declares what it wants, each right with Mahere's reason, in its grant slot order (mahere-ferros: 1 display, 2 touch, 3 surface, 4 ira, 5 its own vault domain, 6 entropy, 7 gps, 8 udp: the network, added 2026-11-11 after the rest so their slots keep their numbers), then stowed (docs/KISTS.md "Stowing"): `ferros send` (tools/ferros-send) offers it to the device over USB, the device's installer checks it as it checks a kist from the directory (native code, as Mahere's is until std apps build masked, only on a development build, its screen saying so), and the person attested there says Stow or Cancel on its screen. Its circle appears on the map once stowed; its first launch asks each want once. `ferros-bridge kist install` is the development fallback with nobody at the screen.
# The option is Mahere's identity across versions (ORACLE.md §5: the developer mints it once and keeps it), so an update keeps the person's answers and its vault domain. Minted 2026-11-08; never change it.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
[ "${1:-}" = "--no-build" ] || tools/mahere-build.sh
bin=/mnt/Harbor/Code/mahere/target/ferros/aarch64-unknown-ferros/${MAHERE_PROFILE:-release}/mahere.bin
out=target/mahere.capsule.vsf
B=target/debug/ferros-bridge
"$B" capsule "$bin" "$out" --kist Mahere --option 4d725960dc438254d7f9cdc981d34b521840db34957187cdb1d4ce06e60621de \
    --want display "to draw the map on the screen" \
    --want touch "to move, turn and zoom the map with your fingers" \
    --want surface "to draw the map in a window of its own" \
    --want ira "to seal its vault to this device" \
    --want domain "to keep your places, areas and settings" \
    --want entropy "to make the keys its vault is sealed with" \
    --want gps "to show where you are on the map" \
    --want udp "to fetch the map from Mahere's servers as you look around, keeping it on this device"
# Not -q: a build, or a wait on another build's lock, says so instead of looking like a silent device.
cargo run -p ferros-send -- send "$out"
