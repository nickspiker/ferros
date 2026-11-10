#!/bin/bash
# The kist directory's source list (docs/KISTS.md "The directory"): every kist listed on kists.fgtw.org and ferros.org, one entry each, as tools/kistcat takes them. Writes target/kists/kists.vsf (signed by the project key, every egg), target/kists/index.html (the page for kists.fgtw.org) and the list in web/index.html, then prints the wrangler commands that publish them. Uploads nothing.
# A local capsule entry: --capsule <file> --version M.m.p --about TEXT. Its developer key, option, name, blake3 and size come from the capsule; it is published at /capsules/<blake3>.vsf.
# A manifest entry: --manifest <plain-http URL> --name NAME --option HEX --about TEXT. The manifest is fetched and verified now; its ferros/arm64 artefact gives the blake3, size and version; the device resolves the capsule thru the manifest. A manifest without that artefact is skipped with a warning.
# Descriptions are one line of ASCII (the device's installer is no_std), no em dashes.
# The key: $FERROS_KISTS_KEY, a file whose first 32 bytes are the Ed25519 seed (default the ferros project key); the path is passed, the key never leaves kistcat's process.
set -euo pipefail
here=$(cd "$(dirname "$0")/../.." && pwd)
cd "$here"
key=${FERROS_KISTS_KEY:-/mnt/Harbor/Code/keys/ferros.secret}
cargo run -q --release --manifest-path tools/kistcat/Cargo.toml -- build --key "$key" --out target/kists --web web/index.html \
    --capsule target/clock.capsule.vsf --version 0.1.0 \
        --about "The time, drawn live as rings in its own circle on the home screen" \
    --capsule target/mahere.capsule.vsf --version 0.6.1 \
        --about "An offline trail map for the backcountry, drawn from its own tiles" \
    --manifest http://brobdingnagian.holdmyoscilloscope.com/photon/manifest-release.vsf --name Photon \
        --option 8c2411c6bf1c10a45d8021340b2dae4a7c69774235069eddd85abbdc6959eb74 \
        --about "Messages, voice and video between people, not numbers"
# Not yet listable (2026-11-10):
# - Clock (tools/clock-kist.sh) and Mahere (tools/mahere-kist.sh): their capsules in target/ predate the kist section's option (hR) and carry one Ed25519 egg under ferros's key, so kistcat skips them, as `kist install` refuses them. They list once re-signed with every egg (the bridge signs a kist only with Photon's release key until fgtw's developer bundles are wired in).
# - Photon: its release manifest has no ferros/arm64 artefact yet (its ferros kist is not released), so it is skipped with a warning until it does.
