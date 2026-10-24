#!/usr/bin/env python3
"""Decode a USF capture: the AoC sensor service's messages as the kernel's kprobes saw them (SENSORS.md "Capturing USF").

Input: an ftrace dump with `usf_tx`/`usf_rx` kprobe events whose fields a/b/c/d are x32[64] arrays (1 KiB of the message). Each message is [u32 channel][a FlatBuffer]; the FlatBuffer is decoded without a schema: every table field is shown as its raw 32-bit word, and a word that is a plausible offset to a table, vector or string is followed.

Usage: tools/usf-decode.py capture.txt [--raw] [--only tx|rx] [--skip-streams]
"""
import re
import struct
import sys

LINE = re.compile(r"^\s*(\S+)-(\d+)\s+\[(\d+)\]\s+\S+\s+([\d.]+):\s+(usf_tx|usf_rx):.*?len=(0x[0-9a-f]+)\s+a=\{([^}]*)\}\s+b=\{([^}]*)\}\s+c=\{([^}]*)\}\s+d=\{([^}]*)\}")


def words(s):
    return [int(w, 16) for w in s.split(",") if w]


def parse(path):
    for line in open(path, errors="replace"):
        m = LINE.match(line)
        if not m:
            continue
        task, pid, cpu, ts, kind, ln = m.group(1), m.group(2), m.group(3), float(m.group(4)), m.group(5), int(m.group(6), 16)
        w = words(m.group(7)) + words(m.group(8)) + words(m.group(9)) + words(m.group(10))
        b = struct.pack("<%dI" % len(w), *w)[: min(ln, 1024)]
        yield dict(task=task, pid=pid, ts=ts, kind=kind, len=ln, data=b)


def u32(b, o):
    return struct.unpack_from("<I", b, o)[0] if o + 4 <= len(b) else None


def i32(b, o):
    return struct.unpack_from("<i", b, o)[0] if o + 4 <= len(b) else None


def u16(b, o):
    return struct.unpack_from("<H", b, o)[0] if o + 2 <= len(b) else None


def as_float(v):
    f = struct.unpack("<f", struct.pack("<I", v))[0]
    if v != 0 and 1e-6 < abs(f) < 1e7:
        return "%.6g" % f
    return None


def looks_table(b, t):
    so = i32(b, t)
    if so is None:
        return False
    vt = t - so
    if vt < 0 or vt + 4 > len(b) or vt % 2:
        return False
    vsz, osz = u16(b, vt), u16(b, vt + 2)
    return vsz is not None and 4 <= vsz <= 128 and vsz % 2 == 0 and 4 <= osz <= 1024 and vt + vsz <= len(b) and t + osz <= len(b) + 64


def show_vector(b, at, depth, out, seen):
    n = u32(b, at)
    body = b[at + 4 : at + 4 + n] if n is not None else b""
    if n is not None and 0 < n < 512 and all(32 <= c < 127 for c in body) and at + 4 + n < len(b) and b[at + 4 + n] == 0:
        out.append("  " * depth + 'string "%s"' % body.decode())
        return
    out.append("  " * depth + "vector n=%s: %s" % (n, body[:48].hex()))
    # A [ubyte] vector that is itself a FlatBuffer (USF nests its payload this way).
    if n and n >= 12 and n <= len(b):
        root = u32(body, 0)
        if root is not None and root < n and looks_table(body, root):
            out.append("  " * depth + "  nested:")
            table(body, root, depth + 2, out, set())


def table(b, t, depth, out, seen):
    if t in seen or depth > 12:
        return
    seen.add(t)
    vt = t - i32(b, t)
    vsz = u16(b, vt)
    nfields = (vsz - 4) // 2
    for i in range(nfields):
        fo = u16(b, vt + 4 + 2 * i)
        if not fo:
            continue
        at = t + fo
        v = u32(b, at)
        if v is None:
            out.append("  " * depth + "f%d @+%d: (past capture)" % (i, fo))
            continue
        tgt = at + v
        desc = "f%d @+%d: G#%X (%d)" % (i, fo, v, v)
        fl = as_float(v)
        if fl:
            desc += " f32 %s" % fl
        out.append("  " * depth + desc)
        if 4 <= v < 4096 and tgt < len(b):
            if looks_table(b, tgt):
                out.append("  " * depth + "  -> table @%d" % tgt)
                table(b, tgt, depth + 2, out, seen)
            elif u32(b, tgt) is not None and u32(b, tgt) < len(b) - tgt:
                show_vector(b, tgt, depth + 1, out, seen)


def decode(m):
    b = m["data"]
    ch = u32(b, 0)
    fb = b[4:]
    out = []
    root = u32(fb, 0)
    if root is not None and root < len(fb) and looks_table(fb, root):
        table(fb, root, 1, out, set())
    else:
        out.append("  (not a FlatBuffer at the root) " + fb[:64].hex())
    return ch, out


def main():
    args = sys.argv[1:]
    raw = "--raw" in args
    only = None
    if "--only" in args:
        only = args[args.index("--only") + 1]
    skip = "--skip-streams" in args
    path = [a for a in args if not a.startswith("--") and a not in ("tx", "rx")][0]
    t0 = None
    for m in parse(path):
        if only and m["kind"] != "usf_" + only:
            continue
        ch, out = decode(m)
        if skip and m["kind"] == "usf_rx" and ch & 0x80000000:
            continue
        t0 = t0 if t0 is not None else m["ts"]
        print("%9.3f %s %-22s ch G#%X len %d" % (m["ts"] - t0, "AP->AoC" if m["kind"] == "usf_tx" else "AoC->AP", m["task"], ch, m["len"]))
        if raw:
            print("  " + m["data"].hex())
        for l in out:
            print(l)


if __name__ == "__main__":
    main()
