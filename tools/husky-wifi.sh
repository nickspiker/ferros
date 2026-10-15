#!/usr/bin/env bash
# Run the pcie1 driver process (payloads/pcie1, WIFI.md) through its stages on cold ferros, stopping at the first stage that does not come back.
#   00 read-only survey · 01 + PHY bring-up · 02 + link training (Gen1) · 03 + config space and the chip id  · 04 + NCI ROM dump · 05 + BAR sizes, security status, CA7 state (read-only)  · 06 + RAM size, BAR1, write/read round trip in dongle RAM · 07 + firmware download and start
# A stage that stalls the bus ends in a watchdog reset (ferros reboots); the last printed stage is the culprit.
# Usage: tools/husky-wifi.sh [last-stage] [first-stage]   (default 03; each stage redoes every earlier one, so a first-stage skips only the earlier runs' output)
set -uo pipefail
REPO=/mnt/Harbor/Code/ferros
B=$REPO/target/debug/ferros-bridge
R=$REPO/target/aarch64-unknown-none/release
LAST=${1:-03}
FIRST=${2:-00}
# Stage 07's blobs (tools/husky-firmware.sh fetches them from the phone, firmware/husky/MANIFEST.md pins them). Chip rev 3 = d0 (dhd_custom_cis.c chip_revs); the NVRAM is the module's (OTP CIS VID → vid_naming_table_4398; V17 = cal_ROW/JP/MMW byte for byte, cal_NA differs only in FTM ranging calibration).
FWDIR=${FWDIR:-/mnt/Harbor/firmware/husky}
FW=${FW:-$FWDIR/fw_bcmdhd.bin_4398_d0}
NVRAM=${NVRAM:-$FWDIR/bcmdhd.cal_USI_G5SN_9957_V17}
( cd "$REPO" && cargo build -q -p ferros-payload-pcie1 --target aarch64-unknown-none --release && aarch64-linux-gnu-objcopy -O binary "$R/pcie1" "$R/pcie1.bin" ) || exit 1
HW="--hw pcie1-elbi --hw pcie1-soc --hw pcie1-pcs --hw pcie1-udbg --hw pcie1-phy --hw pcie1-dbi --hw pcie1-cfg --hw pcie1-mem --hw gpio-hsi2 --hw gpio-far-alive --hw cmu-hsi2 --hw pmu"
for s in 00 01 02 03 04 05 06 07; do
    [[ "$s" < "$FIRST" ]] && continue
    echo "=== stage $s"
    EXTRA=()
    [ "$s" = 07 ] && EXTRA=(--entropy --input-file "$FW" --input-file "$NVRAM")
    timeout 120 "$B" spawn "$R/pcie1.bin" "$s" --name pcie1 $HW "${EXTRA[@]}" 2>&1 | grep -vE "DATA|SPEC|recv\[|recv:|send|Staging|Staged|COMPLETE|Connected|Signed"
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
