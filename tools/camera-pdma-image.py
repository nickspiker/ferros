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

RING_PA, LIST_PA, LOAD_PA, BUF_PA = 0x8800_0000, 0x8801_0000, 0x8802_0000, 0xA000_0000
BUF_SLICE = 0x100_0000
# As many slices as fit between BUF_PA and the first carveout, which zuma-rmem.dtsi puts at G#9000_0000: seven. Patterning that whole range was briefly blamed for resetting the phone, but all three of those runs were built from list 1 and the only run with list 2 was the one that worked; retested with list 2, seven 16 MiB slices to G#8F10_0000 are fine.
# Since 2026-10-21: 32 slices from G#A000_0000 (nothing reserved between G#9700_0000 and G#E800_0000), one per output, so no two channels share; camera.rs PDMA_BUF_PA / PDMA_BUFS agree.
BUF_SLOTS = 32
LOAD_SLICE = 0x1000
IOVA_LO, IOVA_HI = 0x1700_0000, 0x1E00_0000
# Address registers whose captured value falls outside that window: list 2 hands them G#0200_0000, G#0158_0000, G#0D18_0000 and G#0FF0_0000, which the window left alone, so with the SYSMMUs in bypass the front end was pointed at physical addresses in the register space. Found by comparing list 2 with the hv's live snapshot of a Linux stream (tools/camera-snap-list.py, 2026-10-21): each holds a different page-aligned IOVA there. G#1C32_05C0 sits in loch0's image path, beside its buffer address at +G#510.
ISPFE = 0x1C30_0000  # list addresses are offsets into the front end (tools/oracle-decode-pdma.py)
ADDR_REGS = {0x1C32_05C0, 0x1C35_51A8, 0x1C35_51E8, 0x1C35_51F8}
# Since 2026-10-22: the address registers by name, all 25, measured (the main camera's list plus the hv's live Linux snapshot), and each 16 MiB IOVA granule mapped to one slot with its offset kept. A guessed window broke on the ultrawide's session, whose IOVAs land at G#09AF_4000, G#F800_0000, G#FDFE_0000; and two registers (G#1C35_51C0/C8) hold a buffer's base and an offset into it (G#FA00_0000, G#FA02_EE00), which per-page slots split apart.
ADDR_ALL = {0x1C320510, 0x1C3205C0, 0x1C322110, 0x1C3221C0, 0x1C3400D0, 0x1C351820, 0x1C3548A0, 0x1C354A84, 0x1C354ECC, 0x1C354FC8, 0x1C355168, 0x1C355178, 0x1C355180, 0x1C355188, 0x1C355190, 0x1C355198, 0x1C3551A0, 0x1C3551A8, 0x1C3551B0, 0x1C3551C0, 0x1C3551C8, 0x1C3551E0, 0x1C3551E8, 0x1C3551F8, 0x1C355200}
# The image body (G#1C32_05C0) always gets slot 2, so it lands at G#A200_0000 for every camera (camera.rs, tools/camera-pull.sh).
IMAGE_REG, IMAGE_SLOT = 0x1C3205C0, 2
RING_ENTRIES = 1024  # camera.rs writes the same count; enough frames for a focus sweep without wrapping (Linux: 1000)

# --list picks which of the session's rotating lists to build from, and it matters: list 2 is the one docs/oracle/CAMERA.md decoded and the one ferros has run, at G#19DC bytes with 18 output buffers, while the default list 1 is G#1A10 with 20. Handing the engine list 1 reset the phone twice before the difference was noticed (2026-10-20). Always pass `--list 2` until there is a reason not to.
args = [a for a in sys.argv[1:]]
which = 1
if '--list' in args:
    i = args.index('--list'); which = int(args[i + 1]); del args[i:i + 2]
