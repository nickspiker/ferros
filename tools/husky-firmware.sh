#!/usr/bin/env bash
# Fetch the husky WiFi firmware set from the phone's own vendor partition (Android on slot a, root) into a local directory outside the repo, and verify every file against firmware/husky/MANIFEST.md (names, sizes, BLAKE3). Nothing proprietary is ever committed: the repo pins hashes, the bytes come from the owner's own phone.
# Usage: tools/husky-firmware.sh [dest]   (default /mnt/Harbor/firmware/husky)
set -euo pipefail
REPO=/mnt/Harbor/Code/ferros
DEST=${1:-/mnt/Harbor/firmware/husky}
MAN=$REPO/firmware/husky/MANIFEST.md
mkdir -p "$DEST"
adb get-state >/dev/null || { echo "need Android (slot a) on adb" >&2; exit 1; }
bad=0
while IFS='|' read -r _ name size hash _; do
    name=$(echo "$name" | xargs); size=$(echo "$size" | xargs); hash=$(echo "$hash" | xargs)
    [ -z "$name" ] || [ "$name" = file ] || [ "$name" = "---" ] && continue
    adb exec-out su -c "cat /vendor/firmware/$name" > "$DEST/$name"
    got=$(b3sum --no-names "$DEST/$name")
    if [ "$(stat -c%s "$DEST/$name")" = "$size" ] && [ "$got" = "$hash" ]; then echo "ok   $name"; else echo "BAD  $name (got $got)"; bad=1; fi
done < <(grep '^| ' "$MAN")
exit $bad
