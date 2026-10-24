#!/usr/bin/env bash
# Put the phone's own AoC firmware into ferros's vault, for `diag aoc boot` (SENSORS.md "Starting the AoC through GSA's mailbox").
# The file is /vendor/firmware/aoc.bin from the owner's phone (pulled once to /mnt/Harbor/firmware/husky-aoc/ through Android with root); it is Google's signed binary and never enters the repo. It goes in 2 MiB pieces `aoc.fw.0`, `aoc.fw.1`, ... (a PT transfer over 1 MiB lands in the kernel's 6 MiB big-transfer region; a 4 MiB piece reset the phone on 2026-10-23, the lock holder the X3 inside the store, cause not yet measured), and any stale higher piece is removed.
# Usage: tools/husky-aoc-fw.sh [aoc.bin]
set -euo pipefail
REPO=/mnt/Harbor/Code/ferros
BRIDGE=$REPO/target/debug/ferros-bridge
SRC=${1:-/mnt/Harbor/firmware/husky-aoc/aoc.bin}
[ -r "$SRC" ] || { echo "no $SRC: pull it from Android (adb exec-out su -c 'cat /vendor/firmware/aoc.bin')" >&2; exit 1; }
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
split -b 2097152 -d -a 2 "$SRC" "$T/p."
n=0
for f in "$T"/p.*; do
    printf '== aoc.fw.%d (%d bytes): ' $n "$(stat -c%s "$f")"
    timeout 180 "$BRIDGE" store put "aoc.fw.$n" "@$f" 2>&1 | tail -1
    n=$((n + 1))
done
for k in $(seq $n $((n + 4))); do timeout 30 "$BRIDGE" store rm "aoc.fw.$k" >/dev/null 2>&1 || true; done
echo "== $n pieces, $(stat -c%s "$SRC") bytes, BLAKE3 $(b3sum --no-names "$SRC" | cut -c1-16)…"
