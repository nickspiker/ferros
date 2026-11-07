#!/usr/bin/env bash
# The GPS driver as a process (payloads/gps, GPS.md "The driver"): build it and its test client, spawn it with its grants, then ask for the fix every 30 s.
# The kernel keeps only the chip's power (rails, SPI clocks, pin modes) behind the `gps` power capability; the replay and `gps.last` are in the vault (`gps.` scope).
# Usage: tools/husky-gps.sh [sky]   (sky: the replay without the LTO, the chip decoding its own orbits)
set -eu
cd "$(dirname "$0")/.."
R=target/el0/aarch64-unknown-none/release
B=target/debug/ferros-bridge
cargo el0 -q -p ferros-payload-gps -p ferros-payload-gpstest
cargo build -q --bin ferros-bridge
for b in gps gpstest; do aarch64-linux-gnu-objcopy -O binary "$R/$b" "$R/$b.bin"; done
flags=00
[ "${1:-}" = sky ] && flags=01
timeout 120 "$B" spawn "$R/gps.bin" "$flags" --name gps --serve gps --hw usi3-spi --hw gpio-far-alive --hw gpio-gpp0 --hw gpio-hsi2 --store gps --power gps --irq gps-host-req >/dev/null 2>&1
for i in $(seq 20); do
    sleep 30
    timeout 60 "$B" spawn "$R/gpstest.bin" 00 --name gpstest --use gps >/dev/null 2>&1 || true
    timeout 20 "$B" diag console 2>&1 | grep -a "^gps" | grep -v "exit G#0" || true
done
