#!/usr/bin/env bash
# Carve a `ferros` partition out of a Pixel's userdata (tools/flash, ferros-flash): userdata keeps its start and shrinks to <userdata-GiB>, ferros takes the rest. Surgical: only the two headers and the entry blocks that change are written (never a clear-and-recreate, which loses the A/B slot attributes and bricks the phone).
# Needs the phone in Android with root (Magisk, adb's Shell granted). Steps: both ends of LU 0 (sda) read and saved with their hashes, both checked on the device too; the carve planned and read back from the images; the changed blocks written; read back from the device and compared. Then to the bootloader and `fastboot -w`: userdata's filesystem is the old size and must go (it formats itself on the next Android boot). Everything on userdata is lost; the per-unit partitions (persist, efs, ...) are not touched.
# usage: tools/pixel-carve.sh <product> <userdata-GiB>
set -euo pipefail
cd /mnt/Harbor/Code/ferros
PRODUCT=${1:?product (akita, husky)}
GIB=${2:?userdata size in GiB}
. tools/lib/devices.sh
device_pin "$PRODUCT" || exit 1
S=$ANDROID_SERIAL
su_do() { adb -s "$S" shell "su -c '$1'" </dev/null; }

[ "$(adb -s "$S" shell getprop ro.product.device | tr -d '\r')" = "$PRODUCT" ] || { echo "not $PRODUCT in Android: refused" >&2; exit 1; }
su_do id | grep -q "uid=0" || { echo "no root over adb (grant Shell in Magisk)" >&2; exit 1; }
BLOCK=$(su_do "blockdev --getss /dev/block/sda" | tr -d '\r')
BYTES=$(su_do "blockdev --getsize64 /dev/block/sda" | tr -d '\r')
TAIL=$((BYTES / BLOCK - 16))
D=/mnt/Harbor/tmp/$PRODUCT-gpt-$(date +%F-%H%M%S)
mkdir -p "$D"
su_do "dd if=/dev/block/sda bs=$BLOCK count=16 2>/dev/null" > "$D/head.bin"
su_do "dd if=/dev/block/sda bs=$BLOCK skip=$TAIL count=16 2>/dev/null" > "$D/tail.bin"
for f in head:0 tail:$TAIL; do
    n=${f%%:*} at=${f##*:}
    want=$(sha256sum "$D/$n.bin" | cut -c1-64)
    got=$(su_do "dd if=/dev/block/sda bs=$BLOCK skip=$at count=16 2>/dev/null | sha256sum" | cut -c1-64)
    [ "$want" = "$got" ] || { echo "$n: the copy on the host does not match the device" >&2; exit 1; }
done
(cd "$D" && sha256sum head.bin tail.bin > SHA256SUMS)
echo "== the table as it is (saved in $D):"
target/debug/ferros-flash show "$D/head.bin" "$D/tail.bin" "$TAIL" "$BLOCK"

cargo build -q -p ferros-flash
PLAN=$(target/debug/ferros-flash carve "$D/head.bin" "$D/tail.bin" "$TAIL" "$GIB" "$D/carved" "$BLOCK")
echo "== the carved table:"
target/debug/ferros-flash show "$D/carved/head.bin" "$D/carved/tail.bin" "$TAIL" "$BLOCK"

adb -s "$S" push "$D/carved/head.bin" /data/local/tmp/ferros-head.bin >/dev/null
adb -s "$S" push "$D/carved/tail.bin" /data/local/tmp/ferros-tail.bin >/dev/null
# The primary first, the backup last (lib.rs `writes`).
while read -r _ lba n; do
    if [ "$lba" -lt "$TAIL" ]; then f=/data/local/tmp/ferros-head.bin off=$lba; else f=/data/local/tmp/ferros-tail.bin off=$((lba - TAIL)); fi
    echo "== write LBA $lba, $n blocks"
    su_do "dd if=$f of=/dev/block/sda bs=$BLOCK skip=$off seek=$lba count=$n conv=notrunc,fsync 2>/dev/null"
done <<< "$PLAN"
su_do "sync"
for f in head:0 tail:$TAIL; do
    n=${f%%:*} at=${f##*:}
    want=$(sha256sum "$D/carved/$n.bin" | cut -c1-64)
    got=$(su_do "blockdev --flushbufs /dev/block/sda; echo 3 > /proc/sys/vm/drop_caches; dd if=/dev/block/sda bs=$BLOCK skip=$at count=16 2>/dev/null | sha256sum" | cut -c1-64)
    [ "$want" = "$got" ] || { echo "$n: the device does not read back the carved table; the original is in $D" >&2; exit 1; }
done
su_do "rm /data/local/tmp/ferros-head.bin /data/local/tmp/ferros-tail.bin"
echo "== written and read back; to the bootloader"
adb -s "$S" reboot bootloader
for i in $(seq 60); do fastboot devices | grep -q "$S" && break; sleep 2; done
fastboot getvar partition-size:userdata 2>&1 | grep partition-size
fastboot getvar partition-size:ferros 2>&1 | grep partition-size
fastboot -w
echo "== done: userdata wiped, Android formats it on its next boot"
