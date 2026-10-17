#!/usr/bin/env bash
# Bring the display server up on cold ferros (slot b): build the payloads, reload the kernel, spawn the server with its grants, then the home-screen client.
# Usage: tools/husky-display.sh [--no-reload] [--no-shell]
set -uo pipefail
REPO=/mnt/Harbor/Code/ferros
B=$REPO/target/debug/ferros-bridge
R=$REPO/target/aarch64-unknown-none/release
RELOAD=1; SHELL_CLIENT=1
for a in "$@"; do case $a in --no-reload) RELOAD=0;; --no-shell) SHELL_CLIENT=0;; esac; done
cd "$REPO" || exit 1
cargo build -q -p ferros_kernel -p ferros-payload-display -p ferros-payload-shelltest --target aarch64-unknown-none --release || exit 1
for b in ferros_kernel display shelltest; do aarch64-linux-gnu-objcopy -O binary "$R/$b" "$R/$b.bin" || exit 1; done
if lsusb -d 18d1:4ee0 >/dev/null 2>&1; then echo "phone in fastboot: rebooting into ferros"; timeout 30 fastboot reboot >/dev/null 2>&1; fi
for _ in $(seq 60); do lsusb -d 1209:4665 >/dev/null 2>&1 && break; sleep 2; done
lsusb -d 1209:4665 >/dev/null 2>&1 || { echo "ferros is not on USB"; exit 1; }
# Before the reload: the kernel found running says why the previous one left (the flashed kernel only knows PREV_EXIT; the reloaded one also prints the PT record).
echo "found: $("$B" diag 2>/dev/null | grep -E "^GEN|^PREV_EXIT|^PREV_PT_CMD_CAP|^PREV_ALIVE" | tr '\n' ' ')"
if [ $RELOAD = 1 ]; then
    timeout 300 "$B" reload "$R/ferros_kernel.bin" >/dev/null 2>&1
    sleep 4
    for _ in $(seq 20); do lsusb -d 1209:4665 >/dev/null 2>&1 && break; sleep 2; done
fi
"$B" diag 2>/dev/null | grep -E "^GEN|^PREV_EXIT"
# Grant slots are positional (payloads/display/src/main.rs header): console, endpoint, DECON main/win/wincon, DMA0, DPP0, GPP0, framebuffer, heap, DSIM0, DPHY0, SRAMC0, HDRCOMM0.
timeout 120 "$B" spawn "$R/display.bin" 00 --name display --serve display --hw decon0-main --hw decon0-win --hw decon0-wincon --hw dpu-dma0 --hw dpp0 --hw gpio-gpp0 --dma display-fb --dma display-heap --hw dsim0 --hw dphy0 --hw sramc0 --hw hdrcomm0 2>&1 | grep -E "^display|rror"
sleep 3
if [ $SHELL_CLIENT = 1 ]; then
    timeout 120 "$B" spawn "$R/shelltest.bin" 00 --name shelltest --use display >/dev/null 2>&1
    sleep 3
fi
"$B" diag console 2>/dev/null | grep -E "^display|^shelltest"
echo "panel DBV: $("$B" display read 52 2 2>/dev/null | grep -oE 'G#52: .*')"
