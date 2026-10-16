#!/usr/bin/env bash
# attribution-guard — fail-closed scanner that REFUSES any commit message claiming an AI authored, co-authored or owns the work.
#
# Claude builds to the owner's design; it originates nothing and owns nothing, so it gets no byline. Built by Claude is true and fine — end a message with "Built with Claude <version>" (the Photon convention, .githooks/commit-msg there). What is refused is the false claim: a `Co-Authored-By` / `Authored-by` / author/owner line naming an AI, or "authored by Claude" in prose. A human co-author is a true statement and passes. The Claude Code harness keeps appending a co-author trailer via its attribution reminder; that reminder is overridden here.
#
# Canonical copy. ferros's own commit-msg hook calls it, and so does the global commit-msg hook (~/.config/git/hooks/commit-msg) that every repo without its own hooks path uses.
#
# Reads the commit message from $1 (the commit-msg hook's file) or stdin. Exit 0 = clean; 1 = an AI authorship/ownership claim was found (BLOCK).
set -uo pipefail

src="${1:-}"
if [ -n "$src" ] && [ -r "$src" ]; then
  msg="$(cat "$src")"
else
  msg="$(cat)"
fi

# Names that mark a claim as being about an AI.
ai='(claude|anthropic|openai|chatgpt|gpt|copilot|gemini|assistant|noreply@anthropic)'

# Each pattern (extended regex, case-insensitive) names one shape of a false credit CLAIM. "Built with/by …" / "Generated with …" are deliberately NOT here: they describe the tool, not authorship.
patterns=(
  "^[[:space:]]*co-authored-by:.*${ai}"
  "^[[:space:]]*(authored|signed-off|written|created)-by:.*${ai}"
  "^[[:space:]]*(author|owner):[[:space:]]*${ai}"
  "(co-)?authored[[:space:]]+by[[:space:]]+${ai}"
)

fail=0
while IFS= read -r line; do
  # Git strips comment lines from the message; skip them here too, so the template's own help text never trips the guard.
  case "$line" in \#*) continue ;; esac
  for p in "${patterns[@]}"; do
    if printf '%s' "$line" | grep -qiE "$p"; then
      echo "attribution-guard: REFUSED — a commit message does not claim an AI authored or owns the work." >&2
      echo "  offending line: $line" >&2
      echo "  Claude builds, it does not author; drop the line (\"Built with Claude <version>\" is fine)." >&2
      fail=1
      break
    fi
  done
done <<<"$msg"

exit "$fail"
