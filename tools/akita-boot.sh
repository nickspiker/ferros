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

# The 8a's serial: whichever device in fastboot says product akita.
akita() {
    for s in $(fastboot devices | awk '{print $1}'); do
        if [ "$(fastboot -s "$s" getvar product 2>&1 | awk '/^product:/{print $2}')" = akita ]; then
            echo "$s"
            return
        fi
    done
}

cargo el0 -q -p ferros-payload-usb
aarch64-linux-gnu-objcopy -O binary $R/usb $R/usb.bin
cargo build -q -p ferros-bridge
"$B" capsule $R/usb.bin target/akita-usb.capsule.vsf >/dev/null
cargo build -q -p ferros_kernel --target aarch64-unknown-none --release
mkdir -p target/akita
cargo run -q -p ferros-mkimg -- boot "$ELF" --pad 16 -o "$IMG"
echo "== image: $IMG ($(stat -c%s "$IMG") bytes)"

S=$(akita)
if [ -z "$S" ]; then
    echo "no akita in fastboot" >&2
    exit 1
fi
if [ "${1:-}" = --flash-a ]; then
    fastboot -s "$S" flash boot_a "$IMG"
    fastboot -s "$S" --set-active=a
    fastboot -s "$S" reboot
else
    fastboot -s "$S" boot "$IMG"
fi
for i in $(seq 45); do
    if lsusb -d 1209:4665 >/dev/null; then
        echo "== ferros on USB after $((i * 2)) s"
        exit 0
    fi
    sleep 2
done
echo "== ferros not on USB after 90 s" >&2
exit 1
