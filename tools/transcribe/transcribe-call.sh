#!/usr/bin/env bash
# Transcribe a dual-channel call recording (L=you, R=them) into a time-ordered,
# speaker-labeled transcript. Fully local — nothing leaves this machine.
#
#   usage: transcribe-call.sh <call.ogg>  [> transcript.txt]
#
# Env overrides: WHISPER_MODEL (default large-v3), WHISPER_LANG (default en).
set -euo pipefail
IN="${1:?usage: transcribe-call.sh <call.ogg>}"
HERE="$(cd "$(dirname "$0")" && pwd)"
WHISPER="$HERE/build/bin/whisper-cli"
MODEL="${WHISPER_MODEL:-$HOME/whisper-models/ggml-large-v3.bin}"
LANG="${WHISPER_LANG:-en}"

[ -x "$WHISPER" ] || { echo "whisper-cli not found at $WHISPER" >&2; exit 1; }
[ -f "$MODEL" ]   || { echo "model not found at $MODEL" >&2; exit 1; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# 1. split stereo -> two 16 kHz mono WAVs (whisper wants 16k mono)
ffmpeg -v error -y -i "$IN" \
  -filter_complex "[0:a]channelsplit=channel_layout=stereo[l][r]" \
  -map "[l]" -ar 16000 -ac 1 "$tmp/you.wav" \
  -map "[r]" -ar 16000 -ac 1 "$tmp/them.wav"

# 2. transcribe each channel independently -> JSON with per-segment timestamps
"$WHISPER" -m "$MODEL" -f "$tmp/you.wav"  -l "$LANG" -oj -of "$tmp/you"  >/dev/null 2>&1
"$WHISPER" -m "$MODEL" -f "$tmp/them.wav" -l "$LANG" -oj -of "$tmp/them" >/dev/null 2>&1

# 3. interleave by timestamp into one labeled script
python3 "$HERE/merge_transcript.py" "$tmp/you.json" "$tmp/them.json"
