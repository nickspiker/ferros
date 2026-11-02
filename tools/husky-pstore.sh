#!/usr/bin/env bash
# The tail of Android's pstore console ring (ramoops, G#FD3F_F000, 2 MiB: "DBGC", start, size, then the ring), read from a running ferros over the bridge: the hv's lines and ferros's own console (platform::pstore), which survive a warm reset. After a session that died before USB (a jump from Android that ended in a watchdog), the next ferros boot shows what it said.
# Usage: tools/husky-pstore.sh [bytes, default 32768]
set -uo pipefail
B=/mnt/Harbor/Code/ferros/target/debug/ferros-bridge
WANT=${1:-32768}
ZONE=$((0xFD3FF000))
BUF=$((0x200000 - 12))
hdr=$(timeout 30 "$B" read FD3FF000 10 2>&1 | grep "^FD3F" | head -1)
[ -n "$hdr" ] || { echo "no ring header read"; exit 1; }
set -- $hdr
[ "$2$3$4$5" = "44424743" ] || { echo "no DBGC signature: $hdr"; exit 1; }
start=$((0x$9$8$7$6))
size=$((0x${13}${12}${11}${10}))
n=$(( WANT < size ? WANT : size ))
from=$(( ((start - n + BUF) % BUF) / 16 * 16 ))
out=""
read_span() {
    local off=$1 len=$2
    while [ "$len" -gt 0 ]; do
        local chunk=$(( len < 0x8000 ? len : 0x8000 ))
        timeout 60 "$B" read "$(printf "%X" $((ZONE + 12 + off)))" "$(printf "%X" $chunk)" 2>&1 | grep -E "^FD[0-9A-F]{6} "
        off=$((off + chunk)); len=$((len - chunk))
    done
}
{
    if [ $((from + n)) -le $BUF ]; then
        read_span "$from" "$n"
    else
        read_span "$from" $((BUF - from))
        read_span 0 $((n - (BUF - from)))
    fi
} | awk '{for(i=2;i<=17;i++){c=strtonum("0x"$i); if(c==10||(c>=32&&c<127)) printf "%c", c}}'
echo
