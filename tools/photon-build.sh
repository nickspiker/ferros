#!/bin/bash
# Photon for the phone as a ferros process (STD-TARGET.md): std from the ferros-rust fork, the ferros target, a development build, then the flat image the kernel loads. The build is the std/photon package (Photon's library as a dependency, its own lockfile and patch table), so Photon's manifest, lock and config are not touched. Pass extra cargo arguments after the script name.
# PHOTON_FERROS_PKG names another copy of std/photon to build: Photon's release deploy places one in its frozen snapshot, where `../../../photon` is the snapshot's Photon, not the live tree (photon's scripts/lib/ferros-kist.sh). CARGO_TARGET_DIR, when set, is where the image lands.
set -e
here=$(cd "$(dirname "$0")/.." && pwd)
pkg=${PHOTON_FERROS_PKG:-$here/std/photon}
# The std source: the caller's when it names one (Photon's release deploy passes its frozen snapshot's forks/rust/library, so std, libc and ferros_rt all come from one rev), else this tree's fork.
export __CARGO_TESTS_ONLY_SRC_ROOT=${__CARGO_TESTS_ONLY_SRC_ROOT:-$here/forks/rust/library}
export CC_aarch64_unknown_ferros=aarch64-linux-gnu-gcc
export AR_aarch64_unknown_ferros=aarch64-linux-gnu-ar
# No branch protection in the C: Fedora's cross gcc defaults to pac-ret, and its PACIASP/AUTIASP would sign with the pointer-authentication keys, which hold shares of the ira and wairua (the kernel keeps them switched off for EL0 as well, cachemap.rs).
# The registers the target reserves (targets/aarch64-unknown-ferros.json: x24 to x28; x28 the arc base and x27 its mask, which ferros_rt reads) are reserved in the C too (2026-11-17, the Pro: gcc used x27 and x28 freely in PQClean, so a C KEM calling back into Rust for its random bytes made ferros_rt read a C value as the arc base, and the entropy syscall trapped on an address below the arc, killing every CLUTCH encapsulation).
export CFLAGS_aarch64_unknown_ferros="-ffreestanding -fno-stack-protector -fno-builtin -mbranch-protection=none -ffixed-x24 -ffixed-x25 -ffixed-x26 -ffixed-x27 -ffixed-x28 -O2 -isystem $here/tools/std-sysroot/include"
export CARGO_TARGET_AARCH64_UNKNOWN_FERROS_RUSTFLAGS='--cfg getrandom_backend="custom"'
cd "$pkg"
cargo +nightly build -Zjson-target-spec -Zbuild-std=core,alloc,std,panic_abort -Zbuild-std-features=compiler-builtins-mem --target "$here/targets/aarch64-unknown-ferros.json" "$@"
profile=debug
for a in "$@"; do case "$a" in --release) profile=release ;; esac; done
out="${CARGO_TARGET_DIR:-$pkg/target}/aarch64-unknown-ferros/$profile/photon"
aarch64-linux-gnu-objcopy -O binary "$out" "$out.bin" && echo "$out.bin: $(stat -c %s "$out.bin") bytes"
