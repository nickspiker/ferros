#!/usr/bin/env python3
"""Build a sensor init table for ferros (`replay_init` in camera.rs) from a full-byte HS-I2C decode of an oracle capture.

Input: `tools/oracle-decode-hsi2c.py <capture> <controller base> --max-bytes 1000000` output (no summarised transfers). The table holds one sensor's transfers from a `# MARK` onward, up to (not including) its stream-on; `diag cam pdma` streams on itself.

Records, as `replay_init` reads them:
  'W' len(u16 BE) bytes      an I2C write
  'R' n(u16 BE) addr(2)      a two-byte address write then an n-byte read (the value is not checked)
  'D' us(u32 BE)             a delay
Delays follow the main camera's table, which has worked since 2026-10-19: 10 ms after each of the first three transfers, 5 ms after any write longer than 256 bytes (a settings upload), 1 ms after every other write. The capture has no timestamps, so these are generous rather than measured.

The output carries the sensor vendor's settings: write it under target/ (never the repo) and embed it with FERROS_CAM_FRAME or put it in the vault.
Usage: tools/camera-init.py <decoded.txt> <target, e.g. G#1A> <out.bin> [--mark NAME] [--stream-on "01 00 01"]
"""
import struct, sys

args = sys.argv[1:]
mark, son = None, "01 00 01"
if "--mark" in args:
    i = args.index("--mark"); mark = args[i + 1]; del args[i:i + 2]
if "--stream-on" in args:
    i = args.index("--stream-on"); son = args[i + 1]; del args[i:i + 2]
src, target, out = args
lines = open(src).read().splitlines()
started = mark is None
xfers = []
for l in lines:
    if l.startswith("# MARK"):
        if mark is not None and l[7:].strip() == mark:
            started = True
        continue
    if not started or l.startswith("#") or not l.startswith(target + " "):
        continue
    body = l.split(" #")[0].split()
    if "...[" in l:
        sys.exit(f"summarised transfer in the decode (re-run the decoder with --max-bytes 1000000): {l[:60]}")
    if len(body) < 2:
        continue
    d, data = body[1], bytes(int(x, 16) for x in body[2:])
    if d == "W" and data.hex(" ").upper() == son.upper().replace("G#", ""):
        break
    xfers.append((d, data))
recs, n = bytearray(), 0
i = 0
while i < len(xfers):
    d, data = xfers[i]
    if d == "W" and len(data) == 2 and i + 1 < len(xfers) and xfers[i + 1][0] == "R":
        recs += b"R" + struct.pack(">H", len(xfers[i + 1][1])) + data
        i += 2
    elif d == "W":
        recs += b"W" + struct.pack(">H", len(data)) + data
        recs += b"D" + struct.pack(">I", 10000 if n < 3 else 5000 if len(data) > 256 else 1000)
        i += 1
    else:
        recs += b"R" + struct.pack(">H", len(data)) + b"\0\0"
        i += 1
    n += 1
open(out, "wb").write(bytes(recs))
print(f"{n} transfers before stream-on -> {len(recs)} bytes in {out}")
