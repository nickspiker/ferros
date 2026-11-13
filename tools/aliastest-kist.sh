#!/bin/bash
# The alias's test kist (payloads/aliastest; docs/FACETS.md "The alias"), as two kists from the same code: "Alias A" and "Alias B", two options, so two kist identities that must get two aliases.
# Built masked (tools/ferros-masked-build.sh), signed by the developer key with every egg and its public bundle (kistcat kist), written to target/kists/aliastest-a.capsule.vsf and aliastest-b.capsule.vsf.
# Then offered to the device (`ferros send`, the stower checks and seals each; Stow on the device's screen), unless --no-stow.
# Launch each from its circle: the console says "aliastest: alias given, fingerprint <8 bytes>" (a one-way hash, never the alias) or why not.
# The options were minted 2026-11-12; never change them, or the kists' aliases change with them.
# The key: $FERROS_DEVELOPER_KEY (default the owner's), its public record beside it; the paths are passed, the key never leaves kistcat's process.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
key=${FERROS_DEVELOPER_KEY:-/mnt/Harbor/Code/keys/developer-nick.key.vsf}
record=${key%.key.vsf}.pub.vsf
tools/ferros-masked-build.sh aliastest
M=target/masked/aliastest
for which in a b; do
    case $which in
        a) name="Alias A"; option=faedad5a801e956d0c40b0060ccbac938e8e364720ac9f4caf37a796a7ee9de2 ;;
        b) name="Alias B"; option=31c560b98c080f72324458321d8b9169416b642da15a10138ba3667498877a01 ;;
    esac
    cargo run -q --release --manifest-path tools/kistcat/Cargo.toml -- kist --key "$key" --record "$record" \
        --masked $M/aliastest.bin --text-len "$(cat $M/aliastest.textlen)" \
        --name "$name" --option $option --version 0 \
        --out target/kists/aliastest-$which.capsule.vsf
done
[ "${1:-}" = --no-stow ] && exit 0
cargo run -p ferros-send -- send target/kists/aliastest-a.capsule.vsf
cargo run -p ferros-send -- send target/kists/aliastest-b.capsule.vsf
