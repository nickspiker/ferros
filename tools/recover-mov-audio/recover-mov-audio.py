#!/usr/bin/env python3
"""Recover the PCM audio tracks from a QuickTime recording whose `moov` index was never written.

The recorder (ffmpeg, driven by opsin) was killed when the disk filled, leaving `ftyp` + one 30 GB
`mdat` and no index. The media survives; only the map is gone, and because the audio is raw PCM —
no packet signatures — index-rebuilding tools (untrunc) prune those tracks as empty.

The interleave is strictly regular: [video chunk: AVCC length-prefixed h264 NALs][8192 B mono]
[16384 B stereo]. Three observations make byte-exact recovery possible without the index:

1. Content separates audio from video decisively. Compressed h264 reads as near-uniform bytes:
   as int16 it averages |x| ~16000. PCM speech averages ~200, p99 ~2000. A sliding mean over
   exactly one block's width (12288 samples) drops below threshold only where a block sits, so
   each block yields one candidate region. This finds every block (33923/33923 on the reference)
   but locates it only to within a few thousand bytes: the h264 NAL *tails* flanking a block are
   themselves quiet-ish (~4000-5800), which blurs the edge.
2. The block END is anchored exactly by what follows it: a video NAL, i.e. a 4-byte big-endian
   length whose value walks — through one or more NALs — to the next audio block. A false prefix
   inside video will not chain to the next region; the true one does. Scoring every candidate in
   a window around the estimate by how close its chain lands to the next region picks the right
   one 99.3% of the time by itself.
3. The remaining false positives are prefixes a few KB *into* the next video chunk whose fake
   length happens to chain near the next region. Every one of them fails a test the true end
   passes trivially: the bytes immediately before a real block end are the tail of the stereo
   chunk and are quiet; bytes inside video are not.

Both tracks are written from the same anchored block, so mono and stereo stay in sample-exact
mutual sync regardless of anything dropped: a dropped block removes the same 85 ms from both.

Validated on an intact recording from the same recorder by comparing every recovered block start
against the real index: 33859 of 33923 blocks byte-exact, the rest dropped (never mis-split).

Usage: recover-mov-audio.py <file.mov> <out_mono.raw> <out_stereo.raw>
       then e.g.  ffmpeg -f s16le -ar 48000 -ac 1 -i out_mono.raw mic.wav
                  ffmpeg -f s16le -ar 48000 -ac 2 -i out_stereo.raw system.wav
The chunk sizes (8192 B mono / 16384 B stereo) are this recorder's; adjust the constants for another.
"""
import os
import struct
import sys

import numpy as np

MONO_BYTES, STEREO_BYTES = 8192, 16384
BLOCK = MONO_BYTES + STEREO_BYTES
BLOCK_S = BLOCK // 2
TH = 6000.0          # block-mean threshold: video ~16000, speech p99 ~2000 (see note 1)
W = 16384            # bytes either side of the estimate to search for the block end (note 2)
TOL = 20000          # a chain must land this close to the next region's estimate
HOPS = 16            # NALs to walk through one video chunk
PRE = 2048           # bytes before a candidate end that must be quiet (note 3)
VALID = np.array([1, 2, 3, 4, 5, 6, 7, 8, 9])
CHUNK = 192 << 20
OVERLAP = 8 << 20    # > any video chunk + search window + chain, so a region and its successor share a chunk


