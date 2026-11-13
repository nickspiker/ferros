#!/bin/bash
# Lumis as a stowed kist (KISTS.md "Lumis", docs/CAMERA.md): its ferros front's image (tools/lumis-build.sh; `--no-build` takes the last one) signed into a capsule that declares the one right it is asked for, the camera, with Lumis's reason. Everything else it uses is the implicit set (display, touch, surface, ira, its vault domain, entropy in slots 1 to 6), so the camera lands in slot 7, where its code looks (rust/lumis-ferros `CAMERA`), then the GPU's three and sound out, which it does not use.
# Then stowed, unless --no-stow (docs/KISTS.md "Stowing"): `ferros send` offers it to the device over USB, the device's stower checks it (native code, as Mahere's is, only on a development build), and the person attested there says Stow or Cancel on its screen. Stow it only on a device whose kernel, stower and map know the camera right (code 9, ferros commit "The camera right"): one from before refuses `camera` as a right outside its vocabulary.
# The option is Lumis's identity across versions (ORACLE.md §5: minted once, kept), so an update keeps the person's answers and its vault domain. Minted 2026-11-12; never change it.
# The key: the owner's developer key (Lumis is his own app, KISTS.md "Whose key signs a kist"), keys/developer-nick.key.vsf, handed to the bridge as $FERROS_DEV_KEY (one already set wins); the path is passed, the key never leaves the bridge's process.
# The version is Lumis's own (rust/lumis-ferros/Cargo.toml, kept with the Android build's), in the signed declaration; $LUMIS_VERSION wins.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
stow=1
build=1
for a in "$@"; do
    case "$a" in
        --no-build) build=0 ;;
        --no-stow) stow=0 ;;
        *) echo "usage: tools/lumis-kist.sh [--no-build] [--no-stow]" >&2; exit 1 ;;
    esac
done
[ "$build" = 0 ] || tools/lumis-build.sh
code=$(cd "$here/.." && pwd)
[ -d "$code/ferros-rust" ] || code=$(cd "$here/../../../.." && pwd)
lumis=${LUMIS:-$code/lumis}
crate="$lumis/rust/lumis-ferros"
bin=$crate/target/ferros/aarch64-unknown-ferros/${LUMIS_PROFILE:-release}/lumis.bin
version=${LUMIS_VERSION:-$(sed -n -E 's/^version = "([0-9.]+)"$/\1/p' "$crate/Cargo.toml" | head -n 1)}
[ -n "$version" ] || { echo "no version: set LUMIS_VERSION or check $crate/Cargo.toml" >&2; exit 1; }
out=target/lumis.capsule.vsf
B=target/debug/ferros-bridge
FERROS_DEV_KEY="${FERROS_DEV_KEY:-/mnt/Harbor/Code/keys/developer-nick.key.vsf}" "$B" capsule "$bin" "$out" --kist Lumis --option 0b94129b0c3896fa4663993540f730cb623d26de226823b11bb4037c97066556 --version "$version" \
    --want camera "to show you what each camera sees and take its raw frames, kept on this device"
[ "$stow" = 1 ] || exit 0
# Not -q: a build, or a wait on another build's lock, says so instead of looking like a silent device.
cargo run -p ferros-send -- send "$out"
