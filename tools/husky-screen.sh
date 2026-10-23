#!/usr/bin/env bash
# Grab what the display server is showing on husky: the frame read straight out of DRAM over the bridge (64 KiB at a time, ~10 s for a WQHD frame).
# Writes <out>.bin (the raw 32-bit words), <out>-codes.png (the 8-bit codes the panel receives, as plain RGB) and <out>-srgb.png (the panel's light as an ordinary sRGB image: code^2.19, re-encoded, white at the panel's peak — for looking at on another screen; it is not what the eye sees on the phone).
# The layer is 10-bit (B[31:22] G[21:12] R[11:2] A[1:0], low two bits of each code zero: the 8-bit link) when the shell runs; --bgrx reads an 8-bit BGRX layer (the kernel's own screens).
# Which buffer: --phase N (0-3) reads that temporal dither frame (G#8C00_0000 + N × 16 MiB; the layer flips through all four), --fb the single framebuffer (G#8400_0000), --addr G#… any other.
# Usage: tools/husky-screen.sh [out prefix, default target/screen/<time>] [--phase N | --fb | --addr G#…] [--bgrx]
set -euo pipefail
REPO=/mnt/Harbor/Code/ferros
B=$REPO/target/debug/ferros-bridge
W=1344; H=2992
OUT=""; BASE=$((0x8C000000)); FMT=ten
while [ $# -gt 0 ]; do case $1 in
    --phase) BASE=$((0x8C000000 + $2 * 0x1000000)); shift 2;;
    --fb) BASE=$((0x84000000)); shift;;
    --addr) BASE=$((16#${2#G#})); shift 2;;
    --bgrx) FMT=bgrx; shift;;
    *) OUT=$1; shift;;
esac; done
[ -n "$OUT" ] || { mkdir -p "$REPO/target/screen"; OUT=$REPO/target/screen/$(date +%H%M%S); }
BYTES=$((W * H * 4)); CHUNK=65536
rm -f "$OUT.bin"
for ((off = 0; off < BYTES; off += CHUNK)); do
    n=$(( BYTES - off < CHUNK ? BYTES - off : CHUNK ))
    timeout 30 "$B" read "$(printf %X $((BASE + off)))" "$(printf %X $n)" "$OUT.part" >/dev/null 2>&1 || { echo "read failed at +$off"; exit 1; }
    cat "$OUT.part" >> "$OUT.bin"
done
rm -f "$OUT.part"
python3 - "$OUT" $W $H $FMT <<'PY'
import sys, numpy as np
from PIL import Image
out, w, h, fmt = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
v = np.fromfile(out + ".bin", dtype="<u4")[: w * h].reshape(h, w)
if fmt == "ten":
    r, g, b = (v >> 4) & 0xFF, (v >> 14) & 0xFF, (v >> 24) & 0xFF
else:
    r, g, b = (v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF
codes = np.stack([r, g, b], -1).astype(np.uint8)
Image.fromarray(codes).save(out + "-codes.png")
light = (codes / 255.0) ** 2.19
srgb = np.where(light <= 0.0031308, light * 12.92, 1.055 * light ** (1 / 2.4) - 0.055)
Image.fromarray((srgb * 255 + 0.5).astype(np.uint8)).save(out + "-srgb.png")
print(f"{out}.bin, {out}-codes.png, {out}-srgb.png")
PY
