#!/usr/bin/env python3
"""oracle-decode-hsi2c: turn an hv oracle capture of an Exynos USI HS-I2C controller (auto mode) back into I2C transactions.

Input: the flushed console (console-ramoops-0), or just its `ferros hv: S2 <pa> <R|W><bytes> <value>` lines (HV.md, the register oracle). `ferros hv: S2 rep <hex N>` means the previous access repeated N more times (the hv collapses status polls); markers written from Android with `echo "<3>ferros-mark: ..." > /dev/kmsg` are passed through as `# MARK ...` lines in order.
Output: one line per transfer, in order: `<target> W <hex bytes>` or `<target> R <hex bytes>`, the I2C address in G# form.
Register map as in ferros_kernel/src/hsi2c.rs: TX_DATA G#34, RX_DATA G#38, AUTO_CONF G#44 (len [15:0], READ bit 16, STOP bit 17, RUN bit 31), ADDR G#70 (7-bit target at bits [16:10]).
Usage: tools/oracle-decode-hsi2c.py <capture.log> [controller base, default G#1088_0000] [--max-bytes N]
--max-bytes N prints a transfer longer than N bytes as its first bytes plus `...[len, blake2b-64]`: vendor firmware uploaded over the bus (a sensor's settings file) stays out of public captures while its size and identity stay checkable.
"""
import hashlib, re, sys

args = [a for a in sys.argv[1:]]
maxb = None
if "--max-bytes" in args:
    i = args.index("--max-bytes")
    maxb = int(args[i + 1])
    del args[i:i + 2]
path = args[0]
base = int(args[1].replace("G#", "").replace("_", ""), 16) if len(args) > 1 else 0x1088_0000
pat = re.compile(r"S2 ([0-9A-Fa-f]{16}) ([RW])(\d) ([0-9A-Fa-f]{16})")
rep_pat = re.compile(r"S2 rep ([0-9A-Fa-f]{16})")
mark_pat = re.compile(r"ferros-mark: (.*)")

def g(v, w=2):
    return "G#" + format(v, "0%dX" % w)

target, tx, rx, kind, out = None, [], [], None, []
def flush():
    global tx, rx, kind
    if kind is not None and target is not None:
        data = tx if kind == "W" else rx
        if maxb is not None and len(data) > maxb:
            h = hashlib.blake2b(bytes(data), digest_size=8).hexdigest().upper()
            text = " ".join("%02X" % b for b in data[:4]) + " ...[%d bytes, blake2b-64 %s]" % (len(data), h)
        else:
            text = " ".join("%02X" % b for b in data)
        out.append("%s %s %s" % (g(target), kind, text))
    tx, rx, kind = [], [], None

def accesses():
    """Yield (pa, dir, val) in order with repeat runs expanded, and ("MARK", text, None) for markers."""
    prev = None
    for line in open(path, errors="replace"):
        mk = mark_pat.search(line)
        if mk:
            yield ("MARK", mk.group(1).strip(), None)
            continue
        r = rep_pat.search(line)
        if r:
            if prev:
                for _ in range(int(r.group(1), 16)):
                    yield prev
            continue
        m = pat.search(line)
        if m:
            prev = (int(m.group(1), 16), m.group(2), int(m.group(4), 16))
            yield prev

for pa, d, val in accesses():
    if pa == "MARK":
        flush()
        out.append("# MARK " + d)
        continue
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
xfers = [l for l in out if not l.startswith("#")]
print("# %d transfers; targets: %s" % (len(xfers), ", ".join(sorted({l.split()[0] for l in xfers}))), file=sys.stderr)
