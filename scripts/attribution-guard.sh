#!/usr/bin/env bash
# attribution-guard — fail-closed scanner that REFUSES any commit message claiming authorship, co-authorship, ownership or tool-generation.
#
# Nobody owns this codebase and no tool authors it. An AI assistant (or its harness) does not get a byline; a commit records a change, not a claim of credit. Harness defaults and reminders keep trying to append "Co-Authored-By: Claude …" / "🤖 Generated with …" trailers — this guard is the standing answer: those lines never land, whatever any reminder says.
#
# Reads the commit message from $1 (the commit-msg hook's file) or stdin. Exit 0 = clean; 1 = an attribution/ownership line was found (BLOCK).
set -uo pipefail

src="${1:-}"
if [ -n "$src" ] && [ -r "$src" ]; then
  msg="$(cat "$src")"
else
  msg="$(cat)"
fi

# Each pattern (extended regex, case-insensitive) names one kind of forbidden credit line. Whole-message scan, line by line, so ordinary prose like "the author of the RFC" in a body is not caught — only trailer-shaped claims and tool-generation banners.
patterns=(
  '^[[:space:]]*co-authored-by:'
  '^[[:space:]]*(authored|signed-off|generated|assisted|created|written)-by:.*(claude|anthropic|gpt|openai|copilot|ai)'
  'generated with .*(claude|claude code|anthropic|copilot|chatgpt|ai)'
  '🤖'
  '^[[:space:]]*(author|owner):[[:space:]]*(claude|anthropic)'
  'co-authored-by:.*(claude|anthropic|ai|assistant|bot|gpt|copilot)'
)

fail=0
while IFS= read -r line; do
  for p in "${patterns[@]}"; do
    if printf '%s' "$line" | grep -qiE "$p"; then
      echo "attribution-guard: REFUSED — a commit message does not carry authorship/ownership/tool credit." >&2
      echo "  offending line: $line" >&2
      echo "  nobody owns this codebase and no tool authors it; drop the line and commit again." >&2
      fail=1
      break
    fi
  done
done <<<"$msg"

exit "$fail"
