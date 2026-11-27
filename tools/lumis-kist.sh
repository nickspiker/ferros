#!/bin/bash
# Lumis as a stowed kist (KISTS.md "Lumis", docs/CAMERA.md, docs/HOLD-API.md): Lumis's image (std/lumis, Lumis's own core with its ferros platform layer: tools/lumis-build.sh; `--no-build` takes the last one) signed into a capsule that declares the three rights it is asked for, with Lumis's reasons: the camera, the person's things (`items`), with the kinds it makes (photo, its stills; RAW video, its clips), and Bluetooth (its camera mount, lumis ACCESSORIES.md). Everything else it uses is the implicit set (display, touch, surface, ira, its vault domain, entropy in slots 1 to 6), so the camera lands in slot 7, the items in 8 and Bluetooth in 9, where its code looks (std/lumis `CAMERA`, `hold::ITEMS`, `mount::BT`), then the GPU's three (found by probing, ferros_clip `gpu_grants`) and sound out, which it does not use.
# Then stowed, unless --no-stow (docs/KISTS.md "Stowing"): `ferros send` offers it to the device over USB, the device's stower checks it (native code, as Mahere's is, only on a development build), and the person attested there says Stow or Cancel on its screen. Stow it only on a device whose kernel, stower and map know the camera right (code 9, ferros commit "The camera right"): one from before refuses `camera` as a right outside its vocabulary.
# The option is Lumis's identity across versions (ORACLE.md §5: minted once, kept), so an update keeps the person's answers and its vault domain. Minted 2026-11-12; never change it.
# The key: the owner's developer key (Lumis is his own app, KISTS.md "Whose key signs a kist"), keys/developer-nick.key.vsf, handed to the bridge as $FERROS_DEV_KEY (one already set wins); the path is passed, the key never leaves the bridge's process.
# The version is Lumis's own (lumis_core's, rust/Cargo.toml in the Lumis tree beside this one: the same as its Android build), in the signed declaration; $LUMIS_VERSION wins. Since 2026-11-22 it is std/lumis's number too (the owner: "Yup, bump core to match"): lumis_core, the ferros kist and the Android build share one, bumped together.
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
[ -d "$code/ferros/forks/rust" ] || code=$(cd "$here/../../../.." && pwd)
lumis=${LUMIS:-$code/lumis}
bin=${CARGO_TARGET_DIR:-$here/std/lumis/target/ferros}/aarch64-unknown-ferros/${LUMIS_PROFILE:-release}/lumis.bin
version=${LUMIS_VERSION:-$(sed -n -E 's/^version = "([0-9.]+)"$/\1/p' "$lumis/rust/Cargo.toml" | head -n 1)}
[ -n "$version" ] || { echo "no version: set LUMIS_VERSION or check $lumis/rust/Cargo.toml" >&2; exit 1; }
out=target/lumis.capsule.vsf
B=target/debug/ferros-bridge
# Its icon in the one icon format (tools/app-icons `icon`, ferros_kist::Icon), in the signed declaration: kept in the Lumis tree (rust/assets/icon.vsf, Lumis 82aec3e), as Photon keeps its own; $LUMIS_ICON wins. No icon, no capsule: a stow without one shows Lumis with no lens on the deck (2026-11-22: the icon lived only in a build directory, and a stow from a fresh tree lost it).
icon=${LUMIS_ICON:-$lumis/rust/assets/icon.vsf}
[ -f "$icon" ] || { echo "no icon at $icon: make it with tools/app-icons icon <the launcher icon> $icon, or set LUMIS_ICON" >&2; exit 1; }
icon_arg=(--icon "$icon")
FERROS_DEV_KEY="${FERROS_DEV_KEY:-/mnt/Harbor/Code/keys/developer-nick.key.vsf}" "$B" capsule "$bin" "$out" --kist Lumis --option 0b94129b0c3896fa4663993540f730cb623d26de226823b11bb4037c97066556 --version "$version" "${icon_arg[@]}" \
    --want camera "to show you what each camera sees and take its raw frames, kept on this device" \
    --want items "to keep your photos and RAW videos as your own things" \
    --want bluetooth "to drive your camera mount" \
    --makes photo "the stills you save" \
    --makes "RAW video" "the RAW clips you record"
[ "$stow" = 1 ] || exit 0
# Not -q: a build, or a wait on another build's lock, says so instead of looking like a silent device.
cargo run -p ferros-send -- send "$out"
