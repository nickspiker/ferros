#!/bin/bash
# basecalc as a stowed kist (KISTS.md "basecalc"): its image (std/basecalc, basecalc's own engine behind the kist's keypad: tools/basecalc-build.sh; `--no-build` takes the last one) signed into a capsule. Since 2026-11-18 a puck by default (docs/PUCKS.md; owner: "basecalc calculator! Well, a nice GUI puck version anyway"): built `--features puck`, its capsule declaring `puck` and nothing else; `--window` signs the pad in a window that fills the glass instead, asking for nothing. Either is basecalc (one option, so one vault domain); the last stowed is the one on the deck. Everything it uses is the implicit set (display, touch, surface, ira, its vault domain, entropy in slots 1 to 6), then the GPU's three and sound out, which it does not use.
# Then stowed, unless --no-stow (docs/KISTS.md "Stowing"): `ferros send` offers it to the device over USB, the device's stower checks it (native code, as Lumis's and Mahere's are, only on a development build), and the person attested there says Stow or Cancel on its screen.
# The option is basecalc's identity across versions (ORACLE.md §5: minted once, kept), so an update keeps its vault domain (the settings and the history). Minted 2026-11-17; never change it.
# The key: the owner's developer key (basecalc is his own app, KISTS.md "Whose key signs a kist"), keys/developer-nick.key.vsf, handed to the bridge as $FERROS_DEV_KEY (one already set wins); the path is passed, the key never leaves the bridge's process.
# The version is the kist's own (std/basecalc/Cargo.toml), in the signed declaration; $BASECALC_VERSION wins.
# The bridge is target/debug/ferros-bridge (cargo build -p ferros-bridge); $FERROS_BRIDGE names another.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
stow=1
build=1
puck=1
for a in "$@"; do
    case "$a" in
        --no-build) build=0 ;;
        --no-stow) stow=0 ;;
        --window) puck=0 ;;
        *) echo "usage: tools/basecalc-kist.sh [--window] [--no-build] [--no-stow]" >&2; exit 1 ;;
    esac
done
features=()
wants=()
if [ "$puck" = 1 ]; then
    features=(--features puck)
    wants=(--want puck "its circle on the deck")
fi
[ "$build" = 0 ] || tools/basecalc-build.sh "${features[@]}"
bin=${CARGO_TARGET_DIR:-$here/std/basecalc/target/ferros}/aarch64-unknown-ferros/${BASECALC_PROFILE:-release}/basecalc.bin
version=${BASECALC_VERSION:-$(sed -n -E 's/^version = "([0-9.]+)"$/\1/p' std/basecalc/Cargo.toml | head -n 1)}
[ -n "$version" ] || { echo "no version: set BASECALC_VERSION or check std/basecalc/Cargo.toml" >&2; exit 1; }
out=${BASECALC_CAPSULE:-target/basecalc.capsule.vsf}
B=${FERROS_BRIDGE:-target/debug/ferros-bridge}
FERROS_DEV_KEY="${FERROS_DEV_KEY:-/mnt/Harbor/Code/keys/developer-nick.key.vsf}" "$B" capsule "$bin" "$out" --kist basecalc --option 7a254f7e25644cabcb79bfc5f54f0d5becd28d582393474f7299b103a3deeac1 --version "$version" "${wants[@]}"
[ "$stow" = 1 ] || exit 0
# Not -q: a build, or a wait on another build's lock, says so instead of looking like a silent device.
cargo run -p ferros-send -- send "$out"
