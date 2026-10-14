---
name: opsin_recordings_on_harbor
description: opsin live recordings live at /mnt/Harbor/Videos/opsin (never $HOME — it filled at 25 GB/h and killed a recording); transcripts/downmixes sit beside them; tools/transcribe + tools/recover-mov-audio in ferros handle two-track call audio and moov-less MOV recovery
metadata: 
  node_type: memory
  type: project
  originSessionId: 2d5cdf79-8f0e-45bf-807e-cfd54cea13de
  modified: 2026-09-23T20:56:20.129Z
---

**Recordings:** opsin's live capture (4K h264 ~50 Mbit/s + mic mono + system stereo PCM, ~25 GB/h) writes to `/mnt/Harbor/Videos/opsin/` as of 2026-09-23 (`OPSIN_RECORD_DIR` overrides), as fragmented MOV (~2 s fragments) so a killed ffmpeg stays playable. Before that it wrote plain `+faststart` MOV to `$HOME/Videos/opsin`; `/home` (237 GB) hit 100% mid-call on 2026-09-23 and the 30 GB recording lost its moov index.

**Why:** the audio tracks are raw PCM, which has no packet signature, so index-rebuild tools (untrunc) prune them as empty — a moov-less plain MOV is unrecoverable by normal means.

**How to apply:**
- Deliverables for a call go NEXT TO the recording: `<stem>.transcript.txt`, `<stem>.downmix-L-mic-R-system.flac`, `<stem>.recovered-audio.mka` (when recovered).
- `tools/transcribe/transcribe-tracks.sh` — two-track containers (mic track 1, system track 2); `transcribe-call.sh` is for callrec's single stereo L/R file and cannot address separate tracks.
- `tools/transcribe/downmix-tracks.sh` — speaker per side, EBU R128 loudness-matched.
- `tools/recover-mov-audio/recover-mov-audio.py` — pulls the PCM out of a moov-less MOV by content (block-width sliding mean + NAL-length chaining); validated 33859/33923 blocks byte-exact on an intact reference. Chunk sizes (8192 B mono / 16384 B stereo) are this recorder's.
- whisper.cpp is at `~/whisper.cpp` (not tools/transcribe/build); model `~/whisper-models/ggml-large-v3.bin`.

See [[feedback_measure_before_fixing]] — the recovery only converged once each hypothesis was checked against the reference file's real index instead of tuned by feel.
