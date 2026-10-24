#!/usr/bin/env python3
"""The panel's ODT, VSF RGB → panel linear RGB, and its equal-energy (E) gains, from measured spectra, all in VSF RGB.

VSF RGB is the connection space and its white is E, the flat spectrum, at R = G = B = 1. The observer is Stockman & Sharpe (2000) 10°, which vsf stores as cone fundamentals; turned into VSF RGB's own colour-matching functions (vsf's LMS2VSF_RGB applied per wavelength), they give each spectrum's VSF RGB directly, with no other space in between. This integrates each primary's measured spectrum (spotread, 380-730 nm in 10 nm steps, the profile's `colours` readings) against them, which gives the panel's three primaries in VSF RGB, and from those:
  - the gains g (largest = 1) that make R + G + B equal E, in linear light, and the multipliers on the 10-bit code (g^(1/γ), γ = 2.19, Q15);
  - how much light E white keeps against the panel's native white (photopic, vsf's LMS2PHOTOPIC weights carried into VSF RGB), which scales the peak the roll-off lands on;
  - the whole ODT: the primaries' matrix inverted, scaled so the largest row sums to 1. Absolute (no adaptation: E in is E on the glass), so its row sums are the gains; the gains are the matrix's grey axis, and the off-diagonals are what a gains-only present stage leaves out. Also the panel's primaries in VSF RGB, E-scaled, where a negative or a channel above 1 is outside the panel's reach.
The 1931 answer for the gains (from the readings' XYZ) is printed beside them, for comparison only.

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

src = open("/mnt/Harbor/Code/vsf/src/colour/spectral/constants.rs").read()
def floats(body):
    return [float(x.replace("_", "").replace("f32", "")) for x in re.findall(r"-?[0-9][0-9_.]*(?:e-?[0-9]+)?_?f32", body)]
def matrix(name):
    # vsf's matrices are column-major; m[row][col].
    v = floats(re.search(rf"pub const {name}: \[f32; 9\] = \[(.*?)\];", src, re.S)[1])
    return [[v[3 * c + r] for c in range(3)] for r in range(3)]
def mul(a, v):
    return [sum(a[r][k] * v[k] for k in range(3)) for r in range(3)]

# VSF RGB's colour-matching functions, 390-830 nm at 1 nm: vsf's cone fundamentals (LMS_2000_10DEG_1NM_DATA, interleaved L M S) through LMS2VSF_RGB.
nums = floats(re.search(r"LMS_2000_10DEG_1NM_DATA[^=]*=\s*\[(.*?)\];", src, re.S)[1])
to_vsf = matrix("LMS2VSF_RGB")
cmf = {390 + i: mul(to_vsf, nums[3 * i:3 * i + 3]) for i in range(len(nums) // 3)}
# Photopic luminance as weights on VSF RGB: LMS2PHOTOPIC · VSF_RGB2LMS.
w = re.search(r"LMS2PHOTOPIC: \[f32; 3\] = \[(.*?)\]", src, re.S)[1]
photopic_lms = [float(x.replace("_", "").replace("f32", "")) for x in re.findall(r"-?[0-9][0-9_.]*(?:_?f32)?", w)][:3]
from_vsf = matrix("VSF_RGB2LMS")
photopic = [sum(photopic_lms[k] * from_vsf[k][c] for k in range(3)) for c in range(3)]

def vsf_rgb(label):
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

def inv(m):
    cols = [solve(m, [1.0 if i == j else 0.0 for i in range(3)]) for j in range(3)]
    return [[cols[j][i] for j in range(3)] for i in range(3)]

# The panel's primaries in VSF RGB, one per column.
P = [vsf_rgb(c) for c in ("red", "green", "blue")]
M = [[P[c][r] for c in range(3)] for r in range(3)]
g = solve(M, [1.0, 1.0, 1.0])
g = [x / max(g) for x in g]
lum = lambda v: sum(photopic[c] * v[c] for c in range(3))
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

O = inv(M)
top = max(sum(row) for row in O)
O = [[x / top for x in row] for row in O]
print("ODT, VSF RGB -> panel linear RGB (rows: panel R, G, B; columns: VSF R, G, B; row sums = E's gains):")
for row in O:
    print("  " + "  ".join(f"{x:+.4f}" for x in row) + f"   sum {sum(row):.4f}")
print("the panel's primaries in VSF RGB (columns: panel R, G, B at full code, E-scaled):")
for row in inv(O):
    print("  " + "  ".join(f"{x:+.4f}" for x in row))
print("photopic luminance on VSF RGB (sums to 1): " + " ".join(f"{x:.6f}" for x in photopic))
print("for payloads/display (ODT rows, LUM):")
print("  [" + ", ".join("[" + ", ".join(f"{x:.6f}" for x in row) + "]" for row in O) + "]")
print("  [" + ", ".join(f"{x:.6f}" for x in photopic) + "]")
