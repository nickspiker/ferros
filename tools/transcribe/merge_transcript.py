#!/usr/bin/env python3
# Merge two whisper.cpp WORD-LEVEL JSON transcripts (one per channel, produced with
# `-ml 1`) into a single time-ordered, speaker-labeled script.
#
# Each channel is one known speaker, so attribution is ground truth. We interleave at the
# UTTERANCE level: group each channel's word tokens into bursts split on a real pause
# (start-to-start gap > GAP_MS), then sort bursts by start time.
#   - Coarser than raw words (which shred genuinely-overlapping speech into alternating
#     one-word fragments).
#   - Finer than whisper's VAD segments (which glob a 37s monologue into one block that
#     then sorts *before* the other speaker's interjections — the "out of order" bug).
# Because turns are per-channel, a considerate turn-taking human call lands one turn per
# line naturally; only a rude talk-over counterpart (IVR) produces overlap artifacts.
#
# Note: use token START-to-START gaps, not end-to-start — whisper hides pause time inside
# long punctuation-token durations, so end-to-start gaps read as ~0.
import json, sys, os

GAP_MS = int(os.environ.get("TRANSCRIBE_GAP_MS", "2000"))

def utterances(path, speaker):
    with open(path) as f:
        d = json.load(f)
    us, cur, prev = [], None, None
    for s in d.get("transcription", []):
        t = s.get("text", "")
        if t == "":
            continue
        fr = (s.get("offsets") or {}).get("from", 0)
        if cur is not None and prev is not None and (fr - prev) <= GAP_MS:
            cur["parts"].append(t)
        else:
            if cur:
                us.append(cur)
            cur = {"start": fr, "parts": [t], "spk": speaker}
        prev = fr
    if cur:
        us.append(cur)
    return us

def ts(ms):
    s = ms // 1000
    return f"{s//60:02d}:{s%60:02d}"

def main():
    u = utterances(sys.argv[1], "You") + utterances(sys.argv[2], "Them")
    u.sort(key=lambda x: x["start"])
    for x in u:
        line = "".join(x["parts"]).strip()
        if line and any(c.isalnum() for c in line):   # drop orphan punctuation-only bursts
            print(f"[{ts(x['start'])}] {x['spk']:>4}: {line}")

if __name__ == "__main__":
    main()
