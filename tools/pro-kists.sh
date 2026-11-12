#!/bin/bash
# The kists the 8a stowed and ran on 2026-11-12, stowed on the Pro the same way once its new build is flashed and someone has attested there (docs/KISTS.md "Stowing"): each capsule offered with `ferros send` (tools/ferros-send), checked by the Pro's stower, and stowed when the person taps Stow on its Deck page. Nothing is built or signed here: the capsules are the very files the 8a checked, so the Pro gets what was tested.
# The capsules, in $KISTS_DIR (default /mnt/Harbor/tmp/kists-2026-11-12, local only, never in a repo):
#   mahere.capsule.vsf: Mahere 0.6.1 (/mnt/Harbor/tmp/mahere-gpu, branch ferros-gpu, release build), signed with the owner's developer key (tools/mahere-kist.sh), 51 MiB.
#   photon.capsule.vsf: Photon 0.151.1, signed with Photon's release key (tools/photon-capsule.sh), 87 MiB.
#   clock.capsule.vsf: the clock, masked, signed with the owner's developer key (tools/clock-kist.sh --no-stow), 13 KiB.
#   vault.masked.capsule.vsf, vault.bin, vault.textlen: the vault process with its 56 MiB heap (payloads/vault, 2026-11-12), masked, for `tools/pro-kists.sh vault` (below).
# First, once: `tools/pro-kists.sh vault` stores that vault process on the Pro and seals it there, then reboot the Pro. On the 8a the old one (a 24 MiB heap) ran out opening a vault grown by Mahere's cells and the capsules (its high water was 43 MiB once it could open), every boot, and took the GPU, WiFi and sensors processes with it; it also ended mid offer when Photon's 16th part came in. Its console says the open's high water ("vault: open in ... the heap's high water ...").
# A newer build of any of them: its own script writes target/<name>.capsule.vsf (the clock target/kists/clock.capsule.vsf); copy it into $KISTS_DIR, or run that script with the Pro on USB instead (each ends in `ferros send`).
# The device: the first ferros device on USB, or $FERROS_BUS (lsusb's bus number) when the 8a is plugged in too.
# Each `ferros send` holds the USB link until the person answers on the glass; the bridge cannot reach the Pro meanwhile.
# Exit: 0 when every kist named was stowed, else 1 (the first one that was not, and why, is said).
# Usage: tools/pro-kists.sh vault | [mahere] [photon] [clock]   (none: all three kists, in that order)
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
dir=${KISTS_DIR:-/mnt/Harbor/tmp/kists-2026-11-12}
if [ "${1:-}" = vault ]; then
    cargo build -q -p ferros-bridge
    B=target/debug/ferros-bridge
    "$B" store put vault "@$dir/vault.masked.capsule.vsf" | tail -n 1
    "$B" store get vault "$dir/vault.readback.vsf" | tail -n 1
    cmp "$dir/vault.readback.vsf" "$dir/vault.masked.capsule.vsf" && echo "== the vault capsule reads back the same"
    rm -f "$dir/vault.readback.vsf"
    "$B" seal "$dir/vault.bin" --text-len "$(cat "$dir/vault.textlen")" | tail -n 1
    echo "== now reboot the Pro (ferros-bridge reboot); its console should say \"vault: open in\" with the heap's high water"
    exit 0
fi
kists=("$@")
[ ${#kists[@]} -gt 0 ] || kists=(mahere photon clock)
for k in "${kists[@]}"; do
    case $k in mahere|photon|clock) ;; *) echo "unknown kist $k (mahere, photon, clock)" >&2; exit 1;; esac
    [ -f "$dir/$k.capsule.vsf" ] || { echo "no $dir/$k.capsule.vsf: build it with its script (see the top of this file) and copy it there" >&2; exit 1; }
done
cargo build -q -p ferros-send
cat <<'CHECK'
On the Pro, before this goes on:
  0. It runs the new build and someone has attested (the map on the glass, no app in front: an offer waits until Home).
For each kist, when the console below says "waiting for the person":
  1. The Deck page opens on the offer by itself: its name and version, "by" its developer's key, its size, "unchecked code, development build only" (Mahere and Photon are native), what it asks for and why.
  2. Scroll to the bottom and tap Stow. The console then says "stowed" and the next kist goes.
After the last:
  3. Deck page (the settings circle opens on it): each kist's row opens its card, Back above it.
     Mahere: made by G#BB0CCCBD, version 6.1, 51 MiB, its eight permissions "not asked yet", Unstow at the bottom.
     Photon: made by G#DFF3AF0C (its release key), version 151.1, 87 MiB, Stop while it runs. The clock is a puck: "shows the time", asks for nothing.
     A Mahere made by G#3B472B69 (the ferros project key, stowed before 2026-11-12) or G#05302376 (the exposed August key) is another kist with its own answers: Unstow it (tap twice).
  4. Home, tap Mahere's circle: tick its eight permissions, Start. Its console says "the map draws on the GPU" with a 64 MiB block, and cells load ("fetch ... 200").
     Pinch and turn with two fingers; back (bottom left) is Escape; deck's swipe inward (bottom right) is full screen on and off; deck's tap goes Home.
  5. Photon's circle (a new circle starts small: deck's swipe on Home flies out to every circle): tick its seven permissions, Start; its first screen asks for a handle. Then the clock: its circle shows the time.
CHECK
for k in "${kists[@]}"; do
    echo "== $k: tap Stow on the Pro when it asks"
    set +e
    target/debug/ferros send "$dir/$k.capsule.vsf"
    r=$?
    set -e
    case $r in
        0) ;;
        2) echo "== $k was cancelled on the device; nothing more is offered" >&2; exit 1 ;;
        *) echo "== $k was not stowed (above: why); nothing more is offered" >&2; exit 1 ;;
    esac
done
echo "== stowed: ${kists[*]}"
