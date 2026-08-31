#!/usr/bin/env bash
# Keep a local archive of every call: pull each recording not yet transcribed, transcribe
# it, and save audio + transcript side by side. Idempotent — skips calls already done, so
# run it anytime (or from cron) and it just catches up. Also gets recordings OFF the phone.
#
#   usage: transcribe-sync.sh
#   env:   CALL_ARCHIVE (default ~/call-transcripts), plus transcribe-call.sh's WHISPER_* vars
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ARCHIVE="${CALL_ARCHIVE:-$HOME/call-transcripts}"
REMOTE_DIR="/sdcard/Recordings"
mkdir -p "$ARCHIVE"

mapfile -t remote < <(adb shell "ls -1 \"$REMOTE_DIR\"/call*.ogg 2>/dev/null" | tr -d '\r')
[ "${#remote[@]}" -gt 0 ] || { echo "no recordings on phone"; exit 0; }

new=0
for path in "${remote[@]}"; do
  [ -n "$path" ] || continue
  base="$(basename "$path")"     # e.g. call 2026-08-30 19.10.57.ogg
  txt="$ARCHIVE/${base%.ogg}.txt"
  ogg="$ARCHIVE/$base"
  [ -f "$txt" ] && continue      # already transcribed
  echo "==> $base"
  adb pull "$path" "$ogg" >/dev/null
  if "$HERE/transcribe-call.sh" "$ogg" > "$txt"; then
    echo "   saved: $txt"
    new=$((new + 1))
  else
    echo "   transcribe failed" >&2
    rm -f "$txt"
  fi
done
echo "done — $new new transcript(s); archive: $ARCHIVE"
