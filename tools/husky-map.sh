#!/usr/bin/env bash
# The map (SHELL.md: the phone is a map of your life) on cold ferros: build, reload the kernel, then the three processes it is made of —
#   display  owns the panel and keeps the map's retained scene (payloads/display)
#   touch    serves contacts, after the kernel brings the controller up (payloads/touch)
#   map      places the circles and turns touch into the camera (payloads/map)
# Needs cold ferros on USB (slot b: tools/husky-cold.sh). Usage: tools/husky-map.sh [--no-reload]
set -uo pipefail
REPO=/mnt/Harbor/Code/ferros
B=$REPO/target/debug/ferros-bridge
K=$REPO/target/aarch64-unknown-none/release
# Processes build as EL0 (`cargo el0`, .cargo/config.toml), into their own target directory.
R=$REPO/target/el0/aarch64-unknown-none/release
RELOAD=1
for a in "$@"; do case $a in --no-reload) RELOAD=0;; esac; done
cd "$REPO" || exit 1
# The apps' icons, from each app's own codebase, in VSF RGB, bundled into the display server at build time (tools/app-icons).
(cd tools/app-icons && cargo run -q --release -- "$REPO/target/icons" >/dev/null) || exit 1
cargo build -q -p ferros_kernel --target aarch64-unknown-none --release || exit 1
cargo el0 -q -p ferros-payload-display -p ferros-payload-touch -p ferros-payload-map || exit 1
cargo build -q --bin ferros-bridge || exit 1
aarch64-linux-gnu-objcopy -O binary "$K/ferros_kernel" "$K/ferros_kernel.bin" || exit 1
for b in display touch map; do aarch64-linux-gnu-objcopy -O binary "$R/$b" "$R/$b.bin" || exit 1; done
for _ in $(seq 60); do lsusb -d 1209:4665 >/dev/null 2>&1 && break; sleep 2; done
lsusb -d 1209:4665 >/dev/null 2>&1 || { echo "ferros is not on USB (cold ferros on slot b: tools/husky-cold.sh)"; exit 1; }
if [ $RELOAD = 1 ]; then
    timeout 300 "$B" reload "$K/ferros_kernel.bin" >/dev/null 2>&1
    sleep 4
    for _ in $(seq 20); do lsusb -d 1209:4665 >/dev/null 2>&1 && break; sleep 2; done
fi
echo "== the panel"
# Grant slots are positional (payloads/display/src/main.rs header).
timeout 120 "$B" spawn "$R/display.bin" 00 --name display --serve display --hw decon0-main --hw decon0-win --hw decon0-wincon --hw dpu-dma0 --hw dpp0 --hw gpio-gpp0 --dma display-fb --dma display-heap --hw dsim0 --hw dphy0 --hw sramc0 --hw hdrcomm0 2>&1 | grep -E "^display|rror"
sleep 3
echo "== touch"
timeout 60 "$B" diag "touch up" 2>&1 | grep -E "chip ID|scan mode" | head -2
timeout 120 "$B" spawn "$R/touch.bin" 00 --name touch --serve touch --hw usi2-spi --hw gpio-gpn0 2>&1 | grep -E "^touch|rror"
sleep 2
echo "== the map"
timeout 120 "$B" spawn "$R/map.bin" 00 --name map --use display --use touch 2>&1 | grep -E "^map|rror"
sleep 2
"$B" diag console 2>/dev/null | grep -E "^display|^touch|^map" | tail -8
