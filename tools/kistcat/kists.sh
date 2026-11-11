#!/bin/bash
# The kist directory's source list (docs/KISTS.md "The directory"): every kist listed on kists.fgtw.org and ferros.org, one entry each, as tools/kistcat takes them. Writes target/kists/kists.vsf, target/kists/index.html (the page for kists.fgtw.org) and the list in web/index.html, then prints the wrangler commands that publish them. Uploads nothing.
# A local capsule entry: --capsule <file> --about TEXT. Its developer key, option, name, developer's name, version (its signed declaration's: kistcat kist --version), blake3 and size come from the capsule; it is published at /capsules/<blake3>.vsf.
# A manifest entry: --manifest <plain-http URL> --name NAME --option HEX --about TEXT. The manifest is fetched and verified now; its ferros/arm64 artefact gives the blake3, size and version; the device resolves the capsule thru the manifest. A manifest without that artefact is skipped with a warning.
# Descriptions are one line of ASCII (the device's installer is no_std), no em dashes.
# UNSIGNED for now (owner, 2026-11-10): the directory's signature waits for the project key, which the FGTW kaitiakitanga has not chosen. The device reads it unchecked and its Kists page says so. Once the key exists, replace --unsigned with --key <its secret>.
set -euo pipefail
here=$(cd "$(dirname "$0")/../.." && pwd)
cd "$here"
cargo run -q --release --manifest-path tools/kistcat/Cargo.toml -- build --unsigned --out target/kists --web web/index.html \
    --capsule target/kists/clock.capsule.vsf \
        --about "The time, drawn live as rings in its own circle on the home screen"
# Not yet listable (2026-11-10):
# - Mahere (tools/mahere-kist.sh): its capsule is native and signed the old way (one Ed25519 egg, no developer bundle), so the device would refuse it. It lists once built masked and signed with `kistcat kist` and the owner's developer key (its version, 0.6.1, then in the capsule), as tools/clock-kist.sh does the Clock:
#     --capsule target/kists/mahere.capsule.vsf --about "An offline trail map for the backcountry, drawn from its own tiles"
# - Photon: its release manifest has no ferros/arm64 artefact yet (its ferros kist is not released):
#     --manifest http://brobdingnagian.holdmyoscilloscope.com/photon/manifest-release.vsf --name Photon --option 8c2411c6bf1c10a45d8021340b2dae4a7c69774235069eddd85abbdc6959eb74 --about "Messages, voice and video between people, not numbers"
