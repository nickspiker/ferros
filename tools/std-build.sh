#!/bin/bash
# Build a std program for the phone (STD-TARGET.md): std from the ferros-rust fork, the ferros target, then the flat image. Usage: tools/std-build.sh <crate dir> [cargo args]; the image lands at <crate dir>/target/aarch64-unknown-ferros/release/<bin>.bin
set -e
dir=$1; shift
cd "$dir"
export __CARGO_TESTS_ONLY_SRC_ROOT=/mnt/Harbor/Code/ferros-rust/library
# C in the dependency graph (blake3's NEON kernels, the PQClean suites) cross-compiles with the GNU aarch64 toolchain, freestanding: the cc crate knows no "ferros" OS.
export CC_aarch64_unknown_ferros=aarch64-linux-gnu-gcc
export AR_aarch64_unknown_ferros=aarch64-linux-gnu-ar
export CFLAGS_aarch64_unknown_ferros="-ffreestanding -fno-stack-protector -fno-builtin -O2 -isystem /mnt/Harbor/Code/ferros/tools/std-sysroot/include"
cargo +nightly build --release -Zjson-target-spec "$@"
out=target/aarch64-unknown-ferros/release
for elf in $(find $out -maxdepth 1 -type f -executable); do
    aarch64-linux-gnu-objcopy -O binary "$elf" "$elf.bin"
    echo "$elf.bin: $(stat -c %s "$elf.bin") bytes"
done
