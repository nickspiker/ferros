#!/usr/bin/env bash
# Bring up ferros's network stack on husky, three processes over capabilities: the WiFi driver serving Ethernet frames (payloads/pcie1 stage 14, endpoint wlan), the IP process (payloads/ip: ARP, IPv4 + DHCP, IPv6 NDP/SLAAC, UDP; serves udp), and a UDP echo app on port 7777 (payloads/nettest).
# Then it keeps draining the processes' console into a log (and the terminal) so you can ping / ping -6 / `echo hi | nc -u -w1 <addr> 7777` from another machine.
# Usage: tools/husky-net.sh [seconds to keep draining, default 600]   (log: target/husky-net.log)
# Never pipe this into head: killing the bridge mid-transfer makes ferros take its PT-stall exit.
set -uo pipefail
REPO=/mnt/Harbor/Code/ferros
B=$REPO/target/debug/ferros-bridge
R=$REPO/target/aarch64-unknown-none/release
LOG=$REPO/target/husky-net.log
KEEP=${1:-600}
FWDIR=${FWDIR:-/mnt/Harbor/firmware/husky}
FW=${FW:-$FWDIR/fw_bcmdhd.bin_4398_d0}
NVRAM=${NVRAM:-$FWDIR/bcmdhd.cal_USI_G5SN_9957_V17}
CLM=${CLM:-$FWDIR/bcmdhd_clm.blob_4398_d0}
CRED=${CRED:-/mnt/Harbor/Code/keys/wifi.txt}
( cd "$REPO" && cargo build -q -p ferros-payload-pcie1 -p ferros-payload-ip -p ferros-payload-nettest --target aarch64-unknown-none --release ) || exit 1
for b in pcie1 ip nettest; do aarch64-linux-gnu-objcopy -O binary "$R/$b" "$R/$b.bin" || exit 1; done
# MASKED=1: run ip and nettest as sandboxed images (tools/ferros-masked-build.sh; the checker accepted every instruction).
IPB=$R/ip.bin; NTB=$R/nettest.bin; IPFLAG=; NTFLAG=
if [ "${MASKED:-0}" = 1 ]; then
    for p in ip nettest; do "$REPO/tools/ferros-masked-build.sh" $p >/dev/null || exit 1; done
    IPB=$REPO/target/masked/ip/ip.bin; NTB=$REPO/target/masked/nettest/nettest.bin
    IPFLAG="--masked --text-len $(cat $REPO/target/masked/ip/ip.textlen)"
    NTFLAG="--masked --text-len $(cat $REPO/target/masked/nettest/nettest.textlen)"
fi
: > "$LOG"
drain() { "$B" diag console 2>/dev/null | grep -v '^ALIVE=' | grep -v '^$' | tee -a "$LOG"; }
HW="--hw pcie1-elbi --hw pcie1-soc --hw pcie1-pcs --hw pcie1-udbg --hw pcie1-phy --hw pcie1-dbi --hw pcie1-cfg --hw pcie1-mem --hw gpio-hsi2 --hw gpio-far-alive --hw cmu-hsi2 --hw pmu"
echo "=== wifi driver (stage 14: firmware, join, serve wlan)"
timeout 120 "$B" spawn "$R/pcie1.bin" 14 --name pcie1 $HW --entropy --dma pcie1-dma --serve wlan --input-file "$FW" --input-file "$NVRAM" --input-file "$CLM" --input-file "$CRED" >/dev/null 2>&1
for _ in $(seq 90); do
    drain >/dev/null
    grep -q "serving wlan" "$LOG" && break
    grep -q "pcie1: exit" "$LOG" && { echo "=== driver exited"; grep "pcie1:" "$LOG" | tail -20; exit 1; }
    sleep 1
done
grep -q "serving wlan" "$LOG" || { echo "=== driver never served"; tail -20 "$LOG"; exit 1; }
grep -E "joining|KEYED|join failed|allmulti|offload|flags2" "$LOG"
echo "=== ip"
timeout 60 "$B" spawn "$IPB" --name ip --use wlan --serve udp --entropy $IPFLAG >/dev/null 2>&1
echo "=== nettest (UDP echo on 7777)"
timeout 60 "$B" spawn "$NTB" --name nettest --use udp $NTFLAG >/dev/null 2>&1
end=$((SECONDS + KEEP))
while [ $SECONDS -lt $end ]; do
    lsusb -d 1209:4665 >/dev/null || { echo "=== ferros dropped off USB"; exit 1; }
    drain | grep -E "^(ip|nettest|pcie1): " | grep -v "rx frame"
    sleep 1
done
