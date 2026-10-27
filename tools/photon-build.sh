#!/bin/bash
# Photon for the phone as a ferros process (STD-TARGET.md): std from the ferros-rust fork, the ferros target, a development build, then the flat image the kernel loads. The build is the std/photon package (Photon's library as a dependency, its own lockfile and patch table), so Photon's manifest, lock and config are not touched. Pass extra cargo arguments after the script name.
set -e
here=$(cd "$(dirname "$0")/.." && pwd)
export __CARGO_TESTS_ONLY_SRC_ROOT=/mnt/Harbor/Code/ferros-rust/library
export CC_aarch64_unknown_ferros=aarch64-linux-gnu-gcc
export AR_aarch64_unknown_ferros=aarch64-linux-gnu-ar
export CFLAGS_aarch64_unknown_ferros="-ffreestanding -fno-stack-protector -fno-builtin -O2 -isystem $here/tools/std-sysroot/include"
export CARGO_TARGET_AARCH64_UNKNOWN_FERROS_RUSTFLAGS='--cfg getrandom_backend="custom"'
cd "$here/std/photon"
cargo +nightly build -Zjson-target-spec -Zbuild-std=core,alloc,std,panic_abort -Zbuild-std-features=compiler-builtins-mem --target "$here/targets/aarch64-unknown-ferros.json" "$@"
out="$here/std/photon/target/aarch64-unknown-ferros/debug/photon"
aarch64-linux-gnu-objcopy -O binary "$out" "$out.bin" && echo "$out.bin: $(stat -c %s "$out.bin") bytes"
