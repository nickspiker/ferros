#!/usr/bin/env bash
# Bring the touch server up and prove the `touch` endpoint: build, reload the kernel, bring the controller up from the kernel (rails over ACPM, reset, scan mode), spawn the server with its two MMIO grants, then the test client.
# The kernel owns bring-up on purpose, so the server never needs a privileged path; see payloads/touch/src/main.rs and docs/oracle/TOUCH.md.
# Usage: tools/husky-touch.sh
set -eu
cd "$(dirname "$0")/.."
K=target/aarch64-unknown-none/release
# Processes build as EL0 (`cargo el0`, .cargo/config.toml), into their own target directory.
R=target/el0/aarch64-unknown-none/release
B=target/debug/ferros-bridge
cargo build -q -p ferros_kernel --target aarch64-unknown-none --release
cargo el0 -q -p ferros-payload-touch -p ferros-payload-touchtest
cargo build -q --bin ferros-bridge
aarch64-linux-gnu-objcopy -O binary "$K/ferros_kernel" "$K/ferros_kernel.bin"
for b in touch touchtest; do aarch64-linux-gnu-objcopy -O binary "$R/$b" "$R/$b.bin"; done
timeout 120 "$B" reload "$K/ferros_kernel" 2>&1 | tail -1
for i in $(seq 20); do sleep 1; lsusb -d 1209:4665 >/dev/null 2>&1 && break; done
echo "== the controller, from the kernel"
timeout 60 "$B" diag "touch up" 2>&1 | tail -3
echo "== the server"
timeout 120 "$B" spawn "$R/touch.bin" 00 --name touch --serve touch --hw usi2-spi --hw gpio-gpn0 2>&1 | grep -E "^touch|rror" || true
echo "== the client (touch the screen now, 8 s)"
timeout 120 "$B" spawn "$R/touchtest.bin" 00 --name touchtest --use touch 2>&1 | grep -viE "spec|data pack|recv|complete|sending|sent ok|blasted" || true
