#!/bin/bash
# Photon as an installed kist (KISTS.md), the second after Mahere: its development image (tools/photon-build.sh; `--no-build` takes the last one) signed into a capsule that declares what it wants (tools/photon-capsule.sh, the declaration's one home), then stowed (docs/KISTS.md "Stowing"): `ferros send` (tools/ferros-send) carries the capsule (about 100 MB) to the device over USB in parts and offers it, the device's installer checks it (native code, as Photon's is until std apps build masked, only on a development build, its screen saying so), and the person attested there says Stow or Cancel on its screen. Its circle appears on the map once stowed; its first launch asks each want once. `ferros-bridge kist install` is the development fallback with nobody at the screen.
# The developer key is $FERROS_DEV_KEY as the bridge reads it (default the ferros key, keys/ferros.key.vsf); Photon's releases are signed with Photon's release key (photon's deploy.sh).
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
[ "${1:-}" = "--no-build" ] || tools/photon-build.sh
bin=std/photon/target/aarch64-unknown-ferros/debug/photon.bin
out=target/photon.capsule.vsf
tools/photon-capsule.sh "$bin" "$out"
# Not -q: a build, or a wait on another build's lock, says so instead of looking like a silent device.
cargo run -p ferros-send -- send "$out"