# --height N rewrites the captured program's frame geometry, so the front end is configured for our own window instead of the 4080 x 3072 the capture was taken at.
# The mode's dimensions sit in the list in recognisable form: the width G#FF0 (4080) and the height G#C00 (3072) appear packed as (height << 16) | width at about a dozen registers across the pipeline, as (width << 16) | height at one, and the height alone appears halved (G#600) and quartered (G#300). The line stride at +G#20554 and +G#22154 is G#1400, which is 4080 pixels of RAW10 rounded up to a 256-byte boundary, so it does not move while the width is unchanged.
# Only height-derived halves are touched; the width is left alone. A half that happens to hold 3072 for some other reason would be rewritten too, which is the risk this takes for being able to ask the front end for a slit at all.
HEIGHT_FULL = 3072
height = None
if '--height' in args:
    i = args.index('--height'); height = int(args[i + 1]); del args[i:i + 2]
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

def regeom(v):
    """Rewrite a word's height-derived halves to the requested height."""
    if height is None:
        return v
    lo, hi = v & 0xFFFF, v >> 16
    def m(h):
        if h == HEIGHT_FULL:
            return height
        if h == HEIGHT_FULL // 2:
            return height // 2
        if h == HEIGHT_FULL // 4:
            return height // 4
        return h
    return (m(hi) << 16) | m(lo)

# Walk the list: rewrite every page-aligned bus address in a (register, value) pair to a buffer slice, and every block-load source to its own slice.
bufs, load_order, geom, owners = {}, [], 0, {}
slot_of = {}
def remap(reg, v):
    """A bus address in an address register: its 16 MiB granule's slot, offset kept."""
    g = v >> 24
    if g not in slot_of:
        if reg == IMAGE_REG:
            slot_of[g] = IMAGE_SLOT
        else:
            taken = set(slot_of.values()) | {IMAGE_SLOT}
            slot_of[g] = next(k for k in range(BUF_SLOTS) if k not in taken)
    pa = BUF_PA + slot_of[g] * BUF_SLICE + (v & 0xFF_FFFF)
    bufs.setdefault(v, pa)
    owners.setdefault(pa, []).append(reg)
    return pa
# Pre-scan for the image register, so its granule claims slot 2 before anything else claims slots.
j = 0
while j < len(words):
    h = words[j]; n = h >> 14; op = h & 0x3FFF
    if op == 2:
        for k2 in range(j + 2, j + 1 + n, 2):
            if ISPFE + words[k2 - 1] == IMAGE_REG and words[k2]:
                slot_of.setdefault(words[k2] >> 24, IMAGE_SLOT)
    elif h == 0:
        break
    j += 1 + n
i = 0
while i < len(words):
    h = words[i]; n = h >> 14; op = h & 0x3FFF
    body = slice(i + 1, i + 1 + n)
    if op == 2:
        for k2 in range(i + 2, i + 1 + n, 2):
            v = words[k2]
            if ISPFE + words[k2 - 1] in ADDR_ALL and v:
                words[k2] = remap(ISPFE + words[k2 - 1], v)
            else:
                g = regeom(v)
                if g != v:
                    words[k2] = g
                    geom += 1
    elif op == 1 and n:
        # A burst writes consecutive registers from one start address; buffer addresses appear in these too.
        for k2 in range(i + 2, i + 1 + n):
            v = words[k2]
            if ISPFE + words[i + 1] + 4 * (k2 - i - 2) in ADDR_ALL and v:
                words[k2] = remap(ISPFE + words[i + 1] + 4 * (k2 - i - 2), v)
            else:
                g = regeom(v)
                if g != v:
                    words[k2] = g
                    geom += 1
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
if height is not None:
    print(f"geometry rewritten from height {HEIGHT_FULL} to {height} in {geom} words")
print(f"list {len(list_bytes)} B, {len(load_order)} block-load sources, {len(bufs)} output buffers remapped from G#{min(bufs):X}.. to G#{BUF_PA:X}.., G#{BUF_SLICE:X} a slice over {BUF_SLOTS} slices")
print(f"ring {RING_ENTRIES} entries at G#{RING_PA:X}; total {8 + len(body)} bytes -> {out}")
for pa in sorted(owners):
    print(f"  slice G#{pa:X}: " + " ".join(f"G#{r:X}" for r in owners[pa]))
