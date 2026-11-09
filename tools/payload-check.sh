#!/usr/bin/env bash
# A built payload image must carry ferros_rt's header ("ferrosRT" at offset 8) and its declared zeroed tail must reach the last byte of its memory: the kernel sizes the process's arc from the flat image plus that tail and puts the spawn input and the stack after it (process.rs build; owner 2026-11-09, after the display's and bt's stacks sat on their own statics). Run before storing a capsule.
# Usage: tools/payload-check.sh [name ...]   (default every built payload); exits 1 if any fails, naming it.
set -u
R=/mnt/Harbor/Code/ferros/target/el0/aarch64-unknown-none/release
T=$(mktemp)
trap 'rm -f "$T"' EXIT
names=("$@")
[ ${#names[@]} -eq 0 ] && for d in /mnt/Harbor/Code/ferros/payloads/*/; do n=$(basename "$d"); [ -f "$R/$n" ] && names+=("$n"); done
bad=0
for n in "${names[@]}"; do
    [ -f "$R/$n" ] || { echo "payload-check: $n not built"; bad=1; continue; }
    aarch64-linux-gnu-objcopy -O binary "$R/$n" "$T"
    head=$(dd if="$T" bs=1 skip=8 count=8 2>/dev/null | tr -d '\0')
    tail=$(od -An -t u8 -j 16 -N 8 "$T" | tr -d ' ')
    file=$(stat -c %s "$T")
    mem=$(aarch64-linux-gnu-readelf -lW "$R/$n" 2>/dev/null | awk '/LOAD/{e=strtonum($3)+strtonum($6); if(e>m)m=e} END{print m+0}')
    if [ "$head" != "ferrosRT" ]; then echo "payload-check: $n has no image header (ferros_sys::entry!, payload.ld __ferros_tail)"; bad=1
    elif [ $((file + tail)) -lt "$mem" ]; then echo "payload-check: $n declares $tail bytes of tail but its memory ends $((mem - file)) past the file"; bad=1; fi
done
exit $bad
