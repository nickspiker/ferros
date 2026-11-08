#!/bin/bash
# Mahere for a ferros device as a ferros process, a kist (STD-TARGET.md): Mahere's touch-device app (mahere-device, the app its Android build runs, over the same crates as its desktop) under its ferros shell (mahere-ferros) on the ferros target, std from the ferros-rust fork, a development build, then the flat image the kernel loads. Mahere's manifests carry the cfg(target_os = "ferros") arms; this script adds only what a published crate cannot say yet: fluor and kete from their source trees (fluor's host-ferros and kete's ferros vault are not published), libc with a ferros arm, and a lockfile of the ferros build's own (mahere/ferros/Cargo.lock) so Mahere's Cargo.lock stays as its desktop and Android builds want it. Pass extra cargo arguments after the script name.
set -e
here=$(cd "$(dirname "$0")/.." && pwd)
code=$(cd "$here/.." && pwd)
mahere="$code/mahere"
export __CARGO_TESTS_ONLY_SRC_ROOT="$code/ferros-rust/library"
export CC_aarch64_unknown_ferros=aarch64-linux-gnu-gcc
export AR_aarch64_unknown_ferros=aarch64-linux-gnu-ar
export CFLAGS_aarch64_unknown_ferros="-ffreestanding -fno-stack-protector -fno-builtin -O2 -isystem $here/tools/std-sysroot/include"
export CARGO_TARGET_AARCH64_UNKNOWN_FERROS_RUSTFLAGS='--cfg getrandom_backend="custom"'
export CARGO_TARGET_DIR="$mahere/target/ferros"
cd "$mahere"
cargo +nightly build -Zjson-target-spec -Zbuild-std=core,alloc,std,panic_abort -Zbuild-std-features=compiler-builtins-mem \
    --target "$here/targets/aarch64-unknown-ferros.json" \
    --config "resolver.lockfile-path='$mahere/ferros/Cargo.lock'" \
    --config "patch.crates-io.fluor.path='$code/fluor'" \
    --config "patch.crates-io.kete.path='$code/kete'" \
    --config "patch.crates-io.libc.path='$code/ferros-libc'" \
    -p mahere-ferros --features fluor/host-ferros "$@"
out="$CARGO_TARGET_DIR/aarch64-unknown-ferros/debug/mahere"
aarch64-linux-gnu-objcopy -O binary "$out" "$out.bin" && echo "$out.bin: $(stat -c %s "$out.bin") bytes"
