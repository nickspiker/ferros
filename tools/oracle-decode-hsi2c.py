#!/usr/bin/env python3
"""oracle-decode-hsi2c: turn an hv oracle capture of an Exynos USI HS-I2C controller (auto mode) back into I2C transactions.

Input: lines of `ferros hv: S2 <pa> <R|W><bytes> <value>` as pulled from console-ramoops-0 (HV.md, the register oracle).
Output: one line per transfer, in order: `<target> W <hex bytes>` or `<target> R <hex bytes>`, the I2C address in G# form.
Register map as in ferros_kernel/src/hsi2c.rs: TX_DATA G#34, RX_DATA G#38, AUTO_CONF G#44 (len [15:0], READ bit 16, STOP bit 17, RUN bit 31), ADDR G#70 (7-bit target at bits [16:10]).
Usage: tools/oracle-decode-hsi2c.py <capture.log> [controller base, default G#1088_0000]
"""
import re, sys

path = sys.argv[1]
base = int(sys.argv[2].replace("G#", "").replace("_", ""), 16) if len(sys.argv) > 2 else 0x1088_0000
pat = re.compile(r"S2 ([0-9A-Fa-f]{16}) ([RW])(\d) ([0-9A-Fa-f]{16})")

def g(v, w=2):
    return "G#" + format(v, "0%dX" % w)

target, tx, rx, kind, out = None, [], [], None, []
def flush():
    global tx, rx, kind
    if kind is not None and target is not None:
        data = tx if kind == "W" else rx
        out.append("%s %s %s" % (g(target), kind, " ".join("%02X" % b for b in data)))
    tx, rx, kind = [], [], None

for line in open(path, errors="replace"):
    m = pat.search(line)
    if not m:
        continue
    pa, d, _, val = int(m.group(1), 16), m.group(2), m.group(3), int(m.group(4), 16)
    if pa & ~0xFFF != base:
        continue
    off = pa & 0xFFF
    if off == 0x70 and d == "W":
        flush()
        target = (val >> 10) & 0x7F
    elif off == 0x44 and d == "W" and val & (1 << 31):
        if kind is not None:
            flush()
        kind = "R" if val & (1 << 16) else "W"
    elif off == 0x34 and d == "W":
        tx.append(val & 0xFF)
    elif off == 0x38 and d == "R":
        rx.append(val & 0xFF)
flush()
print("\n".join(out))
print("# %d transfers; targets: %s" % (len(out), ", ".join(sorted({l.split()[0] for l in out}))), file=sys.stderr)
