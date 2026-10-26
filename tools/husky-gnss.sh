#!/usr/bin/env bash
# The GNSS driver as a process (payloads/gnss, GNSS.md "The driver"): build it and its test client, spawn it with its grants, then ask for the fix every 30 s.
# The kernel keeps only the chip's power (rails, SPI clocks, pin modes) behind the `gnss` power capability; the replay and `gnss.last` are in the vault (`gnss.` scope).
# Usage: tools/husky-gnss.sh [sky]   (sky: the replay without the LTO, the chip decoding its own orbits)
set -eu
cd "$(dirname "$0")/.."
R=target/el0/aarch64-unknown-none/release
B=target/debug/ferros-bridge
cargo el0 -q -p ferros-payload-gnss -p ferros-payload-gnsstest
cargo build -q --bin ferros-bridge
for b in gnss gnsstest; do aarch64-linux-gnu-objcopy -O binary "$R/$b" "$R/$b.bin"; done
flags=00
[ "${1:-}" = sky ] && flags=01
timeout 120 "$B" spawn "$R/gnss.bin" "$flags" --name gnss --serve gnss --hw usi3-spi --hw gpio-far-alive --hw gpio-gpp0 --hw gpio-hsi2 --store gnss --power gnss --irq gnss-host-req >/dev/null 2>&1
for i in $(seq 20); do
    sleep 30
    timeout 60 "$B" spawn "$R/gnsstest.bin" 00 --name gnsstest --use gnss >/dev/null 2>&1 || true
    timeout 20 "$B" diag console 2>&1 | grep -a "^gnss" | grep -v "exit G#0" || true
done
