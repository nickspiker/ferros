#!/usr/bin/env bash
# Transcribe a recording whose speakers are on SEPARATE AUDIO TRACKS into a time-ordered,
# speaker-labeled transcript. Fully local — nothing leaves this machine.
#
# Companion to transcribe-call.sh, which expects one stereo file (L=you, R=them) as produced by
# ../callrec/. A screen/live recorder instead writes the microphone and the system output as two
# distinct tracks in one container — often with different channel counts (mono mic, stereo system) —
# which channelsplit cannot address. Same ground truth either way: one speaker per track, so
# attribution needs no diarisation.
#
#   usage: transcribe-tracks.sh <file> [you_track] [them_track]  [> transcript.txt]
#          defaults: you_track=1 (mic), them_track=2 (system audio)
#
# Env: WHISPER_BIN, WHISPER_MODEL, WHISPER_VAD_MODEL, WHISPER_LANG, WHISPER_THREADS.
set -euo pipefail

IN="${1:?usage: transcribe-tracks.sh <file> [you_track] [them_track]}"
YOU_T="${2:-1}"
THEM_T="${3:-2}"
HERE="$(cd "$(dirname "$0")" && pwd)"

# Prefer a local build, fall back to a checkout in $HOME — whisper.cpp is commonly built there and
# the binary is far too large to vendor.
WHISPER="${WHISPER_BIN:-}"
if [ -z "$WHISPER" ]; then
    for c in "$HERE/build/bin/whisper-cli" "$HOME/whisper.cpp/build/bin/whisper-cli"; do
        [ -x "$c" ] && { WHISPER="$c"; break; }
    done
fi
MODEL="${WHISPER_MODEL:-$HOME/whisper-models/ggml-large-v3.bin}"
VAD_MODEL="${WHISPER_VAD_MODEL:-}"
if [ -z "$VAD_MODEL" ]; then
    for c in "$HERE/models/ggml-silero-v5.1.2.bin" "$HOME/whisper.cpp/models/ggml-silero-v5.1.2.bin"; do
        [ -f "$c" ] && { VAD_MODEL="$c"; break; }
    done
fi
LANG="${WHISPER_LANG:-en}"
# Two tracks run concurrently, so give each half the box. whisper.cpp scales poorly past ~16
# threads, making 2x16 faster than 2x32 sequentially on a 32-core machine.
THREADS="${WHISPER_THREADS:-$(( $(nproc) / 2 ))}"

[ -n "$WHISPER" ] && [ -x "$WHISPER" ] || { echo "whisper-cli not found (set WHISPER_BIN)" >&2; exit 1; }
[ -f "$MODEL" ] || { echo "model not found at $MODEL (set WHISPER_MODEL)" >&2; exit 1; }

WFLAGS=(-m "$MODEL" -l "$LANG" -oj -mc 0 -ml 1 -t "$THREADS")  # -ml 1 = word timestamps, needed by the merge
if [ -n "$VAD_MODEL" ]; then
    WFLAGS+=(--vad --vad-model "$VAD_MODEL")
else
    echo "note: no VAD model found — silence may hallucinate" >&2
fi

tmp="${TRANSCRIBE_TMP:-$(mktemp -d)}"
mkdir -p "$tmp"
[ -n "${TRANSCRIBE_TMP:-}" ] || trap 'rm -rf "$tmp"' EXIT

# 1. each track -> its own 16 kHz mono WAV (whisper wants 16k mono; stereo tracks downmix)
echo "extracting track $YOU_T (you) and track $THEM_T (them)..." >&2
ffmpeg -v error -y -i "$IN" \
    -map "0:$YOU_T"  -ar 16000 -ac 1 "$tmp/you.wav" \
    -map "0:$THEM_T" -ar 16000 -ac 1 "$tmp/them.wav"

# 2. transcribe both at once — they are independent, and one core-half each beats running in series
echo "transcribing (${THREADS} threads per track, both in parallel)..." >&2
"$WHISPER" "${WFLAGS[@]}" -f "$tmp/you.wav"  -of "$tmp/you"  >/dev/null 2>&1 &
you_pid=$!
"$WHISPER" "${WFLAGS[@]}" -f "$tmp/them.wav" -of "$tmp/them" >/dev/null 2>&1 &
them_pid=$!
wait "$you_pid" || { echo "whisper failed on track $YOU_T" >&2; exit 1; }
wait "$them_pid" || { echo "whisper failed on track $THEM_T" >&2; exit 1; }

# 3. interleave by timestamp into one labeled script
python3 "$HERE/merge_transcript.py" "$tmp/you.json" "$tmp/them.json"
