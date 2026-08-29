#!/usr/bin/env bash
# Pull the newest call recording off the phone and transcribe it locally.
#   usage: transcribe-latest.sh            # newest recording
#          transcribe-latest.sh -k         # keep the pulled .ogg in /tmp
# Filenames are sortable timestamps, so "newest" = last after sort.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REMOTE_DIR="/sdcard/Recordings"

latest="$(adb shell "ls -1 \"$REMOTE_DIR\"/call*.ogg 2>/dev/null | sort | tail -1" | tr -d '\r')"
[ -n "$latest" ] || { echo "no call recordings found in $REMOTE_DIR" >&2; exit 1; }

base="$(basename "$latest")"
local="/tmp/$base"
echo "pulling: $base" >&2
adb pull "$latest" "$local" >/dev/null
echo "=== transcript: $base ===" >&2
"$HERE/transcribe-call.sh" "$local"
[ "${1:-}" = "-k" ] || rm -f "$local"
