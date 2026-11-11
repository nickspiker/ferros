#!/bin/bash
# Mahere as an installed kist (KISTS.md): its development image (tools/mahere-build.sh; `--no-build` takes the last one) signed into a capsule that declares what it wants, each right with Mahere's reason, in its grant slot order (mahere-ferros: 1 display, 2 touch, 3 surface, 4 ira, 5 its own vault domain, 6 entropy, 7 gps), then installed for the person attested on the device: the capsule kept in the vault under its blake3, the kist added to their list. Its circle appears on the map at the next tap on the plane; its first launch asks each want once.
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
    --want gps "to show where you are on the map"
"$B" kist install "$out"
