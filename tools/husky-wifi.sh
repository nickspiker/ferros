#!/usr/bin/env bash
# Run the pcie1 driver process (payloads/pcie1, WIFI.md) through its stages on cold ferros, stopping at the first stage that does not come back.
#   00 read-only survey · 01 + PHY bring-up · 02 + link training (Gen1) · 03 + config space and the chip id
# A stage that stalls the bus ends in a watchdog reset (ferros reboots); the last printed stage is the culprit.
# Usage: tools/husky-wifi.sh [last-stage]   (default 03)
set -uo pipefail
REPO=/mnt/Harbor/Code/ferros
B=$REPO/target/debug/ferros-bridge
R=$REPO/target/aarch64-unknown-none/release
LAST=${1:-03}
( cd "$REPO" && cargo build -q -p ferros-payload-pcie1 --target aarch64-unknown-none --release && aarch64-linux-gnu-objcopy -O binary "$R/pcie1" "$R/pcie1.bin" ) || exit 1
HW="--hw pcie1-elbi --hw pcie1-soc --hw pcie1-pcs --hw pcie1-udbg --hw pcie1-phy --hw pcie1-dbi --hw pcie1-cfg --hw pcie1-mem --hw gpio-hsi2 --hw gpio-far-alive --hw cmu-hsi2 --hw pmu"
for s in 00 01 02 03; do
    echo "=== stage $s"
    timeout 120 "$B" spawn "$R/pcie1.bin" "$s" --name pcie1 $HW 2>&1 | grep -vE "DATA|SPEC|recv\[|recv:|send|Staging|Staged|COMPLETE|Connected|Signed"
    # Long waits (WL_REG_ON's 200 ms) span many time slices: the spawn returns after the first one and the driver finishes in the background. Drain its console until it exits.
    for _ in $(seq 60); do
        lsusb -d 1209:4665 >/dev/null || { echo "=== ferros dropped off USB during stage $s"; exit 1; }
        out=$("$B" diag console 2>/dev/null)
        printf '%s\n' "$out" | grep -v '^ALIVE=' | grep -v '^$'
        printf '%s\n' "$out" | grep -q '^ALIVE=00000000' && break
        sleep 1
    done
    [ "$s" = "$LAST" ] && break
done
