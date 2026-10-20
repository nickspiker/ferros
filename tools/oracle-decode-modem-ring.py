#!/usr/bin/env python3
"""Decode the hv's modem boot-channel trace (FERROS_HV_MODEM_RING) into the boot protocol.

Input: pstore lines "ferros hv: RING T|R <len> <old> <hex>" (T = a frame the AP put in the RAW TX ring, R = frames it consumed from the RAW RX ring; hex = the first 48 / 128 bytes).
Each frame is a 12-byte SIT header (G#ABCD, seq, cfg, length, channel G#F1, channel seq, 2 spare) and a payload whose first u16 is the command; boot-download data frames (G#A12B) carry {u16 cmd, u16 chunk+8, u32 total, u32 offset, data}.
Runs of data frames are summarised (count, total, offsets) instead of printed; with --data-head, each run's first 16 data bytes are shown so the chunk can be found in an image file.
Raw traces stay local (the boot channel can carry per-unit NV data); this summary prints no payload beyond command words, sizes and offsets unless asked.
"""
import re, sys
show_head = '--data-head' in sys.argv
frames = []
for ln in sys.stdin:
    m = re.search(r'RING ([TR]) ([0-9A-F]+) ([0-9A-F]+) ([0-9A-F]*)', ln)
    if m:
        frames.append((m.group(1), int(m.group(2), 16), bytes.fromhex(m.group(4))))
def u16(b, o): return b[o] | b[o+1] << 8
def u32(b, o): return u16(b, o) | u16(b, o+2) << 16
run = None
def flush():
    global run
    if run:
        d, cmd, total, first, last, n, head = run
        extra = f" first data {head.hex()}" if show_head else ""
        print(f"{d} G#{cmd:04X} data x{n}: total G#{total:X}, offsets G#{first:X}..G#{last:X}{extra}")
        run = None
acks = 0
for d, ln, b in frames:
    # A frame consumed from RX may be several CP frames back to back.
    pos = 0
    while pos + 12 <= len(b):
        if u16(b, pos) != 0xABCD:
            print(f"{d} (no SIT header at +{pos}): {b[pos:pos+16].hex()}"); break
        flen = u16(b, pos + 6)
        p = b[pos + 12: pos + max(flen, 12)]
        cmd = u16(p, 0) if len(p) >= 2 else -1
        if cmd & 0xF000 == 0xA000 and len(p) >= 12 and u16(p, 2) > 8 and flen > 64:
            total, off = u32(p, 4), u32(p, 8)
            if run and run[0] == d and run[1] == cmd and run[2] == total:
                run = (d, cmd, total, run[3], off, run[5] + 1, run[6])
            else:
                flush(); run = (d, cmd, total, off, off, 1, p[12:28])
        elif run and d == 'R' and cmd == (run[1] & 0x0FFF | 0xC000) and len(p) <= 4:
            acks += 1
        else:
            flush()
            print(f"{d} G#{cmd & 0xFFFF:04X} len {flen} payload {p[2:min(len(p), 2+32)].hex()}")
        pos += max(flen, 12)
        pos = (pos + 3) & ~3
        if d == 'T': break
flush()
print(f"({acks} data acks folded into the runs)")
