#!/bin/bash
# Build a std program for the phone (STD-TARGET.md): std from the ferros-rust fork, the ferros target, then the flat image. Usage: tools/std-build.sh <crate dir> [cargo args]; the image lands at <crate dir>/target/aarch64-unknown-ferros/release/<bin>.bin, or under CARGO_TARGET_DIR when that is set.
# FERROS_STD_SRC names another copy of the fork's library: a worktree's ferros_rt is built only by a copy whose std/Cargo.toml points at it (the fork names the main checkout's ferros_rt by path).
set -e
dir=$1; shift
cd "$dir"
export __CARGO_TESTS_ONLY_SRC_ROOT=${FERROS_STD_SRC:-/mnt/Harbor/Code/ferros-rust/library}
# C in the dependency graph (blake3's NEON kernels, the PQClean suites) cross-compiles with the GNU aarch64 toolchain, freestanding: the cc crate knows no "ferros" OS.
export CC_aarch64_unknown_ferros=aarch64-linux-gnu-gcc
export AR_aarch64_unknown_ferros=aarch64-linux-gnu-ar
# No branch protection in the C: Fedora's cross gcc defaults to pac-ret, and its PACIASP/AUTIASP would sign with the pointer-authentication keys, which hold shares of the ira and wairua (the kernel keeps them switched off for EL0 as well, cachemap.rs).
# The registers the target reserves (targets/aarch64-unknown-ferros.json: x24 to x28; x28 the arc base and x27 its mask, which ferros_rt reads) are reserved in the C too (2026-11-17, the Pro: gcc used x27 and x28 freely in PQClean, so a C KEM calling back into Rust for its random bytes made ferros_rt read a C value as the arc base, and the entropy syscall trapped on an address below the arc, killing every CLUTCH encapsulation).
export CFLAGS_aarch64_unknown_ferros="-ffreestanding -fno-stack-protector -fno-builtin -mbranch-protection=none -ffixed-x24 -ffixed-x25 -ffixed-x26 -ffixed-x27 -ffixed-x28 -O2 -isystem /mnt/Harbor/Code/ferros/tools/std-sysroot/include"
cargo +nightly build --release -Zjson-target-spec "$@"
out=${CARGO_TARGET_DIR:-target}/aarch64-unknown-ferros/release
for elf in $(find $out -maxdepth 1 -type f -executable); do
    aarch64-linux-gnu-objcopy -O binary "$elf" "$elf.bin"
    echo "$elf.bin: $(stat -c %s "$elf.bin") bytes"
done
