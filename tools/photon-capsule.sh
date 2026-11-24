#!/bin/bash
# Photon's kist capsule (KISTS.md): a built image signed into a capsule that declares what Photon wants, each right with Photon's reason, in its grant slot order (std/photon: 1 display, 2 touch, 3 surface, 4 udp, 5 its own vault domain, 6 ira, 7 entropy;  8 be you and 9 the person's people, asked (docs/FACETS.md, 2026-11-13: its alias bound to the person and their people, std/photon's `BE_YOU` and `PEOPLE`); then the kernel's 10 to 12 the GPU, 13 sound out).
# Only udp is asked (KISTS.md "The implicit set"; owner, 2026-11-12, "Yes on the implicit set"): the other six are every kist's, never asked and never shown. Photon still declares them, unlike Mahere, because a capsule that declares an implicit right gets it in the slot it declares it in, and Photon's slots are fixed in two repos: udp is 4 in the photon repo's src/platform/ferros_net.rs (UDP_SLOT), which every Photon release freezes, and the ira 6 in std/photon. Declaring only udp would move it to slot 7 and the ira to 4 (ferros_kist `slots`), so that change waits for a Photon release that reads the new slots; until then these six lines are placements, their reasons read by no one. The one home of Photon's declaration: tools/photon-kist.sh (development: build, sign, stow over USB with `ferros send`) and Photon's release deploy (photon's scripts/lib/ferros-kist.sh: its frozen build, signed with Photon's release key) both sign thru here. Nothing is built or stowed here.
# The option is Photon's identity across versions (ORACLE.md §5: minted once, kept), so an update keeps the person's answers and its vault domain. Minted 2026-11-08; never change it.
# The key is Photon's release key, keys/photon-signing-key.vsf: the raw photon-signing-key converted once (2026-11-12, kistcat devkey convert --raw keys/photon-signing-key --label PHOTON_RELEASE --secret keys/photon-signing-key.vsf; PHOTON_RELEASE is the label whose post-quantum domains are fgtw SigningBundle::release's, and the bridge knows the key by its public key and signs with the release bundle fgtw pins). It is handed to the bridge as $FERROS_DEV_KEY (a key kept as VSF, ferros_kist::secret: section developer_key { label, seed kge }); a FERROS_DEV_KEY already set wins (Photon's deploy sets it to the same file). A raw key file is refused. Photon's kist is signed with Photon's release key (owner, 2026-11-10: Photon is a commons project, KISTS.md "Whose key signs a kist"); the path is passed, the key never leaves the bridge's process.
# The version is Photon's own (its Cargo.toml `version`, e.g. 0.151.1, three parts as Photon numbers its releases), in the signed declaration, so the Items card shows it and the pandok can list it and offer an update (KISTS.md "The version"); $PHOTON_VERSION wins (Photon's deploy passes the version it injects into its frozen snapshot), else the tree std/photon builds against (PHOTON_FERROS_PKG's, as photon-build.sh), read here.
# The entry (docs/KIST-DISTRIBUTION.md 1.0; owner, 2026-11-24: "One entry format, yes.", Photon included): with `--entry <out.entry.vsf>`, Photon's signed entry for the capsule is written beside it with the same key (tools/kistcat `entry`), naming kists.fgtw.org as where Photon's capsule lives (Photon is a commons kist: "Photon and possibly some other system apps will be hosted at kists.fgtw.org since Photon is not mine"). The capsule then goes to the `kists` bucket at capsules/<blake3>.vsf and the entry beside it at capsules/<blake3>.entry.vsf; kistcat prints the commands.
# Usage: tools/photon-capsule.sh <image.bin> <out.vsf> [--entry <out.entry.vsf>]  (the bridge at target/debug/ferros-bridge, built)
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
entry=""
if [ $# -eq 4 ] && [ "$3" = --entry ]; then
    entry=$4
elif [ $# -ne 2 ]; then
    echo "Usage: $0 <image.bin> <out.vsf> [--entry <out.entry.vsf>]" >&2
    exit 1
fi
pkg=${PHOTON_FERROS_PKG:-$here/std/photon}
version=${PHOTON_VERSION:-$(sed -n -E 's/^version = "([0-9.]+)"$/\1/p' "$pkg/../../../photon/Cargo.toml" | head -n 1)}
[ -n "$version" ] || { echo "no version: set PHOTON_VERSION or check $pkg/../../../photon/Cargo.toml" >&2; exit 1; }
# Its icon in the one icon format (tools/app-icons, ferros_kist::Icon), in the signed declaration; $PHOTON_ICON wins, else target/icons/photon.vsf (app-icons bundle writes it), made here from Photon's logo when it is not there (app-icons icon, the codes assigned VSF RGB g2t as app-icons' APPS has Photon: owner, 2026-11-20).
# Never signed without it: until 2026-11-21 a missing file was passed over in silence, and every Photon capsule stowed on the Pro (from the farm and from exports, where target/icons was never made) carried no icon, so its circle had none.
icon=${PHOTON_ICON:-$here/target/icons/photon.vsf}
if [ ! -f "$icon" ]; then
    logo="$pkg/../../../photon/assets/logo original.jpeg"
    mkdir -p "$(dirname "$icon")"
    OPUS_NO_PKG=1 OPUS_STATIC=1 cargo run --manifest-path "$here/tools/app-icons/Cargo.toml" -- icon "$logo" "$icon" >&2 || { echo "no icon: app-icons could not make $icon from $logo (set PHOTON_ICON to one made already)" >&2; exit 1; }
fi
icon_arg=(--icon "$icon")
FERROS_DEV_KEY="${FERROS_DEV_KEY:-/mnt/Harbor/Code/keys/photon-signing-key.vsf}" "$here/target/debug/ferros-bridge" capsule "$1" "$2" --kist Photon --option 8c2411c6bf1c10a45d8021340b2dae4a7c69774235069eddd85abbdc6959eb74 --version "$version" "${icon_arg[@]}" \
    --want display "to show your conversations on the screen" \
    --want touch "to read your taps and typing" \
    --want surface "to draw in a window of its own" \
    --want udp "to send and receive your photons over the network" \
    --want domain "to keep your contacts and conversations" \
    --want ira "to tie its keys to this device" \
    --want entropy "to make the keys your photons are sealed with" \
    --want be_you "so your contacts find and recognise you in Photon" \
    --want people "to show your people as your contacts"
[ -n "$entry" ] || exit 0
cargo run -q --release --manifest-path "$here/tools/kistcat/Cargo.toml" -- entry --key "${FERROS_DEV_KEY:-/mnt/Harbor/Code/keys/photon-signing-key.vsf}" --capsule "$2" \
    --about "Messages, voice and video between people, not numbers" --host http://kists.fgtw.org --out "$entry"
