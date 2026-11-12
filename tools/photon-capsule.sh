#!/bin/bash
# Photon's kist capsule (KISTS.md): a built image signed into a capsule that declares what Photon wants, each right with Photon's reason, in its grant slot order (std/photon: 1 display, 2 touch, 3 surface, 4 udp, 5 its own vault domain, 6 ira, 7 entropy). The one home of Photon's declaration: tools/photon-kist.sh (development: build, sign, stow over USB with `ferros send`) and Photon's release deploy (photon's scripts/lib/ferros-kist.sh: its frozen build, signed with Photon's release key) both sign thru here. Nothing is built or stowed here.
# The option is Photon's identity across versions (ORACLE.md §5: minted once, kept), so an update keeps the person's answers and its vault domain. Minted 2026-11-08; never change it.
# The key is Photon's release key, keys/photon-signing-key.vsf: the raw photon-signing-key converted once (2026-11-12, kistcat devkey convert --raw keys/photon-signing-key --label PHOTON_RELEASE --secret keys/photon-signing-key.vsf; PHOTON_RELEASE is the label whose post-quantum domains are fgtw SigningBundle::release's, and the bridge knows the key by its public key and signs with the release bundle fgtw pins). It is handed to the bridge as $FERROS_DEV_KEY (a key kept as VSF, ferros_kist::secret: section developer_key { label, seed kge }); a FERROS_DEV_KEY already set wins (Photon's deploy sets it to the same file). A raw key file is refused. Photon's kist is signed with Photon's release key (owner, 2026-11-10: Photon is a commons project, KISTS.md "Whose key signs a kist"); the path is passed, the key never leaves the bridge's process.
# The version is Photon's own (its Cargo.toml `version`, e.g. 0.151.1, three parts as Photon numbers its releases), in the signed declaration, so the Items card shows it and the pandok can list it and offer an update (KISTS.md "The version"); $PHOTON_VERSION wins (Photon's deploy passes the version it injects into its frozen snapshot), else the tree std/photon builds against (PHOTON_FERROS_PKG's, as photon-build.sh), read here.
# Usage: tools/photon-capsule.sh <image.bin> <out.vsf>  (the bridge at target/debug/ferros-bridge, built)
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
if [ $# -ne 2 ]; then
    echo "Usage: $0 <image.bin> <out.vsf>" >&2
    exit 1
fi
pkg=${PHOTON_FERROS_PKG:-$here/std/photon}
version=${PHOTON_VERSION:-$(sed -n -E 's/^version = "([0-9.]+)"$/\1/p' "$pkg/../../../photon/Cargo.toml" | head -n 1)}
[ -n "$version" ] || { echo "no version: set PHOTON_VERSION or check $pkg/../../../photon/Cargo.toml" >&2; exit 1; }
FERROS_DEV_KEY="${FERROS_DEV_KEY:-/mnt/Harbor/Code/keys/photon-signing-key.vsf}" "$here/target/debug/ferros-bridge" capsule "$1" "$2" --kist Photon --option 8c2411c6bf1c10a45d8021340b2dae4a7c69774235069eddd85abbdc6959eb74 --version "$version" \
    --want display "to show your conversations on the screen" \
    --want touch "to read your taps and typing" \
    --want surface "to draw in a window of its own" \
    --want udp "to send and receive your photons over the network" \
    --want domain "to keep your contacts and conversations" \
    --want ira "to tie its keys to this device" \
    --want entropy "to make the keys your photons are sealed with"
