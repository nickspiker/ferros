#!/usr/bin/env bash
# Install the ferros git hooks. Run once per clone: scripts/install-hooks.sh
# Points core.hooksPath at scripts/hooks/ so the comment gate runs on every commit.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
git -C "$root" config core.hooksPath scripts/hooks
echo "hooks installed: core.hooksPath -> scripts/hooks"
