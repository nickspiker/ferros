#!/usr/bin/env bash
# The shell's development loop on husky without a reboot: build the named payloads, sign them into capsules, put them in the vault, and restart those processes from it (kernel `diag restart`, service.rs). Restarting the display server restarts the map too: the map hands a fresh server its scene when it starts.
# Usage: tools/husky-shell.sh display|map|touch|ahau|ip|ramps|init ...   (ramps and init are capsules started by others: they are only stored; ahau restarted puts the attest screen up again)
set -euo pipefail
REPO=/mnt/Harbor/Code/ferros
cd "$REPO"
B=target/debug/ferros-bridge
R=target/el0/aarch64-unknown-none/release
[ $# -gt 0 ] || { sed -n '2,4p' "$0"; exit 1; }
pkgs=(); for n in "$@"; do pkgs+=(-p "ferros-payload-$n"); done
# The display server embeds the app icons; bring the bundle up to date first.
[[ " $* " == *" display "* ]] && (cd tools/app-icons && cargo run -q --release -- "$REPO/target/icons" >/dev/null)
cargo el0 -q "${pkgs[@]}" 2>&1 | grep -E "^error" -A10 && exit 1 || true
restart=()
for n in "$@"; do
    aarch64-linux-gnu-objcopy -O binary "$R/$n" "$R/$n.bin"
    "$B" capsule "$R/$n.bin" "target/$n.capsule.vsf" >/dev/null
    timeout 120 "$B" store put "$n" "@target/$n.capsule.vsf" 2>&1 | grep -q "stored" || { echo "store put $n failed"; exit 1; }
    case $n in display|map|touch|ahau|ip) restart+=("$n");; esac
done
# The map follows the display server.
if [[ " ${restart[*]:-} " == *" display "* && " ${restart[*]} " != *" map "* ]]; then restart+=(map); fi
if [ ${#restart[@]} -gt 0 ]; then
    timeout 60 "$B" diag "restart ${restart[*]}" 2>&1 | grep -aE "restarted|started|not a|no capsule|:" | grep -avE "SPEC|DATA|recv|COMPLETE|Command|Connected|Received|blasted|ACK" || true
    sleep 2
    timeout 30 "$B" diag console 2>&1 | grep -aE "^[a-z]+: " | tail -12
fi
