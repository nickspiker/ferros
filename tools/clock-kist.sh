#!/bin/bash
# The clock as an installed kist (KISTS.md; owner, 2026-11-10: "can I get my clock widget kist on the homescreen again?"): a widget, drawn live by the display in the kist's circle (payloads/clock says why the capsule has nothing to run), in a capsule that wants nothing.
# Built masked (tools/ferros-masked-build.sh: the network installer refuses native code), then signed by the owner's developer key with every egg and its whole public bundle (kistcat kist; KISTS.md "Trust on first install"), its developer's name from the key's public record. Written to target/kists/clock.capsule.vsf, which tools/kistcat/kists.sh lists in the directory.
# Then installed for the person attested on the device over the bridge (development), unless --no-install. Its circle appears on the map at the next tap on the plane.
# The option is the clock's identity across versions (ORACLE.md §5: minted once, kept). Minted 2026-11-10; never change it.
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
    --name Clock --option e6f06ca18ca2027b99ea0f0a3e5e97916ea81f57b227a8d9c51a5632aac5a35b \
    --out "$out"
[ "${1:-}" = --no-install ] && exit 0
target/debug/ferros-bridge kist install "$out"
