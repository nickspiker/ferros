#!/usr/bin/env bash
# Transcribe a dual-channel call recording (L=you, R=them) into a time-ordered,
# speaker-labeled transcript. Fully local — nothing leaves this machine.
#
#   usage: transcribe-call.sh <call.ogg>  [> transcript.txt]
#
# Env overrides: WHISPER_MODEL (default large-v3), WHISPER_LANG (default en),
#                WHISPER_VAD_MODEL (default bundled silero).
#
# Anti-hallucination: each channel is ~50% silence (the other party talking), and
# Whisper invents/loops speech over silence. VAD skips the silent stretches so it
# only ever sees real speech; -mc 0 stops it conditioning on its own (looped) output.
set -euo pipefail
IN="${1:?usage: transcribe-call.sh <call.ogg>}"
HERE="$(cd "$(dirname "$0")" && pwd)"
WHISPER="$HERE/build/bin/whisper-cli"
MODEL="${WHISPER_MODEL:-$HOME/whisper-models/ggml-large-v3.bin}"
VAD_MODEL="${WHISPER_VAD_MODEL:-$HERE/models/ggml-silero-v5.1.2.bin}"
LANG="${WHISPER_LANG:-en}"

[ -x "$WHISPER" ] || { echo "whisper-cli not found at $WHISPER" >&2; exit 1; }
[ -f "$MODEL" ]   || { echo "model not found at $MODEL" >&2; exit 1; }

WFLAGS=(-m "$MODEL" -l "$LANG" -oj -mc 0)
if [ -f "$VAD_MODEL" ]; then
  WFLAGS+=(--vad --vad-model "$VAD_MODEL")
else
  echo "note: VAD model not found ($VAD_MODEL) — running without VAD; silence may hallucinate" >&2
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# 1. split stereo -> two 16 kHz mono WAVs (whisper wants 16k mono)
ffmpeg -v error -y -i "$IN" \
  -filter_complex "[0:a]channelsplit=channel_layout=stereo[l][r]" \
  -map "[l]" -ar 16000 -ac 1 "$tmp/you.wav" \
  -map "[r]" -ar 16000 -ac 1 "$tmp/them.wav"

# 2. transcribe each channel independently -> JSON with per-segment timestamps
"$WHISPER" "${WFLAGS[@]}" -f "$tmp/you.wav"  -of "$tmp/you"  >/dev/null 2>&1
"$WHISPER" "${WFLAGS[@]}" -f "$tmp/them.wav" -of "$tmp/them" >/dev/null 2>&1

# 3. interleave by timestamp into one labeled script
python3 "$HERE/merge_transcript.py" "$tmp/you.json" "$tmp/them.json"
