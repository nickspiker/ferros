#!/bin/bash
# The kist directory (docs/KISTS.md "The directory"; docs/KIST-DISTRIBUTION.md 1.1): kists.fgtw.org/kists.vsf built from developers' signed entries (one entry form, owner 2026-11-24: "One entry format, yes."), each carried byte for byte. Writes target/kists/kists.vsf and target/kists/index.html (the page for kists.fgtw.org), then prints the wrangler commands that publish them. Uploads nothing.
# The entries: every *.entry.vsf in $ENTRIES (default target/kists/entries), each made by its developer with `kistcat entry` (or `kistcat kist --entry`, tools/photon-capsule.sh --entry) and published beside its capsule on the developer's host and on kists.fgtw.org at capsules/<blake3>.entry.vsf.
# UNSIGNED as a whole (owner, 2026-11-10; KIST-DISTRIBUTION.md open question 14 recommends it stays so): every entry verifies alone, so the device checks each one and nothing else.
set -euo pipefail
here=$(cd "$(dirname "$0")/../.." && pwd)
cd "$here"
entries=${ENTRIES:-target/kists/entries}
args=()
for e in "$entries"/*.entry.vsf; do
    args+=(--entry "$e")
done
[ ${#args[@]} -gt 0 ] || { echo "no entries in $entries" >&2; exit 1; }
cargo run -q --release --manifest-path tools/kistcat/Cargo.toml -- build --unsigned --out target/kists "${args[@]}"
