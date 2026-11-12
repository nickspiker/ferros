#!/bin/bash
# Photon's kist capsule (KISTS.md): a built image signed into a capsule that declares what Photon wants, each right with Photon's reason, in its grant slot order (std/photon: 1 display, 2 touch, 3 surface, 4 udp, 5 its own vault domain, 6 ira, 7 entropy). The one home of Photon's declaration: tools/photon-kist.sh (development: build, sign, stow over USB with `ferros send`) and Photon's release deploy (photon's scripts/lib/ferros-kist.sh: its frozen build, signed with Photon's release key) both sign thru here. Nothing is built or installed here.
# The option is Photon's identity across versions (ORACLE.md §5: minted once, kept), so an update keeps the person's answers and its vault domain. Minted 2026-11-08; never change it.
# The key is the bridge's: $FERROS_DEV_KEY, a key kept as VSF (ferros_kist::secret: section developer_key { label, seed kge }); the ferros key keys/ferros.key.vsf by default, or Photon's release key converted once from its raw photon-signing-key (kistcat devkey convert --raw keys/photon-signing-key --label PHOTON_RELEASE --secret keys/photon-signing-key.vsf; the bridge knows it by its public key and signs with its release bundle). A raw key file is refused. Photon's kist is signed with Photon's release key (owner, 2026-11-10: Photon is a commons project, KISTS.md "Whose key signs a kist"); the path is passed, the key never leaves the bridge's process.
# Usage: tools/photon-capsule.sh <image.bin> <out.vsf>  (the bridge at target/debug/ferros-bridge, built)
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
if [ $# -ne 2 ]; then
    echo "Usage: $0 <image.bin> <out.vsf>" >&2
    exit 1
fi
"$here/target/debug/ferros-bridge" capsule "$1" "$2" --kist Photon --option 8c2411c6bf1c10a45d8021340b2dae4a7c69774235069eddd85abbdc6959eb74 \
    --want display "to show your conversations on the screen" \
    --want touch "to read your taps and typing" \
    --want surface "to draw in a window of its own" \
    --want udp "to send and receive your photons over the network" \
    --want domain "to keep your contacts and conversations" \
    --want ira "to tie its keys to this device" \
    --want entropy "to make the keys your photons are sealed with"
