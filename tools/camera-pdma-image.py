#!/usr/bin/env python3
"""Build the ISP front end's PDMA job for ferros (`diag cam pdma`) from a raw hv dump of a working one.

The front end's coefficient/statistics region is not CPU-writable, so the register program cannot be replayed with CPU writes (docs/oracle/CAMERA.md): the PDMA engine has to execute it. This lays the whole job out at fixed physical addresses ferros can hand the engine directly (the front end's SysMMUs are in bypass on a cold boot, so a bus address IS a physical address), with every address inside the captured list rewritten to point at ferros memory instead of the Android session's.

Layout (free DRAM on a cold boot: above ferros's window and the display regions, below the first reserved carveout at G#9000_0000):
  G#8800_0000  the descriptor ring, 16-byte entries {flags G#C003, list address, 0, length}
  G#8801_0000  the register-write list, as the engine reads it
  G#8802_0000  the sources of the list's block-load records, one 4 KiB-aligned slice each
  G#8810_0000  the output buffers, BUF_SLICE each, one slice per distinct address in the list, wrapping over BUF_SLOTS slices

A slice has to hold a frame. The captured mode is 4080 x 3072 RAW10, which is 15 MiB, so the original 2 MiB slices could never have held one however correctly everything else was programmed; free DRAM here runs G#8810_0000 to G#9000_0000, which is 126 MiB, enough for seven 16 MiB slices. There are more distinct addresses than that in the list, so they wrap: the ones that matter are the image channels, and a collision between two of the rest only matters once a frame actually lands.

Output: 'CPR3', u32 blob count, then per blob {u32 physical address, u32 length, bytes padded to 4}.
Usage: tools/camera-pdma-image.py <hv dump> <out file> [--list N]
"""
import re, struct, sys

RING_PA, LIST_PA, LOAD_PA, BUF_PA = 0x8800_0000, 0x8801_0000, 0x8802_0000, 0x8810_0000
BUF_SLICE = 0x100_0000
# Two slices, not the seven that fit between BUF_PA and the carveout at G#9000_0000: writing a pattern as far as G#8F10_0000 reset the phone twice, while G#8810_0000 to G#8AA0_0000 has been written for days without trouble, so something in between is not ours. Two 16 MiB slices end at G#8A10_0000, inside the proven range, and two is enough for the image channels that matter.
BUF_SLOTS = 2
LOAD_SLICE = 0x1000
IOVA_LO, IOVA_HI = 0x1700_0000, 0x1E00_0000
RING_ENTRIES = 16

# --list picks which of the session's rotating lists to build from, and it matters: list 2 is the one docs/oracle/CAMERA.md decoded and the one ferros has run, at G#19DC bytes with 18 output buffers, while the default list 1 is G#1A10 with 20. Handing the engine list 1 reset the phone twice before the difference was noticed (2026-10-20). Always pass `--list 2` until there is a reason not to.
args = [a for a in sys.argv[1:]]
which = 1
if '--list' in args:
    i = args.index('--list'); which = int(args[i + 1]); del args[i:i + 2]
src, out = args[0], args[1]

words, loads, limit, k = [], {}, None, 0
for ln in open(src, errors='replace'):
    if 'PDMA buffer ' in ln and 'unmapped' not in ln:
        k += 1
        if k == which:
            m = re.search(r'length ([0-9A-F]+)', ln)
            limit = int(m.group(1), 16) // 4 if m else None
        continue
    if k != which:
        continue
    m = re.search(r'PDMA buf \+([0-9A-F]+)((?: [0-9A-F]{8}){8})', ln)
    if m:
        words += [int(x, 16) for x in m.group(2).split()]
    m = re.search(r'PDMA load ([0-9A-F]+) \+([0-9A-F]+)((?: [0-9A-F]{8}){8})', ln)
    if m:
        reg, off = int(m.group(1), 16), int(m.group(2), 16)
        for j, x in enumerate(m.group(3).split()):
            loads.setdefault(reg, {})[off + 4 * j] = int(x, 16)
if limit:
    words = words[:limit]
assert words, "no list words in the dump"

# Walk the list: rewrite every page-aligned bus address in a (register, value) pair to a buffer slice, and every block-load source to its own slice.
bufs, load_order = {}, []
i = 0
while i < len(words):
    h = words[i]; n = h >> 14; op = h & 0x3FFF
    body = slice(i + 1, i + 1 + n)
    if op == 2:
        for k2 in range(i + 2, i + 1 + n, 2):
            v = words[k2]
            if IOVA_LO <= v < IOVA_HI and v & 0xFFF == 0:
                words[k2] = bufs.setdefault(v, BUF_PA + (len(bufs) % BUF_SLOTS) * BUF_SLICE)
    elif op == 1 and n:
        # A burst writes consecutive registers from one start address; buffer addresses appear in these too.
        for k2 in range(i + 2, i + 1 + n):
            v = words[k2]
            if IOVA_LO <= v < IOVA_HI and v & 0xFFF == 0:
                words[k2] = bufs.setdefault(v, BUF_PA + (len(bufs) % BUF_SLOTS) * BUF_SLICE)
    elif op == 9 and n == 4:
        reg, size = words[i + 4], words[i + 3]
        data = loads.get(reg, {})
        payload = b''.join(struct.pack('<I', data.get(o, 0)) for o in range(0, size, 4))
        pa = LOAD_PA + len(load_order) * ((size + LOAD_SLICE - 1) // LOAD_SLICE) * LOAD_SLICE
        pa = LOAD_PA + sum((len(p) + LOAD_SLICE - 1) // LOAD_SLICE for _, p in load_order) * LOAD_SLICE
        load_order.append((pa, payload))
        words[i + 1] = pa
        if len(payload) * 1 < size:
            print(f"warning: block load for G#{reg:X} has {len(payload)} of {size} bytes captured")
    elif h == 0:
        break
    i += 1 + n

list_bytes = b''.join(struct.pack('<I', w) for w in words)
ring = b''.join(struct.pack('<IIII', 0xC003, LIST_PA, 0, len(list_bytes)) for _ in range(RING_ENTRIES))
blobs = [(RING_PA, ring), (LIST_PA, list_bytes)] + load_order
body = b''.join(struct.pack('<II', pa, len(b)) + b + b'\0' * (-len(b) % 4) for pa, b in blobs)
open(out, 'wb').write(struct.pack('<4sI', b'CPR3', len(blobs)) + body)
print(f"list {len(list_bytes)} B, {len(load_order)} block-load sources, {len(bufs)} output buffers remapped from G#{min(bufs):X}.. to G#{BUF_PA:X}.., G#{BUF_SLICE:X} a slice over {BUF_SLOTS} slices")
print(f"ring {RING_ENTRIES} entries at G#{RING_PA:X}; total {8 + len(body)} bytes -> {out}")
