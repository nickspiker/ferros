#!/usr/bin/env bash
# ferros hv on husky (HV.md): build the EL2 layer, pack it in front of the slot-a Linux Image as one boot.img v4 kernel, and (with --flash) put it on boot_a and boot it.
# Blob = [hv flat binary padded to G#18_0000][Linux Image], LZ4-legacy compressed as ABL expects; ABL loads it at G#8008_0000, so the Image lands 2 MiB aligned at G#8020_0000.
# The Linux Image defaults to the one unpacked from the 2026-10-13 boot_a backup (the kernel Android runs today). Restore: `fastboot flash boot_a /mnt/Harbor/tmp/husky-backup-2026-10-13/boot_a.img`.
# The boot gate (ferros_hv/src/gate.rs): by default the blob also carries the cold ferros image, packed past Linux's image_size (rounded up to 2 MiB) behind a 4 KiB header {"FERROSGT", u64 length}; the hv then shows "Ready?" before Linux: power (or 1 s on dev builds) = ferros, volume down = Android, volume up = hold. --no-ferros leaves it out (the hv boots Android straight away, as before: oracle captures).
# Usage: tools/husky-hv.sh [--image <Linux Image>] [--no-ferros] [--flash]
set -eu
cd "$(dirname "$0")/.."
MKB=/mnt/Harbor/husky-kernel/tools/mkbootimg
BACKUP=/mnt/Harbor/tmp/husky-backup-2026-10-13/boot_a.img
W=target/hv; mkdir -p $W
IMAGE=""; FLASH=0; FERROS=1
while [ $# -gt 0 ]; do case $1 in --image) IMAGE=$2; shift 2;; --flash) FLASH=1; shift;; --no-ferros) FERROS=0; shift;; *) echo "unknown arg $1"; exit 2;; esac; done
if [ -z "$IMAGE" ]; then
  if [ ! -f $W/Image ]; then
    python3 $MKB/unpack_bootimg.py --boot_img $BACKUP --out $W/unpacked >/dev/null
    python3 - $W/unpacked/kernel $W/Image <<'PY'
import lz4.block, struct, sys
d=open(sys.argv[1],'rb').read(); assert d[:4]==b'\x02\x21\x4c\x18', "not LZ4 legacy"
out=bytearray(); p=4
while p+4<=len(d):
    n=struct.unpack_from('<I',d,p)[0]; p+=4
    if n==0x184C2102: continue
    out+=lz4.block.decompress(d[p:p+n],uncompressed_size=8<<20); p+=n
open(sys.argv[2],'wb').write(out)
PY
  fi
  IMAGE=$W/Image
fi
cargo build -q -p ferros_hv --target aarch64-unknown-none-softfloat --release
aarch64-linux-gnu-objcopy -O binary target/aarch64-unknown-none-softfloat/release/ferros_hv $W/hv.bin
HV=$(stat -c %s $W/hv.bin); PAD=$((0x180000))
[ $HV -le $PAD ] || { echo "hv.bin is $HV bytes, over the G#18_0000 pad"; exit 1; }
FERROS_BIN=""
if [ $FERROS = 1 ]; then
  # The cold ferros image (link base G#8008_0000), the same one slot b boots.
  cargo build -q -p ferros_kernel --target aarch64-unknown-none --release
  aarch64-linux-gnu-objcopy -O binary target/aarch64-unknown-none/release/ferros_kernel $W/ferros.bin
  FERROS_BIN=$W/ferros.bin
fi
python3 - $W/hv.bin $IMAGE $W/kernel.lz4 $PAD $FERROS_BIN <<'PY'
import lz4.block, struct, sys
hv=open(sys.argv[1],'rb').read(); img=open(sys.argv[2],'rb').read(); pad=int(sys.argv[4])
to,isz,flags=struct.unpack_from('<QQQ',img,8); assert img[56:60]==b'ARMd' and to==0 and flags&8, "Image header"
blob=hv+b'\0'*(pad-len(hv))+img
if len(sys.argv) > 5 and sys.argv[5]:
    fer=open(sys.argv[5],'rb').read()
    # gate.rs finds it at Linux's entry + image_size rounded up to 2 MiB: past Linux's bss, below ABL's ramdisks at G#8600_0000.
    at=pad+((isz+0x1FFFFF)&~0x1FFFFF)
    assert 0x80080000+at+0x1000+len(fer) < 0x86000000, "ferros image would reach ABL's ramdisks"
    blob+=b'\0'*(at-len(blob))
    blob+=(b'FERROSGT'+struct.pack('<Q',len(fer))).ljust(0x1000,b'\0')+fer
    print(f"ferros {len(fer)} B packed at G#{0x80080000+at:X}")
out=bytearray(b'\x02\x21\x4c\x18')
for i in range(0,len(blob),8<<20):
    c=lz4.block.compress(blob[i:i+(8<<20)],store_size=False,mode='high_compression')
    out+=struct.pack('<I',len(c))+c
open(sys.argv[3],'wb').write(out)
print(f"hv {len(hv)} B, Image {len(img)} B (image_size G#{isz:X}), blob {len(blob)} B, lz4 {len(out)} B")
PY
: > $W/ramdisk.empty
# HV_CMDLINE: extra kernel command line in the boot image header (ABL appends it). Oracle captures of boot-time drivers use HV_CMDLINE=loglevel=4 so the kernel's boot log stays out of the pstore console ring the captures travel through.
python3 $MKB/mkbootimg.py --header_version 4 --kernel $W/kernel.lz4 --ramdisk $W/ramdisk.empty --cmdline "${HV_CMDLINE:-}" -o $W/boot_a_hv.img
echo "== image: $W/boot_a_hv.img ($(stat -c %s $W/boot_a_hv.img) bytes)"
[ $FLASH = 1 ] || { echo "not flashing (add --flash). Then: fastboot flash boot_a $W/boot_a_hv.img; fastboot --set-active=a; fastboot reboot"; exit 0; }
# Decide from the USB bus, not from `adb devices`: phones reachable over the LAN show up there too, and then a bare `adb reboot bootloader` fails.
if lsusb -d 1209:4665 >/dev/null 2>&1; then timeout 60 target/debug/ferros-bridge reboot fastboot >/dev/null 2>&1 || true
elif lsusb | grep -q "18d1:4ee[0-9a-f]"; then adb -s "$(adb devices | awk '$2=="device" && $1 !~ /[:.]/ {print $1; exit}')" reboot bootloader || true; fi
for i in $(seq 60); do lsusb -d 18d1:4ee0 >/dev/null 2>&1 && break; sleep 2; tools/usb-kick.sh --if-wedged; done
lsusb -d 18d1:4ee0 >/dev/null 2>&1 || { echo "no fastboot device"; exit 1; }
fastboot flash boot_a $W/boot_a_hv.img 2>&1 | tail -2
fastboot --set-active=a 2>&1 | tail -1
fastboot reboot >/dev/null 2>&1
echo "== booting slot a under the hv; waiting for adb (or ferros, if the gate chose it)"
for i in $(seq 80); do sleep 3; tools/usb-kick.sh --if-wedged; if lsusb -d 1209:4665 >/dev/null 2>&1; then echo "== FERROS on USB after $((i*3)) s (the gate chose ferros)"; exit 0; fi; if lsusb | grep -q "18d1:4ee[1-9a-f]"; then echo "== ANDROID on USB after $((i*3)) s"; sleep 5; adb -s "$(adb devices | awk '$2=="device" && $1 !~ /[:.]/ {print $1; exit}')" shell 'dmesg 2>/dev/null | grep -E "started at EL"; ls /sys/firmware/devicetree/base/reserved-memory/ | grep hv' 2>/dev/null; exit 0; fi; lsusb -d 18d1:4ee0 >/dev/null 2>&1 && { echo "== back in fastboot (slot a did not boot)"; exit 1; }; done
echo "== nothing on USB after 240 s"
