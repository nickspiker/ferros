#!/bin/bash
# Kūaha as a stowed kist (docs/KISTS.md "Kūaha"): its image (std/kuaha: tools/kuaha-build.sh; `--no-build` takes the last one) signed into a capsule that declares the one right it asks for, `items` ("your things"), and opens every kind it shows (ferros_kist's `items` section: `--opens KIND REASON`), making none (it views). Everything else it uses is the implicit set (display, touch, surface, ira, its vault domain, entropy in slots 1 to 6), so `items` lands in slot 7, where its code looks (std/kuaha `ITEMS`).
# Then stowed, unless --no-stow (docs/KISTS.md "Stowing"): `ferros send` offers it over USB; a device in developer mode stows it with every check run and no tap ("Auto-stow in developer mode"), any other asks the person attested there to tap Stow or Cancel. Stow it only on a device whose kernel, vault and stower know the hold (branch `hold`: the `items` right, code 13, and its kinds): one from before refuses `items` as a right outside its vocabulary.
# The option is Kūaha's identity across versions (ORACLE.md §5: minted once, kept), so an update keeps the person's answers and its vault domain. Minted 2026-11-20; never change it.
# The key: the ferros project's (Kūaha is ferros's own, a commons kist, KISTS.md "Whose key signs a kist": the same key ferros's pucks are signed with, tools/pucks-kist.sh), keys/ferros.key.vsf, handed to the bridge as $FERROS_DEV_KEY (one already set wins); the path is passed, the key never leaves the bridge's process.
# The name in the capsule is Kuaha: the person's list keeps kists' names as ASCII (`a`), so the macron waits for names as ideas (IDEAS.md); every word Kūaha draws itself has it.
# The version is std/kuaha's own (its Cargo.toml), in the signed declaration; $KUAHA_VERSION wins.
# FERROS_BUS pins `ferros send` and the bridge to one device when two are plugged in (bridge_debug_for_two_devices).
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
stow=1
build=1
for a in "$@"; do
    case "$a" in
        --no-build) build=0 ;;
        --no-stow) stow=0 ;;
        *) echo "usage: tools/kuaha-kist.sh [--no-build] [--no-stow]" >&2; exit 1 ;;
    esac
done
[ "$build" = 0 ] || tools/kuaha-build.sh
bin=${CARGO_TARGET_DIR:-$here/std/kuaha/target/ferros}/aarch64-unknown-ferros/${KUAHA_PROFILE:-release}/kuaha.bin
version=${KUAHA_VERSION:-$(sed -n -E 's/^version = "([0-9.]+)"$/\1/p' std/kuaha/Cargo.toml | head -n 1)}
out=${KUAHA_CAPSULE:-target/kuaha.capsule.vsf}
B=${BRIDGE:-target/debug/ferros-bridge}
# The kinds it opens: every kind it shows (std/kuaha/core kinds::SHOWN), by their first phrasings as the bridge takes them (`ferros-bridge items kinds`).
opens=()
for kind in "photo" "RAW video" "video" "document" "scan" "list" "contact" "song" "GPS track" "find" "collection" "Sega Genesis save state" "Sega Genesis battery RAM"; do
    opens+=(--opens "$kind" "to show it to you among your things, and remove it when you say")
done
FERROS_DEV_KEY="${FERROS_DEV_KEY:-/mnt/Harbor/Code/keys/ferros.key.vsf}" "$B" capsule "$bin" "$out" --kist Kuaha --option 7ade4957bb21aa6f75736e2fd38dc04a9057632b30d3b8aecc2cabb642d3f187 --version "$version" \
    --want items "to show you what is on this device: your things in the hold, and what each kist keeps" "${opens[@]}"
[ "$stow" = 1 ] || exit 0
# $SEND names a ferros-send already built (an agent's build directory); else it is built here. Not -q: a build, or a wait on another build's lock, says so instead of looking like a silent device.
if [ -n "${SEND:-}" ]; then
    "$SEND" send "$out"
else
    cargo run -p ferros-send -- send "$out"
fi
