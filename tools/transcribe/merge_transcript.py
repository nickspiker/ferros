#!/usr/bin/env python3
# Merge two whisper.cpp JSON transcripts (one per channel) into a single
# time-ordered, speaker-labeled script. Each channel is one known speaker,
# so attribution is ground-truth, not guessed.
import json, sys

def load(path, speaker):
    with open(path) as f:
        d = json.load(f)
    out = []
    for seg in d.get("transcription", []):
        text = (seg.get("text") or "").strip()
        if not text:
            continue
        ms = (seg.get("offsets") or {}).get("from", 0)
        out.append((ms, speaker, text))
    return out

def ts(ms):
    s = ms // 1000
    return f"{s//60:02d}:{s%60:02d}"

def main():
    you = load(sys.argv[1], "You")
    them = load(sys.argv[2], "Them")
    for ms, spk, text in sorted(you + them, key=lambda x: x[0]):
        print(f"[{ts(ms)}] {spk:>4}: {text}")

if __name__ == "__main__":
    main()
