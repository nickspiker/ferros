# Decode a captured ftrace of Photon-free DSI traffic (dpu/panel dsi_tx, dsi_rx, dsi_label_scope, te2_update_settings) into a clean command list: relative ms, packet type, bytes, delay, labels. Pure text; carries no secret.
import re, sys, os
ev = re.compile(r'^\s*\S+\s+\[(\d+)\]\s+\S+\s+(\d+\.\d+): (dsi_tx|dsi_rx|dsi_label_scope|te2_update_settings|panel_write_generic|tracing_mark_write): (.*)$')
def decode(src, dst):
    t0 = None; out = []; n_tx = 0; seen_hex = set()
    for line in open(src, errors="replace"):
        m = ev.match(line)
        if not m: continue
        cpu, ts, kind, rest = m.groups(); ts = float(ts)
        if t0 is None: t0 = ts
        ms = (ts - t0) * 1000
        if kind == "dsi_tx":
            mm = re.match(r'type=0x([0-9a-f]+) length=(\d+) last=(\d) delay=(\d+) tx=\[([0-9a-f ]*)\]', rest)
            if not mm: continue
            ty, ln, last, delay, hx = mm.groups(); n_tx += 1
            tag = "" if ty.lower() == "39" else f" type {ty}"
            out.append(f"{ms:9.3f}  tx {hx}{tag}{'  last' if last == '1' else ''}{'  wait ' + delay + ' ms' if delay != '0' else ''}")
        elif kind == "dsi_rx":
            out.append(f"{ms:9.3f}  rx {rest}")
        elif kind == "dsi_label_scope":
            out.append(f"{ms:9.3f}  -- {rest}")
        elif kind == "te2_update_settings":
            out.append(f"{ms:9.3f}  -- {rest}")
    with open(dst, "w") as f:
        f.write(f"# {n_tx} DSI packets; times in ms from the first event; bytes are the packet payload (the first byte is the DCS command); type shown when not a long write (39)\n")
        f.write("\n".join(out) + "\n")
    return n_tx, (out[-1].split()[0] if out else "0")
root = sys.argv[1]
for d in sorted(os.listdir(root)):
    p = os.path.join(root, d, "dsi-trace.txt")
    if os.path.isfile(p):
        n, span = decode(p, os.path.join(root, d, "dsi-commands.txt"))
        print(f"{d:14s} {n:5d} packets over {span} ms")
