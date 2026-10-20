#!/usr/bin/env python3
"""Decode a register-oracle capture of an Exynos (s3c64xx-family) SPI controller into SPI transactions.

Input: pstore lines "ferros hv: S2 <addr> <R|W><size> <value>" and "ferros hv: S2 rep <n>" (the previous access repeated n more times), plus ferros-mark lines, passed through.
A transaction runs from slave-select active (SLAVE_SEL written with bit0 clear) to inactive (bit0 set).
Inside it, consecutive TX_DATA writes form one "W" segment and RX_DATA reads one "R" segment, which is how the controller runs in CPU mode (a write-only transfer has RX off, a read-only one TX off).
Word-size transfers (MODE_CFG CH_TSZ = word) carry four bytes per access: in wire order when SWAP_CFG swaps bytes and halfwords, most significant byte first otherwise.
"""
import re, sys

BASE_MASK = ~0xFFF
OFF_MODE, OFF_SS, OFF_TX, OFF_RX, OFF_SWAP = 0x08, 0x0C, 0x18, 0x1C, 0x28

def accesses(lines):
    prev = None
    for ln in lines:
        if 'ferros-mark' in ln:
            yield ('mark', ln.split('ferros-mark:', 1)[1].strip())
            continue
        m = re.search(r'S2 rep ([0-9A-Fa-f]+)', ln)
        if m and prev:
            for _ in range(int(m.group(1), 16)):
                yield prev
            continue
        m = re.search(r'S2 ([0-9A-Fa-f]{16}) ([RW])(\d) ([0-9A-Fa-f]{16})', ln)
        if m:
            prev = ('acc', int(m.group(1), 16), m.group(2), int(m.group(3)), int(m.group(4), 16))
            yield prev

def main():
    word = False
    swap = 0
    active = False
    segs = []
    n = 0
    def tobytes(v, size):
        if size == 4 and word:
            return v.to_bytes(4, 'little' if (swap & 0x0C) == 0x0C else 'big')
        return bytes([v & 0xFF])
    def add(d, b):
        if segs and segs[-1][0] == d:
            segs[-1][1].extend(b)
        else:
            segs.append([d, bytearray(b)])
    def flush():
        nonlocal n
        if segs:
            n += 1
            print(f"T{n:04d} " + " | ".join(f"{d} {s.hex(' ')}" for d, s in segs))
            segs.clear()
    for a in accesses(sys.stdin):
        if a[0] == 'mark':
            flush(); print(f"MARK {a[1]}"); continue
        _, addr, rw, size, v = a
        off = addr & 0xFFF
        if rw == 'W' and off == OFF_MODE:
            word = ((v >> 29) & 3) == 2
        elif rw == 'W' and off == OFF_SWAP:
            swap = v
        elif rw == 'W' and off == OFF_SS:
            if v & 1:
                if active: flush()
                active = False
            else:
                active = True
        elif rw == 'W' and off == OFF_TX:
            add('W', tobytes(v, size))
        elif rw == 'R' and off == OFF_RX:
            add('R', tobytes(v, size))
    flush()

main()
