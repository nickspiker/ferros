#!/usr/bin/env python3
"""Generate ferros_kernel/src/camera_close.rs: every CPU write Linux makes to the ISP front end from the "camera close" mark to the end of a capture, in order (target/hv/ispfe-full-raw.log, 2026-10-19).

That is Linux's teardown: the PDMA's last index, the PHY lanes off, the local channels disabled (G#1C32_00A4 = G#80 then 1), the processing blocks cleared, the PDMA stopped, all twelve CSI links and the core reset. ferros replays it at the end of `diag cam pdma` so the next run in the same boot starts from the state a camera open expects.
Usage: tools/camera-close.py [capture.log]
"""
import re, sys
from pathlib import Path
ROOT = Path(__file__).resolve().parent.parent
src = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "target/hv/ispfe-full-raw.log"
writes, on = [], False
for l in src.read_text(errors="replace").splitlines():
    if "ferros-mark: camera close" in l:
        on = True
    m = re.search(r"S2 ([0-9A-F]{16}) W(\d) ([0-9A-F]{16})", l)
    if on and m:
        writes.append((int(m[1], 16), int(m[3], 16)))
out = ["//! Linux's camera close, every CPU write to the ISP front end in order, from the oracle capture (tools/camera-close.py; do not edit by hand).", "", f"pub const CLOSE: [(usize, u32); {len(writes)}] = ["]
out += [f"    (0x{a:X}, 0x{v:X})," for a, v in writes]
out.append("];")
(ROOT / "ferros_kernel/src/camera_close.rs").write_text("\n".join(out) + "\n")
print(f"{len(writes)} writes")
