#!/usr/bin/env bash
# The NFC and Bluetooth drivers as processes (docs/NFC.md, docs/BT.md): build, spawn with their grants, and poke their endpoints by hand through epcall.
# The kernel keeps only the chips' power behind the `nfc` and `bt` power capabilities (kernel nfc.rs, bt.rs).
# Usage:
#   tools/husky-radio.sh nfc                 spawn the NFC driver (listening as Photon's pairing tag)
#   tools/husky-radio.sh bt [16-byte hex]    spawn the Bluetooth driver (the patch from vendor_a), advertising the beacon if given
#   tools/husky-radio.sh call <nfc|bt> <hex> one call to the endpoint (first byte the op; the drivers' module docs list them)
#   tools/husky-radio.sh kill <nfc|bt>       stop the driver (bt: the beacon stopped first, the chip left powered)
set -eu
cd "$(dirname "$0")/.."
R=target/el0/aarch64-unknown-none/release
B=target/debug/ferros-bridge
console() { sleep "$1"; timeout 20 "$B" diag console 2>&1 | grep -a "^$2: " | tail -"${3:-12}" || true; }
call() {
    cargo el0 -q -p ferros-payload-epcall 2>/dev/null
    aarch64-linux-gnu-objcopy -O binary "$R/epcall" "$R/epcall.bin"
    timeout 60 "$B" spawn "$R/epcall.bin" "$2" --name epcall --use "$1" >/dev/null 2>&1 || true
    sleep 2; timeout 20 "$B" diag console 2>&1 | grep -a "^\(epcall\|$1\): " | tail -12 || true
}
case "${1:-}" in
    nfc)
        cargo el0 -q -p ferros-payload-nfc 2>/dev/null
        aarch64-linux-gnu-objcopy -O binary "$R/nfc" "$R/nfc.bin"
        timeout 60 "$B" spawn "$R/nfc.bin" --name nfc --serve nfc --hw hsi2c0 --hw gpio-alive --power nfc >/dev/null 2>&1
        console 3 nfc ;;
    bt)
        cargo el0 -q -p ferros-payload-bt 2>/dev/null
        aarch64-linux-gnu-objcopy -O binary "$R/bt" "$R/bt.bin"
        timeout 60 "$B" spawn "$R/bt.bin" ${2:-} --name bt --serve bt --hw apm-usi1-uart --part vendor_a --power bt --entropy >/dev/null 2>&1
        console 6 bt ;;
    call) call "$2" "$3" ;;
    kill)
        [ "$2" = bt ] && call bt 02
        timeout 30 "$B" diag "kill $2" 2>&1 | grep -a "killed" || true ;;
    *) sed -n '2,8p' "$0"; exit 1 ;;
esac
