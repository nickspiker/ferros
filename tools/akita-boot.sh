#!/usr/bin/env bash
# The Pixel 8a (akita) dev loop (ferros_kernel/src/board.rs): the usb process as a native capsule the kernel carries (akita has no vault yet, so no init and no stored capsule), the bare kernel (no hv: it lives in husky's top RAM, past akita's 8 GB), wrapped as ABL boots it, then booted on the 8a.
# Default: `fastboot boot`, from RAM, nothing flashed. --flash-a: boot_a only, then slot a active; slot b keeps Android as the escape (ABL falls back to it once slot a's retries run out, since this ferros marks no slot).
# Every fastboot call names the 8a by serial: with husky on the same host an unnamed one can land on the wrong phone.
set -euo pipefail
REPO=/mnt/Harbor/Code/ferros
cd "$REPO"
R=target/el0/aarch64-unknown-none/release
B=target/debug/ferros-bridge
ELF=target/aarch64-unknown-none/release/ferros_kernel
IMG=target/akita/ferros_cold.img

# Only the 8a: every fastboot and adb call goes to it, and fastboot refuses a phone that is not (tools/lib/devices.sh).
. /mnt/Harbor/Code/ferros/tools/lib/devices.sh

cargo el0 -q -p ferros-payload-usb
aarch64-linux-gnu-objcopy -O binary $R/usb $R/usb.bin
cargo build -q -p ferros-bridge
"$B" capsule $R/usb.bin target/akita-usb.capsule.vsf >/dev/null
cargo build -q -p ferros_kernel --target aarch64-unknown-none --release
mkdir -p target/akita
cargo run -q -p ferros-mkimg -- boot "$ELF" --pad 16 -o "$IMG"
echo "== image: $IMG ($(stat -c%s "$IMG") bytes)"
# Flashed whole, with its own AVB footer (ferros-mkimg avb-footer; docs/BOARDS.md "The hv on akita"): a bare image leaves Google's footer at the partition's end, and once anything has buried the vbmeta it points at, ABL refuses the slot ("invalid vbmeta header"). Google's slot-b boot image sizes the partition and lends its properties (slot a's is the same size).
cargo build -q -p ferros-mkimg
target/debug/ferros-mkimg avb-footer --key /mnt/Harbor/Code/keys/avb.vsf --google-boot /mnt/Harbor/tmp/akita-boot-b-2026-11-06/boot_b.img --image "$IMG" -o "${IMG%.img}_part.img"

device_pin akita || exit 1
S=$ANDROID_SERIAL
if [ "${1:-}" = --flash-a ]; then
    fastboot -s "$S" flash boot_a "${IMG%.img}_part.img"
    fastboot -s "$S" --set-active=a
    fastboot -s "$S" reboot
else
    fastboot -s "$S" boot "$IMG"
fi
# Either ferros on USB, or (a FERROS_PROBE build, ferros_kernel/src/board.rs) the 8a back in fastboot: the boot reached the probe. Give fastboot time to drop off first.
sleep 6
for i in $(seq 60); do
    if lsusb -d 1209:4665 >/dev/null; then
        echo "== ferros on USB after $((i * 2 + 6)) s"
        exit 0
    fi
    if fastboot devices | grep -q "$S"; then
        echo "== the 8a back in fastboot after $((i * 2 + 6)) s${FERROS_PROBE:+ (probe $FERROS_PROBE reached)}"
        exit 0
    fi
    sleep 2
done
echo "== neither ferros on USB nor fastboot after 126 s" >&2
exit 1
