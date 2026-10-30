#!/usr/bin/env bash
# Start the cellular modem's driver process on husky (payloads/modem): it brings the CP up over PCIe ch0, registers, sets up the internet data call and serves its packets on `rmnet` to the IP process. Then it drains the console for a while so the bring-up can be read.
# Usage: tools/husky-modem.sh [input] [seconds to drain, default 60]   (input: empty = up and serve; sN = staged diagnostic N, then exit)
set -uo pipefail
REPO=/mnt/Harbor/Code/ferros
B=$REPO/target/debug/ferros-bridge
R=$REPO/target/el0/aarch64-unknown-none/release
IN=${1:-}
KEEP=${2:-60}
( cd "$REPO" && cargo el0 -q -p ferros-payload-modem 2>&1 | grep -E "^error" -A10 ) && exit 1
aarch64-linux-gnu-objcopy -O binary "$R/modem" "$R/modem.bin" || exit 1
HW=""
for h in pcie0-elbi pcie0-soc pcie0-pcs pcie0-udbg pcie0-phy pcie0-dbi pcie0-cfg pcie0-mem cmu-hsi1 hsi1-sysreg gpio-hsi1 sysmmu-hsi1 s2mpu-hsi1 gpio-alive pmu; do HW="$HW --hw $h"; done
PARTS=""
for p in modem_a efs persist modem_userdata vendor_a; do PARTS="$PARTS --part $p"; done
HEX=$(printf '%s' "$IN" | xxd -p | tr -d '\n')
timeout 60 "$B" spawn "$R/modem.bin" $HEX --name modem --serve rmnet $HW --dma modem-pktproc --dma modem-ipc --dma modem-msi $PARTS >/dev/null 2>&1
end=$(( $(date +%s) + KEEP ))
while [ "$(date +%s)" -lt "$end" ]; do
    timeout 20 "$B" diag console 2>/dev/null | grep -a "^modem: \|^ip: " || true
    sleep 2
done
