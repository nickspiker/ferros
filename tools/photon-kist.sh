#!/bin/bash
# Photon as an installed kist (KISTS.md), the second after Mahere: its development image (tools/photon-build.sh; `--no-build` takes the last one) signed into a capsule that declares what it wants (tools/photon-capsule.sh, the declaration's one home), then installed for the person attested on the device: the capsule (about 100 MB) kept in the vault in parts under its blake3, the kist added to their list. Its circle appears on the map at the next tap on the plane; its first launch asks each want once.
# The developer key is $FERROS_DEV_KEY as the bridge reads it (default the ferros key, keys/ferros.key.vsf); Photon's releases are signed with Photon's release key (photon's deploy.sh).
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
[ "${1:-}" = "--no-build" ] || tools/photon-build.sh
bin=std/photon/target/aarch64-unknown-ferros/debug/photon.bin
out=target/photon.capsule.vsf
tools/photon-capsule.sh "$bin" "$out"
target/debug/ferros-bridge kist install "$out"
