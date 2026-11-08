#!/bin/bash
# Photon as an installed kist (KISTS.md), the second after Mahere: its development image (tools/photon-build.sh; `--no-build` takes the last one) signed into a capsule that declares what it wants, each right with Photon's reason, in its grant slot order (std/photon: 1 display, 2 touch, 3 surface, 4 udp, 5 its own vault domain, 6 ira, 7 entropy), then installed for the person attested on the device: the capsule (about 100 MB) kept in the vault in parts under its blake3, the kist added to their list. Its circle appears on the map at the next tap on the plane; its first launch asks each want once.
# The option is Photon's identity across versions (ORACLE.md §5: minted once, kept), so an update keeps the person's answers and its vault domain. Minted 2026-11-08; never change it.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
[ "${1:-}" = "--no-build" ] || tools/photon-build.sh
bin=std/photon/target/aarch64-unknown-ferros/debug/photon.bin
out=target/photon.capsule.vsf
B=target/debug/ferros-bridge
"$B" capsule "$bin" "$out" --kist Photon --option 8c2411c6bf1c10a45d8021340b2dae4a7c69774235069eddd85abbdc6959eb74 \
    --want display "to show your conversations on the screen" \
    --want touch "to read your taps and typing" \
    --want surface "to draw in a window of its own" \
    --want udp "to send and receive your photons over the network" \
    --want domain "to keep your contacts and conversations" \
    --want ira "to tie its keys to this device" \
    --want entropy "to make the keys your photons are sealed with"
"$B" kist install "$out"