def main():
    src, out_mono, out_stereo = sys.argv[1], sys.argv[2], sys.argv[3]
    total = os.path.getsize(src)
    f = open(src, "rb")
    off, start, end = 0, None, None
    while off < total - 8:
        f.seek(off)
        head = f.read(16)
        size = struct.unpack_from(">I", head, 0)[0]
        typ = head[4:8].decode("latin1", "replace")
        hdr = 8
        if size == 1:
            size, hdr = struct.unpack_from(">Q", head, 8)[0], 16
        if size == 0:
            size = total - off
        if typ == "mdat":
            start, end = off + hdr, min(off + size, total)
            break
        off += size
    if start is None:
        sys.exit("no mdat")
    print(f"mdat {start}..{end} ({(end-start)/2**30:.1f} GiB)", file=sys.stderr)

    mono = open(out_mono, "wb")
    stereo = open(out_stereo, "wb")
    blocks = dropped = 0
    last_start = -1
    cpos = start
    while cpos < end:
        f.seek(cpos)
        data = f.read(min(CHUNK, end - cpos))
        if len(data) < BLOCK * 2:
            break
        final = cpos + len(data) >= end
        b = np.frombuffer(data, dtype=np.uint8)
        # Blocks start on ANY byte offset (they follow variable-length video NALs), and an s16le
        # stream viewed one byte out of phase reads as noise — the high byte of one sample pairs
        # with the low byte of the next. A single int16 view therefore misses every block whose
        # start parity differs from the view's: 25% of them on the reference file. Evaluate both
        # parities and take each block from whichever view sees it.
        est = []
        for par in (0, 1):
            n = (len(data) - par) // 2 * 2
            a = np.frombuffer(data, dtype="<i2", count=n // 2, offset=par)
            mag = np.abs(a.astype(np.int32))
            cs = np.concatenate(([0], np.cumsum(mag, dtype=np.int64)))
            win = (cs[BLOCK_S:] - cs[:-BLOCK_S]) / BLOCK_S
            idx = np.flatnonzero(win < TH)
            if len(idx) == 0:
                continue
            for g in np.split(idx, np.flatnonzero(np.diff(idx) > 1) + 1):
                est.append(2 * int(g[np.argmin(win[g])]) + par)
        est.sort()
        # The same block can surface in both views (one of them a byte off); keep one per block.
        merged = []
        for e in est:
            if merged and e - merged[-1] < BLOCK:
                continue
            merged.append(e)
        est = merged
        if not est:
            cpos += len(data) - OVERLAP if not final else len(data)
            continue

        def nal_at(p):
            if p + 5 > len(b) or b[p] != 0 or b[p + 1] >= 0x80:
                return 0
            L = int(struct.unpack_from(">I", data, p)[0])
            if L < 2 or p + 4 + L > len(b):
                return 0
            h = int(b[p + 4])
            if h & 0x80 or (h & 0x1F) not in (1, 2, 3, 4, 5, 6, 7, 8, 9):
                return 0
            return 4 + L

        def chain_end(e):
            p = e
            for _ in range(HOPS):
                n = nal_at(p)
                if not n:
                    return p
                p += n
            return p

        limit = len(data) - (OVERLAP if not final else 0)
        for i, eb in enumerate(est):
            if eb >= limit:
                break                      # resolved by the next chunk, which overlaps this one
            nxt = est[i + 1] if i + 1 < len(est) else None
            if nxt is None and not final:
                break
            lo, hi = max(0, eb + BLOCK - W), min(len(b) - 5, eb + BLOCK + W)
            seg = b[lo:hi]
            ok = (seg[:-4] == 0) & (seg[1:-3] < 0x80) & ((seg[4:] & 0x80) == 0) & np.isin(seg[4:] & 0x1F, VALID)
            best = None
            for e in (np.flatnonzero(ok) + lo):
                e = int(e)
                if e < PRE or e - BLOCK < 0 or nal_at(e) == 0:
                    continue
                # note 3: a real block end is preceded by quiet stereo audio, video is not
                # Read the stereo tail in the CANDIDATE'S own alignment (PRE is even, so e-PRE has
                # e's parity); forcing an even offset here evaluates the wrong phase for odd blocks.
                pre = np.abs(np.frombuffer(data, dtype="<i2", count=PRE // 2, offset=e - PRE).astype(np.int32)).mean()
                if pre >= TH:
                    continue
                if nxt is None:
                    best = (e, 0)
                    break
                d = abs(chain_end(e) - nxt)
                if d <= TOL and (best is None or d < best[1]):
                    best = (e, d)
            if best is None:
                dropped += 1
                continue
            s_abs = cpos + best[0] - BLOCK
            if s_abs <= last_start:
                continue                   # already written from the previous chunk's overlap
            blk = data[best[0] - BLOCK : best[0]]
            mono.write(blk[:MONO_BYTES])
            stereo.write(blk[MONO_BYTES:])
            blocks += 1
            last_start = s_abs
        print(f"  {100.0*(cpos+len(data)-start)/(end-start):5.1f}%  blocks={blocks} "
              f"({blocks*4096/48000/60:.1f} min)  dropped={dropped}", file=sys.stderr, flush=True)
        if final:
            break
        cpos += len(data) - OVERLAP

    mono.close()
    stereo.close()
    print(f"done: {blocks} blocks = {blocks*4096/48000/60:.1f} min, dropped={dropped}", file=sys.stderr)


if __name__ == "__main__":
    main()
