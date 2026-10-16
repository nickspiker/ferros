#!/usr/bin/env bash
# attribution-guard — fail-closed scanner that REFUSES any commit message claiming authorship, co-authorship or ownership.
#
# Claude is a tool, not an author. It builds to the owner's design; it originates nothing and owns nothing, so it gets no byline. A factual tool note is fine and encouraged — end a message with "Built with Claude <version>" (the Photon convention, .githooks/commit-msg there). What is refused is the false claim: a `Co-Authored-By` / `Authored-by` / author/owner line. The Claude Code harness keeps appending such trailers via its attribution reminder; that reminder is overridden here.
#
# Reads the commit message from $1 (the commit-msg hook's file) or stdin. Exit 0 = clean; 1 = an authorship/ownership claim was found (BLOCK).
set -uo pipefail

src="${1:-}"
if [ -n "$src" ] && [ -r "$src" ]; then
  msg="$(cat "$src")"
else
  msg="$(cat)"
fi

# Each pattern (extended regex, case-insensitive) names one shape of a forbidden credit CLAIM. "Built with …" / "Generated with …" are deliberately NOT here — they describe the tool, not authorship.
patterns=(
  '^[[:space:]]*co-authored-by:'
  '^[[:space:]]*(authored|signed-off|written|created)-by:.*(claude|anthropic|gpt|openai|copilot|chatgpt|assistant|[[:space:]]ai[[:space:]])'
  '^[[:space:]]*(author|owner):[[:space:]]*(claude|anthropic|openai|gpt|copilot)'
)

fail=0
while IFS= read -r line; do
  for p in "${patterns[@]}"; do
    if printf '%s' "$line" | grep -qiE "$p"; then
      echo "attribution-guard: REFUSED — a commit message does not claim authorship or ownership." >&2
      echo "  offending line: $line" >&2
      echo "  Claude is a tool, not an author; drop the line (a factual \"Built with Claude <version>\" note is fine)." >&2
      fail=1
      break
    fi
  done
done <<<"$msg"

exit "$fail"
