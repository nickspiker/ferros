#!/bin/bash
# The clock as an installed kist (KISTS.md; owner, 2026-11-10: "can I get my clock widget kist on the homescreen again?"): a widget, drawn live by the display in the kist's circle (payloads/clock says why the capsule has nothing to run), in a capsule that wants nothing.
# Built masked (tools/ferros-masked-build.sh: the network installer refuses native code), then signed by the owner's developer key with every egg and its whole public bundle (kistcat kist; KISTS.md "Trust on first install"), its developer's name from the key's public record. Written to target/kists/clock.capsule.vsf, which tools/kistcat/kists.sh lists in the directory.
# Then stowed, unless --no-install (docs/KISTS.md "Stowing"): `ferros send` (tools/ferros-send) offers it to the device over USB, its installer checks and seals it as it would one from the directory, and the person attested there says Stow or Cancel on its screen. Its circle appears on the map once stowed.
# The option is the clock's identity across versions (ORACLE.md §5: minted once, kept). Minted 2026-11-10; never change it.
# The version is in the capsule's signed declaration (kistcat kist --version: any number of parts; leading zero parts mean nothing, more parts is a later stage, so 12 < 1.0 < 45.0 < 1.0.0). Raise it with every capsule published: the Kists page offers an update only for a newer one.
# The key: $FERROS_DEVELOPER_KEY (default the owner's, label FERROS_DEVELOPER_NICK), its public record beside it (.pub.vsf); the paths are passed, the key never leaves kistcat's process.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
key=${FERROS_DEVELOPER_KEY:-/mnt/Harbor/Code/keys/developer-nick.key.vsf}
record=${key%.key.vsf}.pub.vsf
tools/ferros-masked-build.sh clock
M=target/masked/clock
out=target/kists/clock.capsule.vsf
cargo run -q --release --manifest-path tools/kistcat/Cargo.toml -- kist --key "$key" --record "$record" \
    --masked $M/clock.bin --text-len "$(cat $M/clock.textlen)" \
    --name Clock --option e6f06ca18ca2027b99ea0f0a3e5e97916ea81f57b227a8d9c51a5632aac5a35b --version 0 \
    --out "$out"
[ "${1:-}" = --no-install ] && exit 0
# Not -q: a build, or a wait on another build's lock, says so instead of looking like a silent device.
cargo run -p ferros-send -- send "$out"
