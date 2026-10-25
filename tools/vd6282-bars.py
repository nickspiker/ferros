#!/usr/bin/env python3
"""Live bar graph of the rear spectral sensor (VD6282) in the terminal, for pointing laser lines at it.

Polls the newest sample over the bridge (~3 per second) and redraws one bar per channel, log scale, with its raw counts, its gain and counts per unit gain (comparable across the auto-gain's changes). Every new sample also goes to a CSV (as tools/vd6282-log.sh writes). Ctrl-C to stop.
Usage: tools/vd6282-bars.py [out.csv]
"""
import math
import os
import re
import subprocess
import sys
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
B = os.path.join(REPO, "target/debug/ferros-bridge")
CEIL = 16777215.0
WIDTH = 60


def bridge(cmd):
    try:
        return subprocess.run([B, "diag", cmd], capture_output=True, text=True, timeout=10).stdout
    except subprocess.TimeoutExpired:
        return ""


def sample():
    out = bridge("usf ev")
    m = re.search(r"^type 12:.*\n\s*ts G#([0-9A-F]+):(.*)$", out, re.M)
    if not m:
        return None, None
    vals = [float(v) for v in re.findall(r" (-?[0-9]+\.[0-9]+) \(", m.group(2))]
    return m.group(1), vals


def bar(v, top):
    if v <= 0:
        return ""
    # Log scale from 1 count to the ADC ceiling.
    n = int(WIDTH * math.log10(max(v, 1.0)) / math.log10(top))
    return "█" * max(0, min(WIDTH, n))


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(REPO, "target/vd6282", time.strftime("%Y%m%d-%H%M%S") + ".csv")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    print(bridge("usf subh 18 200 1").strip().splitlines()[-1:] or "")
    f = open(out, "w")
    f.write("host_time,aoc_ts," + ",".join("v%d" % i for i in range(16)) + "\n")
    last = None
    peak = [0.0] * 6
    while True:
        ts, v = sample()
        if ts and ts != last and len(v) >= 12:
            last = ts
            f.write("%.3f,%s,%s\n" % (time.time(), ts, ",".join("%.4f" % x for x in v)))
            f.flush()
            counts, gains = v[:6], v[6:12]
            lines = ["\x1b[H\x1b[2J", "VD6282 rear spectral — %s   (log scale, 1 … %d counts)   csv: %s" % (time.strftime("%H:%M:%S"), CEIL, out), ""]
            for i in range(6):
                c, g = counts[i], gains[i]
                peak[i] = max(peak[i], c)
                sat = "  SATURATED" if c >= CEIL * 0.98 else ""
                lines.append("ch%d %-60s %12.1f  gain %5.1f  per gain %10.2f  peak %12.1f%s" % (i, bar(c, CEIL), c, g, c / g if g else 0.0, peak[i], sat))
            rest = v[12:]
            lines.append("")
            lines.append("other words: " + "  ".join("%.4f" % x for x in rest))
            sys.stdout.write("\n".join(lines) + "\n")
            sys.stdout.flush()
        time.sleep(0.1)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        pass
