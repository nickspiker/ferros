#!/usr/bin/env python3
"""The panel's channel gains for equal-energy white (E) under VSF's observer, from measured spectra.

VSF's white is E, the flat spectrum. Under the Stockman & Sharpe (2000) 10° cone fundamentals as vsf carries them (each channel normalised to sum to 1), a flat spectrum gives L = M = S, so "the panel shows E" means: the mix of its three primaries gives equal L, M and S. This integrates each primary's measured spectrum (spotread, 380-730 nm in 10 nm steps, the profile's `colours` readings) against the cone fundamentals, solves for the gains g (largest = 1) that make R + G + B equal in L, M and S, and reports what the present stage needs:
  - the gains in linear light, and the multipliers on the 10-bit code (g^(1/γ), γ = 2.19, Q15);
  - how much light E white keeps against the panel's native white (photopic, vsf's LMS2PHOTOPIC weights), which scales the peak the roll-off lands on.
The 1931 answer (from the readings' XYZ) is printed beside it, for comparison only.

Usage: tools/panel-white-e.py <profile dir> [dbv, default 1353]
"""
import re, sys

d = sys.argv[1]
dbv = sys.argv[2] if len(sys.argv) > 2 else "1353"
log = open(f"{d}/spotread.log", errors="replace").read()

def spectrum(label):
    m = re.search(rf"=== colours {label} dbv={dbv} .*?Spectrum from ([0-9.]+) to ([0-9.]+) nm in (\d+) steps\n([^\n]+)\n.*?Result is XYZ: ([-0-9.e]+) ([-0-9.e]+) ([-0-9.e]+)", log, re.S)
    if not m:
        sys.exit(f"no spectrum for {label} at DBV {dbv}")
    lo, hi, n = float(m[1]), float(m[2]), int(m[3])
    vals = [float(v) for v in m[4].split(",")]
    return lo, (hi - lo) / (n - 1), vals, (float(m[5]), float(m[6]), float(m[7]))

# vsf's cone fundamentals: LMS_2000_10DEG_1NM_DATA, 390-830 nm at 1 nm, interleaved L M S.
src = open("/mnt/Harbor/Code/vsf/src/colour/spectral/constants.rs").read()
block = re.search(r"LMS_2000_10DEG_1NM_DATA[^=]*=\s*\[(.*?)\];", src, re.S)[1]
nums = [float(x.replace("_", "").replace("f32", "")) for x in re.findall(r"-?[0-9][0-9_.]*(?:e-?[0-9]+)?_?f32", block)]
cmf = {390 + i: nums[3 * i:3 * i + 3] for i in range(len(nums) // 3)}
w = re.search(r"LMS2PHOTOPIC: \[f32; 3\] = \[(.*?)\]", src, re.S)[1]
photopic = [float(x.replace("_", "").replace("f32", "")) for x in re.findall(r"-?[0-9][0-9_.]*(?:_?f32)?", w)][:3]

def lms(label):
    lo, step, vals, _ = spectrum(label)
    acc = [0.0, 0.0, 0.0]
    for nm in range(390, 731):
        # The 10 nm reading, linearly interpolated to 1 nm.
        t = (nm - lo) / step
        i = min(int(t), len(vals) - 2)
        v = vals[i] + (vals[i + 1] - vals[i]) * (t - i)
        for c in range(3):
            acc[c] += v * cmf[nm][c]
    return acc

def solve(m, rhs):
    # 3x3 by Cramer's rule; m[row][col].
    def det(a):
        return a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
    D = det(m)
    out = []
    for c in range(3):
        a = [row[:] for row in m]
        for r in range(3):
            a[r][c] = rhs[r]
        out.append(det(a) / D)
    return out

P = [lms(c) for c in ("red", "green", "blue")]
M = [[P[c][r] for c in range(3)] for r in range(3)]
g = solve(M, [1.0, 1.0, 1.0])
g = [x / max(g) for x in g]
lum = lambda v: photopic[0] * v[0] + photopic[1] * v[1] + photopic[2] * v[2]
native = lum([sum(P[c][r] for c in range(3)) for r in range(3)])
e = lum([sum(g[c] * P[c][r] for c in range(3)) for r in range(3)])
gamma = 2.19
print(f"E white under Stockman-Sharpe 10° (DBV {dbv}): gains R {g[0]:.4f} G {g[1]:.4f} B {g[2]:.4f}")
print(f"  code multipliers g^(1/{gamma}): " + " ".join(f"{x ** (1 / gamma):.4f} (Q15 {round(x ** (1 / gamma) * 32768)})" for x in g))
print(f"  E white keeps {100 * e / native:.1f} % of native white's light (photopic)")
X = [spectrum(c)[3] for c in ("red", "green", "blue")]
M31 = [[X[c][r] for c in range(3)] for r in range(3)]
g31 = solve(M31, [1.0, 1.0, 1.0])
g31 = [x / max(g31) for x in g31]
print(f"for comparison, E under CIE 1931 2° (X = Y = Z): R {g31[0]:.4f} G {g31[1]:.4f} B {g31[2]:.4f}")
