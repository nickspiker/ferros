#!/usr/bin/env bash
# Wrapped-comment ratchet, ported from photon's scripts/lib/comment-gate.sh.
# Hard-wrapped comment prose is banned across the ferros workspace: one line per thought — a sentence never continues onto the next comment line. Long lines are CORRECT (the editor soft-wraps them); it is manual mid-sentence continuations that make comments hard to read.
# A wrap = a comment line longer than 60 columns ending mid-clause (a word character) whose next line is the same comment marker (`//`, `///`, or `//!`) continuing in lowercase.
# Baseline is ZERO. Do not add a baseline/allowlist mechanism — join the wrapped sentence onto one line instead.
#
# Run standalone (scans the whole workspace, exits non-zero on any offender):
#   scripts/comment-gate.sh
# Or wire it as a pre-commit hook — see scripts/install-hooks.sh.
set -euo pipefail

# Resolve repo root so the gate works from any CWD (standalone or as a git hook).
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

offenders=$(find "$root" -name "*.rs" -not -path "*/target/*" | while read -r f; do
    awk -v F="$f" '
        FNR == 1 { pm = ""; prev = "" }
        {
            m = ""
            if ($0 ~ /^[[:space:]]*\/\//) {
                t = $0; sub(/^[[:space:]]*/, "", t)
                m = (substr(t, 1, 3) == "///") ? "///" : "//"
            }
            if (pm != "" && m == pm) {
                body = $0; sub(/^[[:space:]]*\/+!?\/* ?/, "", body)
                if (length(prev) > 60 && prev ~ /[A-Za-z]$/ && body ~ /^[a-z]/) print F ":" FNR - 1
            }
            pm = m; prev = $0
        }
    ' "$f"
done)

if [ -n "$offenders" ]; then
    echo "COMMENT GATE: hard-wrapped comment prose (one line per thought, never wrapped):" >&2
    echo "$offenders" >&2
    echo "COMMENT GATE: blocked — join each sentence onto one line." >&2
    exit 1
fi
echo "COMMENT GATE: clean."
