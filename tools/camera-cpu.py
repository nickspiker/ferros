#!/usr/bin/env python3
"""A camera's CPU-side setup for `diag cam pdma`, from an ISP front-end oracle capture of its open.

From the capture's lines [a, b) (setup before the PDMA start) and (b, c) (after it, up to the first frame), writes:
  <out>.cpr2      'CPR2' with no buffers: the CPU writes to the CSI links and the ISP front end before the PDMA start, an empty program section, the ones after. The PHY range (G#1C10_xxxx) and the PDMA's own page are left out (the receiver table carries the PHY; the PDMA is started by ferros), and so are the 25 address registers, which the PDMA program sets every frame (tools/camera-pdma-image.py ADDR_ALL): a CPU-written IOVA would point the front end into nothing before the program's first run.
  <out>.rx.rs     the receiver start as a Rust table: every write to the PHY (G#1C10_xxxx), to the link `--link` and to the CSIS reset word (G#1C02_0500), in the capture's order over [a, c).
Usage: tools/camera-cpu.py <capture.log> <a> <b> <c> <out prefix> --link G#1C06 --name RX_START_UW
"""
import re, struct, sys

args = sys.argv[1:]
link, name = 0x1C06, "RX_START_UW"
if "--link" in args:
    i = args.index("--link"); link = int(args[i + 1].replace("G#", "").replace("_", ""), 16); del args[i:i + 2]
if "--name" in args:
    i = args.index("--name"); name = args[i + 1]; del args[i:i + 2]
src, a, b, c, out = args[0], int(args[1]), int(args[2]), int(args[3]), args[4]
ADDR_ALL = {0x1C320510, 0x1C3205C0, 0x1C322110, 0x1C3221C0, 0x1C3400D0, 0x1C351820, 0x1C3548A0, 0x1C354A84, 0x1C354ECC, 0x1C354FC8, 0x1C355168, 0x1C355178, 0x1C355180, 0x1C355188, 0x1C355190, 0x1C355198, 0x1C3551A0, 0x1C3551A8, 0x1C3551B0, 0x1C3551C0, 0x1C3551C8, 0x1C3551E0, 0x1C3551E8, 0x1C3551F8, 0x1C355200}
pat = re.compile(r"S2 ([0-9A-F]{16}) W4 ([0-9A-F]{16})")
pre, post, rx = [], [], []
for n, ln in enumerate(open(src, errors="replace"), 1):
    if n < a or n >= c:
        continue
    m = pat.search(ln)
    if not m:
        continue
    reg, v = int(m[1], 16), int(m[2], 16)
    if 0x1C10_0000 <= reg < 0x1C11_0000 or reg >> 16 == link or reg == 0x1C02_0500:
        rx.append((reg, v))
    if not (0x1C04_0000 <= reg < 0x1C40_0000) or 0x1C39_0000 <= reg < 0x1C39_1000 or 0x1C10_0000 <= reg < 0x1C11_0000 or reg in ADDR_ALL:
        continue
    if n < b:
        pre.append((reg, v))
    elif n > b:
        post.append((reg, v))
sec = lambda l: struct.pack("<I", len(l)) + b"".join(struct.pack("<II", r, v) for r, v in l)
open(out + ".cpr2", "wb").write(struct.pack("<4sI", b"CPR2", 0) + sec(pre) + sec([]) + sec(post))
body = "\n".join(f"    (0x{r >> 16:04X}_{r & 0xFFFF:04X}, 0x{v:X})," for r, v in rx)
open(out + ".rx.rs", "w").write(f"pub const {name}: [(usize, u32); {len(rx)}] = [\n{body}\n];\n")
print(f"{len(pre)} CPU writes before, {len(post)} after; receiver start {len(rx)} writes -> {out}.cpr2, {out}.rx.rs")
