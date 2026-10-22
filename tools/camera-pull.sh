#!/usr/bin/env bash
# Pull the main camera's last frame out of ferros after `diag cam pdma`, and decode it.
# The image body lands at G#A200_0000 (the G#1C32_05C0 output, docs/oracle/CAMERA.md "First frame"): 4080 x 3072 RAW10 packed, stride G#1400, 15 MiB, read 64 KiB at a time.
# Usage: tools/camera-pull.sh <out prefix> [base, default A2000000; a focus sweep keeps frames at 98000000 + k x 1000000]   -> <prefix>.raw10 (as written by the front end) and, through tools/camera-raw10.py, <prefix>.npy + <prefix>-grey.png
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=$1; BASE=$((0x${2:-A2000000})); B=target/debug/ferros-bridge
: > "$OUT.raw10"
for i in $(seq 0 239); do
    a=$(printf "%X" $((BASE + i * 0x10000)))
    timeout 30 $B read $a 10000 "$OUT.chunk" >/dev/null 2>&1
    cat "$OUT.chunk" >> "$OUT.raw10"
done
rm -f "$OUT.chunk"
python3 tools/camera-raw10.py "$OUT.raw10" "$OUT"
