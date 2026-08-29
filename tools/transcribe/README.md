# transcribe — local dual-channel call transcription

Turns a dual-channel call recording (from [`../callrec/`](../callrec/) — you-left, them-right) into a **time-ordered, speaker-labeled transcript**, entirely on your own machine. Nothing is uploaded.

Because each speaker is already on their own channel, attribution is **physical ground truth** — we transcribe each channel independently and interleave by timestamp. No acoustic diarization (guessing who's talking from a mono mix); the channel *is* the speaker.

```
[00:00]  You: the quick brown fox jumped over the lazy dog
[00:00] Them: 2-0-6-4-0-6-6-9-2-5
[00:08] Them: You have reached test call.
```

## Pipeline

1. `ffmpeg` splits the stereo `.ogg` into two 16 kHz mono WAVs (L/R).
2. `whisper.cpp` transcribes each channel → JSON with per-segment timestamps.
3. `merge_transcript.py` interleaves them by time into one `You:` / `Them:` script.

## Anti-hallucination (why VAD is required)

Each channel is ~50% silence — while one party talks, the other channel is dead air. Whisper was trained on continuous speech, so over silence it **invents and loops** phrases ("I don't want a text message" ×15). Splitting into two channels doubles the silence, so this is the dominant failure mode for call transcription. The fix (baked into `transcribe-call.sh`):
- **`--vad`** (Silero) — skip the silent stretches so Whisper only sees real speech. This is the root-cause fix.
- **`-mc 0`** — zero stored context, so it can't condition on its own looped output.

Without VAD, a 4-minute call produced 250+ lines of looping garbage; with it, 54 clean lines. It's not a model-quality issue (large-v3 is the most accurate and actually loops *more* on silence) — it's silence handling.

## Setup (one-time)

Uses [whisper.cpp](https://github.com/ggerganov/whisper.cpp) — pure C++, no Python/torch, great on CPU (chosen over faster-whisper because bleeding-edge Python versions often lack the ML wheels).

```bash
git clone --depth 1 https://github.com/ggerganov/whisper.cpp ~/whisper.cpp
cmake -S ~/whisper.cpp -B ~/whisper.cpp/build -DCMAKE_BUILD_TYPE=Release
cmake --build ~/whisper.cpp/build -j"$(nproc)"
# accuracy-first model (time doesn't matter for calls):
mkdir -p ~/whisper-models
curl -L -o ~/whisper-models/ggml-large-v3.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin
# VAD model (REQUIRED for clean output — see "Anti-hallucination" below):
bash ~/whisper.cpp/models/download-vad-model.sh silero-v5.1.2
# put these scripts next to the whisper.cpp build (they resolve build/bin/whisper-cli relative to themselves):
cp tools/transcribe/*.sh tools/transcribe/*.py ~/whisper.cpp/
```

GPU is optional — an AMD card works via whisper.cpp's Vulkan/HIP backend, but a decent CPU transcribes call-length audio fine.

## Use

```bash
~/whisper.cpp/transcribe-latest.sh            # pull newest recording off the phone + transcribe
~/whisper.cpp/transcribe-call.sh <call.ogg>   # transcribe a specific local file
```

Env overrides: `WHISPER_MODEL` (default `~/whisper-models/ggml-large-v3.bin`), `WHISPER_LANG` (default `en`).
