#!/bin/bash
# Build a std program for the phone (STD-TARGET.md): std from the ferros-rust fork, the ferros target, then the flat image. Usage: tools/std-build.sh <crate dir> [cargo args]; the image lands at <crate dir>/target/aarch64-unknown-ferros/release/<bin>.bin
set -e
dir=$1; shift
cd "$dir"
export __CARGO_TESTS_ONLY_SRC_ROOT=/mnt/Harbor/Code/ferros-rust/library
cargo +nightly build --release -Zjson-target-spec "$@"
out=target/aarch64-unknown-ferros/release
for elf in $(find $out -maxdepth 1 -type f -executable); do
    aarch64-linux-gnu-objcopy -O binary "$elf" "$elf.bin"
    echo "$elf.bin: $(stat -c %s "$elf.bin") bytes"
done
