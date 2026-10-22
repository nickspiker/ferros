#!/usr/bin/env python3
"""Summarise a display profile run (tools/display-profile.sh): the roll-off of a small window against full field at every brightness, sustained white over time, the limiter's response to average picture level, the tone curve per channel at each brightness (gamma fitted over codes 16-240, with the deviation of each step), near-black, primaries and secondaries (xy, Y, and R+G+B against white), and drift.

Usage: tools/display-profile-report.py <profile dir> [--md out.md]
"""
import csv, math, sys
from collections import defaultdict

args = sys.argv[1:]
md_out = None
if "--md" in args:
    i = args.index("--md"); md_out = args[i + 1]; del args[i:i + 2]
d = args[0]
rows = [r for r in csv.DictReader(open(f"{d}/readings.csv")) if r["Y_cdm2"] not in ("?", "")]
for r in rows:
    r["Y"] = float(r["Y_cdm2"]); r["x_"] = float(r["x"]); r["y_"] = float(r["y"])
out = []
p = out.append
def sec(name): return [r for r in rows if r["section"] == name]

p("## Window against full field (white)\n")
p("| DBV | 1 cm | 2 cm | 4 cm | full | full ÷ 1 cm |")
p("|---|---|---|---|---|---|")
by = defaultdict(dict)
for r in sec("rolloff"):
    by[r["dbv"]][int(r["window_px"])] = r["Y"]
for dbv, w in by.items():
    ks = sorted(k for k in w if k)
    small = w[ks[0]] if ks else None
    cells = [f"{w[k]:.1f}" for k in ks] + [f"{w.get(0, float('nan')):.1f}"]
    ratio = f"{w[0] / small:.3f}" if small and 0 in w else "-"
    p(f"| {dbv} | " + " | ".join(cells) + f" | {ratio} |")

p("\n## Full white held\n")
p("| DBV | first | last | change | readings |")
p("|---|---|---|---|---|")
sus = defaultdict(list)
for r in sec("sustain"):
    sus[r["dbv"]].append(r["Y"])
for dbv, ys in sus.items():
    p(f"| {dbv} | {ys[0]:.1f} | {ys[-1]:.1f} | {100 * (ys[-1] / ys[0] - 1):+.1f} % | {' '.join(f'{y:.0f}' for y in ys)} |")

p("\n## The 1 cm white disc against its surround (average picture level)\n")
p("| DBV | surround 00 | 40 | 80 | C0 | FF |")
p("|---|---|---|---|---|---|")
apl = defaultdict(dict)
for r in sec("apl"):
    apl[r["dbv"]][r["surround"]] = r["Y"]
for dbv, s in apl.items():
    p(f"| {dbv} | " + " | ".join(f"{s.get(k, float('nan')):.1f}" for k in ["000000", "404040", "808080", "c0c0c0", "ffffff"]) + " |")

p("\n## Tone curves (1 cm window)\n")
ramps = defaultdict(dict)
for r in sec("ramps"):
    ch = "".join(c for c in r["label"] if c.isalpha()); code = int("".join(c for c in r["label"] if c.isdigit()))
    ramps[(r["dbv"], ch)][code] = r["Y"]
p("| DBV | channel | Y at 255 | black | gamma (16-240) | worst step off the fit |")
p("|---|---|---|---|---|---|")
for (dbv, ch), m in sorted(ramps.items()):
    top, blk = m.get(255), m.get(0, 0.0)
    if not top:
        continue
    pts = [(c / 255, (y - blk) / (top - blk)) for c, y in m.items() if 16 <= c <= 240 and y > blk]
    # Least squares on log Y/Ytop = g log c.
    num = sum(math.log(v) * math.log(u) for u, v in pts); den = sum(math.log(u) ** 2 for u, v in pts)
    g = num / den if den else float("nan")
    worst = max(((abs(v / u ** g - 1), c) for (u, v), c in zip(pts, [round(u * 255) for u, _ in pts])), default=(0, 0))
    p(f"| {dbv} | {ch} | {top:.2f} | {blk:.4f} | {g:.3f} | {100 * worst[0]:.1f} % at {worst[1]} |")

p("\n## Near black (DBV 1353)\n")
p("| code | Y cd/m² | against γ 2.185 from white |")
p("|---|---|---|")
w1353 = ramps.get(("1353", "grey"), {}).get(255)
for r in sec("nearblack"):
    c = int("".join(ch for ch in r["label"] if ch.isdigit()))
    exp = w1353 * (c / 255) ** 2.185 if w1353 else float("nan")
    p(f"| {c} | {r['Y']:.4f} | {100 * (r['Y'] / exp - 1):+.0f} % |")

p("\n## Colours (1 cm window)\n")
p("| DBV | patch | Y | x | y |")
p("|---|---|---|---|---|")
col = defaultdict(dict)
for r in sec("colours"):
    col[r["dbv"]][r["label"]] = r
    p(f"| {r['dbv']} | {r['label']} | {r['Y']:.2f} | {r['x_']:.4f} | {r['y_']:.4f} |")
for dbv, c in col.items():
    if all(k in c for k in ("red", "green", "blue", "white")):
        s = c["red"]["Y"] + c["green"]["Y"] + c["blue"]["Y"]
        p(f"\nAt DBV {dbv}: R+G+B = {s:.1f} against white {c['white']['Y']:.1f} ({100 * (s / c['white']['Y'] - 1):+.1f} %).")

dr = sec("drift")
if len(dr) >= 2:
    p(f"\n## Drift\n\nWhite at 1353, first {dr[0]['Y']:.2f} at t={dr[0]['t_s']} s, last {dr[-1]['Y']:.2f} at t={dr[-1]['t_s']} s ({100 * (dr[-1]['Y'] / dr[0]['Y'] - 1):+.2f} %).")

text = "\n".join(out)
print(text)
if md_out:
    open(md_out, "w").write(text + "\n")
