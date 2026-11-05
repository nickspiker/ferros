#!/usr/bin/env bash
# The GPU driver as a process (payloads/gpu, GPU.md): build it, spawn it with its grants, print what it said. It boots the CSF microcontroller, runs its test jobs (command-stream instructions, then a compute shader) and ends.
# Usage: tools/husky-gpu.sh [show]   (show: the GPU renders the Mandelbrot set and it stays on the glass for 60 s, over everything)
set -eu
cd "$(dirname "$0")/.."
R=target/el0/aarch64-unknown-none/release
B=target/release/ferros-bridge
[ -x "$B" ] || B=target/debug/ferros-bridge
cargo el0 -q -p ferros-payload-gpu
aarch64-linux-gnu-objcopy -O binary "$R/gpu" "$R/gpu.bin"
if [ "${1:-}" = show ]; then
    timeout 120 "$B" spawn "$R/gpu.bin" 01 --name gpu --dma gpu-regs --dma gpu-mem --store gpu --irq gpu-job --use display --dma surface >/dev/null 2>&1 || true
else
    timeout 120 "$B" spawn "$R/gpu.bin" --name gpu --dma gpu-regs --dma gpu-mem --store gpu --irq gpu-job >/dev/null 2>&1 || true
fi
sleep 2
timeout 30 "$B" diag console 2>&1 | grep -a "^gpu: " | grep -av "section G#" | tail -24
