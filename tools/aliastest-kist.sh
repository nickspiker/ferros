#!/bin/bash
# The alias's test kist (payloads/aliastest; docs/FACETS.md "The alias"), as two kists from the same code: "Alias A" and "Alias B", two options, so two kist identities that must get two aliases, each the same after a reboot.
# A development loop with nobody at the screen: built native (cargo el0), signed by the developer key at $FERROS_DEV_KEY (the bridge's default, ferros's), kept in the person's list by the bridge's development stow (`kist stow`: no stower, no Stow tap), and launched thru init's `launch` endpoint by epcall, as the map launches a circle.
# The console then says "aliastest: alias given, fingerprint <8 bytes>" (a one-way hash, never the alias) or why not, and the kernel's log "alias given to the kist <name>" or "alias refused to <name>: <why>" (diag log).
#   tools/aliastest-kist.sh build        the two capsules, target/kists/aliastest-a.capsule.vsf and aliastest-b.capsule.vsf
#   tools/aliastest-kist.sh stow         both into the attested person's list (development)
#   tools/aliastest-kist.sh launch a|b   one launched; then `ferros-bridge diag console | grep aliastest`
#   tools/aliastest-kist.sh plain        the same code spawned as a plain process with an Ira grant (not by the kist job): must be refused
# The options were minted 2026-11-12; never change them, or the kists' aliases change with them.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
R=target/el0/aarch64-unknown-none/release
B=target/debug/ferros-bridge
option() { case $1 in a) echo faedad5a801e956d0c40b0060ccbac938e8e364720ac9f4caf37a796a7ee9de2;; b) echo 31c560b98c080f72324458321d8b9169416b642da15a10138ba3667498877a01;; esac; }
case "${1:-}" in
    build)
        cargo el0 -q -p ferros-payload-aliastest -p ferros-payload-epcall
        for x in aliastest epcall; do aarch64-linux-gnu-objcopy -O binary $R/$x $R/$x.bin; done
        cargo build -q -p ferros-bridge
        mkdir -p target/kists
        for w in a b; do
            "$B" capsule $R/aliastest.bin target/kists/aliastest-$w.capsule.vsf --kist "Alias ${w^^}" --option "$(option $w)" --version 0
        done
        ;;
    stow)
        for w in a b; do "$B" kist stow target/kists/aliastest-$w.capsule.vsf | tail -n 1; done
        ;;
    launch)
        w=${2:?a or b}
        address=$(b3sum --no-names target/kists/aliastest-$w.capsule.vsf)
        "$B" spawn $R/epcall.bin "00$address" --name epcall --use launch | grep -a "^proc:" | tail -n 1
        ;;
    plain)
        # Slot 1: the first grant after the console is the --ira one.
        "$B" spawn $R/aliastest.bin 01 --name aliasplain --ira | grep -a "^proc:" | tail -n 1
        ;;
    *) sed -n '2,9p' "$0"; exit 1;;
esac
