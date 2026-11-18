#!/usr/bin/env bash
# The main camera's image and its phase-detect rows, held as a pair by payloads/camera's SNAP (camtest's fourth byte 8), pulled by physical address while camtest holds the buffer, and packed into one VSF visual (docs/CAMERA.md "Phase detect as data").
# Usage: tools/camera-pd-pair.sh <out prefix> "<camtest's PAIR line>"
#   the line as the console has it: `camtest: PAIR A#k image G#.. pd G#.. count A#.. exposure_ns A#.. gain A#.. focus A#.. eagle_ns A#.. status A#..`
# Output: <prefix>.image.raw10 and <prefix>.pd.raw10 (as the front end wrote them), <prefix>.vsf (tools/camera-pd-pair: the sensor plane and a `phase` plane), and their blake3.
# The pairs are local only (the scene and the sensor's own settings): keep them in /mnt/Harbor/Code/keys/husky/captures, never in the repository.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=$1; LINE=$2
B=${BRIDGE:-target/debug/ferros-bridge}
TOOL=${PAIR:-tools/camera-pd-pair/target/release/camera-pd-pair}
field() { sed -E "s/.* $1 [AG]#([0-9A-Fa-f]+).*/\1/" <<< "$LINE"; }
IMG=$((0x$(field image))); PD=$((0x$(field pd)))
[ "$(field status)" = 0 ] || { echo "the driver says this pair's PD rows were never written (status $(field status))"; exit 1; }
# The main's frame: 3072 lines of G#1400 bytes; its PD rows: 768 lines of G#1400. Read 64 KiB at a time, as tools/camera-pull.sh does.
pull() {
    local base=$1 bytes=$2 out=$3
    : > "$out"
    local chunks=$(( (bytes + 0xFFFF) / 0x10000 ))
    for i in $(seq 0 $((chunks - 1))); do
        a=$(printf "%X" $((base + i * 0x10000)))
        timeout 30 "$B" read "$a" 10000 "$out.chunk" >/dev/null 2>&1
        cat "$out.chunk" >> "$out"
    done
    rm -f "$out.chunk"
    truncate -s "$bytes" "$out"
}
pull $IMG $((3072 * 0x1400)) "$OUT.image.raw10"
pull $PD $((768 * 0x1400)) "$OUT.pd.raw10"
"$TOOL" "$OUT.image.raw10" "$OUT.pd.raw10" "$OUT.vsf" "$(field count)" "$(field exposure_ns)" "$(field gain)" "$(field focus)" "$(field eagle_ns)"
b3sum "$OUT.image.raw10" "$OUT.pd.raw10" "$OUT.vsf"
