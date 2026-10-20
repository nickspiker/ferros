#!/usr/bin/env python3
"""Build the ISP front end's register program for ferros (`diag cam frame`) from a decoded PDMA list (tools/oracle-decode-pdma.py output).

Output (little-endian u32s): 'CPRG', nbuf, nbuf buffer physical addresses, npairs, npairs x (register, value).
With --cpu <oracle raw log> <first line> <PDMA start line> <last line>: 'CPR2', nbuf, buffers, then three sections (count, pairs): the CPU's own writes to the ISPFE block and the CSI links (G#1C04_0000..G#1C3F_FFFF, not the PDMA engine's page or the D-PHY, which camera_rx.rs sets) before the PDMA start, the PDMA program, and the CPU's writes after the start up to the last line (before the per-frame interrupt handling). The CPU writes come from the full ISPFE capture of a camera open (target/hv/ispfe-full-raw.log, 2026-10-19).
Values that are page-aligned and in the IOVA window G#1700_0000..G#1E00_0000 (seen: G#17C0_0000..G#1D7E_0000) are taken as buffer addresses (the front end's output channels, e.g. +G#2_0510 / +G#2_2110 / +G#5_48A0..) and moved to physical memory: PA = G#8800_0000 + (IOVA - G#1700_0000), up to G#8F00_0000, free DRAM on a cold boot below the first carveout at G#9000_0000; the front end's SYSMMUs are put in bypass, so the registers then hold physical addresses.
"""
import re, struct, sys
argv = sys.argv[1:]
cpu = None
if '--cpu' in argv:
    i = argv.index('--cpu'); cpu = (argv[i + 1], int(argv[i + 2]), int(argv[i + 3]), int(argv[i + 4])); del argv[i:i + 5]
src, out = argv[0], argv[1]
LO, HI, PA = 0x1700_0000, 0x1E00_0000, 0x8800_0000
pairs, bufs = [], []
for ln in open(src):
    m = re.match(r'W G#([0-9A-F]+) = G#([0-9A-F]+)', ln)
    if not m:
        continue
    a, v = int(m.group(1), 16), int(m.group(2), 16)
    if LO <= v < HI and v & 0xFFF == 0:
        p = PA + (v - LO)
        print(f"remap G#{a:X}: IOVA G#{v:X} -> PA G#{p:X}")
        bufs.append(p)
        v = p
    pairs.append((a, v))
def cpu_writes(path, a, b):
    w = []
    for n, ln in enumerate(open(path, errors='replace'), 1):
        if n < a or n > b:
            continue
        m = re.search(r'S2 ([0-9A-F]{16}) W4 ([0-9A-F]{16})', ln)
        if m:
            reg, v = int(m.group(1), 16), int(m.group(2), 16)
            # The ISPFE block and the CSI links (G#1C04_0000..), but not the PDMA engine's own page (G#1C39_0000, its descriptor ring is not used): the PDMA channels and the PDMA wrapper at G#1C39_1000..G#1C39_5FFF carry per-channel enables the front end's memory path needs.
            if 0x1C04_0000 <= reg < 0x1C40_0000 and not 0x1C39_0000 <= reg < 0x1C39_1000 and not 0x1C10_0000 <= reg < 0x1C11_0000:
                w.append((reg, v))
    return w
if cpu:
    pre, post = cpu_writes(cpu[0], cpu[1], cpu[2] - 1), cpu_writes(cpu[0], cpu[2] + 1, cpu[3])
    sec = lambda l: struct.pack('<I', len(l)) + b''.join(struct.pack('<II', a, v) for a, v in l)
    b = struct.pack('<4sI', b'CPR2', len(bufs)) + b''.join(struct.pack('<I', p) for p in bufs) + sec(pre) + sec(pairs) + sec(post)
    open(out, 'wb').write(b)
    print(f"{len(pre)} CPU writes before, {len(pairs)} program writes, {len(post)} CPU writes after; {len(bufs)} buffers, {len(b)} bytes -> {out}")
    sys.exit(0)
b = struct.pack('<4sI', b'CPRG', len(bufs)) + b''.join(struct.pack('<I', p) for p in bufs) + struct.pack('<I', len(pairs)) + b''.join(struct.pack('<II', a, v) for a, v in pairs)
open(out, 'wb').write(b)
print(f"{len(pairs)} writes, {len(bufs)} buffers, {len(b)} bytes -> {out}")
