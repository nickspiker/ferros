#!/usr/bin/env bash
# The main camera's image and phase-detect rows, kept as a pair by payloads/camera's SNAP (camtest's fourth byte 6), pulled by physical address while camtest holds the camera, and packed into one VSF (docs/CAMERA.md "Phase detect as data").
# Usage: tools/camera-pd-pair.sh <image G#addr> <PD G#addr> <out prefix> <count> <exposure_ns> <gain> <focus> <eagle_ns>
#   the addresses and fields as the driver's "camera: SNAP frame" line and camtest's "camtest: SNAP frame" line say them (hex addresses without G#).
# Output: <prefix>.image.raw10, <prefix>.pd.raw10 (as the front end wrote them), <prefix>.vsf (tools/camera-pd-pair).
# The pair is local only (the scene and the sensor's own settings): keep it in /mnt/Harbor/Code/keys/husky/captures, never in the repository.
set -euo pipefail
cd "$(dirname "$0")/.."
IMG=$((0x$1)); PD=$((0x$2)); OUT=$3
B=${BRIDGE:-target/debug/ferros-bridge}
TOOL=${PAIR:-tools/camera-pd-pair/target/release/camera-pd-pair}
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
"$TOOL" "$OUT.image.raw10" "$OUT.pd.raw10" "$OUT.vsf" "$4" "$5" "$6" "$7" "$8"
b3sum "$OUT.image.raw10" "$OUT.pd.raw10" "$OUT.vsf"
