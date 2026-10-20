#!/usr/bin/env python3
"""Decode the ISP front end's PDMA register-write list from the hv's dump (FERROS_HV_PDMA_DUMP: "PDMA buf +<off> w0..w7" lines).

Format, worked out from the dump of 2026-10-20 (every header lands where the previous record ends):
- header word: count = w >> 14 (the number of words that follow), opcode = w & G#3FFF.
- opcode 2: count/2 (address, value) pairs.
- opcode 1: a burst: the first word is the start address, the rest are values for consecutive registers (+4 each).
- opcode 9: 4 words {IOVA, 0, length, register}: a block load from memory (not expanded here).
- opcode G#B: a marker word (the list starts with one).
Addresses are offsets; --base (default G#1C30_0000, the ISPFE block per zuma-isp.dtsi lwis_isp_fe, which also holds the PDMA at +G#9_0000) is added when printing.
"""
import re, sys
base = 0x1C30_0000
args = [a for a in sys.argv[1:]]
if '--base' in args:
    i = args.index('--base'); base = int(args[i + 1].replace('G#', '').replace('_', ''), 16); del args[i:i + 2]
words = []
src = open(args[0], errors='replace') if args else sys.stdin
for ln in src:
    m = re.search(r'PDMA buf \+([0-9A-F]+)((?: [0-9A-F]{8}){8})', ln)
    if m:
        words += [int(x, 16) for x in m.group(2).split()]
i = 0
writes = []
while i < len(words):
    h = words[i]; n = h >> 14; op = h & 0x3FFF
    body = words[i + 1:i + 1 + n]
    if op == 2:
        for k in range(0, len(body) - 1, 2):
            writes.append((body[k], body[k + 1]))
    elif op == 1 and body:
        a = body[0]
        for k, v in enumerate(body[1:]):
            writes.append((a + 4 * k, v))
    elif op == 9 and len(body) == 4:
        print(f"LOAD G#{body[2]:X} bytes from IOVA G#{body[0]:X} to G#{base + body[3]:X}")
    elif op == 0xB:
        pass
    elif h == 0 and all(w == 0 for w in words[i:i + 8]):
        break
    else:
        print(f"? word {i}: G#{h:08X} (count {n}, op G#{op:X})")
    i += 1 + n
for a, v in writes:
    print(f"W G#{base + a:X} = G#{v:X}")
print(f"({len(writes)} register writes, {i} of {len(words)} words parsed)", file=sys.stderr)
