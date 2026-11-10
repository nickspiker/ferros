#!/usr/bin/env bash
# akita's AoC firmware and sensor registry into ferros's vault, from the factory image of the build the 8a runs (SENSORS.md, BOARDS.md "Sensors"): /vendor/firmware/aoc.bin as `aoc.fw.0`.. (tools/husky-aoc-fw.sh's 2 MiB pieces) and /vendor/etc/sensors/registry's scripts as `aoc.reg.0`.. in the order Android's sensor HAL reads them.
# The order is ext4's readdir on a one-block directory with dir_index: the directory hash (signed half_md4 with vendor.img's seed, `debugfs dx_hash`), which on cp2a.260805.005 gives pvt, common, dvt, proto, dev, then append/'s motion_state, power. Each script carries a CDT filter for its own build stage, so only the 8a's applies.
# Then the phone's own factory calibration, /sensors/registry on persist (f2fs), in its directory's entry order (the order f2fs's readdir gives Android), read from a persist image with tools/f2fs-cat (ferros_f2fs, the kernel's own reader).
# Google's files and the phone's calibration never enter the repo: they are read into a temporary directory and stored.
# Usage: tools/akita-aoc.sh [image-akita-<build>.zip] [persist.img]
set -euo pipefail
REPO=/mnt/Harbor/Code/ferros
BRIDGE=$REPO/target/debug/ferros-bridge
ZIP=${1:-/mnt/Harbor/tmp/akita-factory/akita-cp2a.260805.005/image-akita-cp2a.260805.005.zip}
PERSIST=${2:-/mnt/Harbor/tmp/akita-backup-2026-11-06/persist.img}
[ -r "$ZIP" ] || { echo "no $ZIP" >&2; exit 1; }
[ -r "$PERSIST" ] || { echo "no $PERSIST" >&2; exit 1; }
F2FS=$REPO/tools/f2fs-cat/target/release/f2fs-cat
[ -x "$F2FS" ] || (cd "$REPO/tools/f2fs-cat" && cargo build -q --release)
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
unzip -q -o "$ZIP" vendor.img -d "$T"
debugfs -R "dump /firmware/aoc.bin $T/aoc.bin" "$T/vendor.img" 2>/dev/null
"$REPO/tools/husky-aoc-fw.sh" "$T/aoc.bin"
n=0
for f in akita_pvt.reg akita_common.reg akita_dvt.reg akita_proto.reg akita_dev.reg append/motion_state.reg append/power.reg; do
    debugfs -R "dump /etc/sensors/registry/$f $T/reg" "$T/vendor.img" 2>/dev/null
    [ -s "$T/reg" ] || { echo "== skip $f (missing)"; continue; }
    printf '== aoc.reg.%d %s (%d bytes): ' $n "$f" "$(stat -c%s "$T/reg")"
    timeout 60 "$BRIDGE" store put "aoc.reg.$n" "@$T/reg" 2>&1 | tail -1
    rm -f "$T/reg"
    n=$((n + 1))
done
for f in $("$F2FS" "$PERSIST" ls /sensors/registry | awk '$2 == "file" && $1 ~ /\.reg$/ {print $1}'); do
    "$F2FS" "$PERSIST" get "/sensors/registry/$f" "$T/reg"
    printf '== aoc.reg.%d persist %s (%d bytes): ' $n "$f" "$(stat -c%s "$T/reg")"
    timeout 60 "$BRIDGE" store put "aoc.reg.$n" "@$T/reg" 2>&1 | tail -1
    rm -f "$T/reg"
    n=$((n + 1))
done
for k in $(seq $n $((n + 8))); do timeout 30 "$BRIDGE" store rm "aoc.reg.$k" >/dev/null 2>&1 || true; done
echo "== $n scripts"
