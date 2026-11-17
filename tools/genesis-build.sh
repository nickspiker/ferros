#!/bin/bash
# The Genesis kist (docs/GENESIS.md) for a ferros device: its crate (/mnt/Harbor/Code/genesis-kist, jgenesis's core from the clone's `ferros` branch, fluor from its tree) on the ferros target, std from the ferros-rust fork, then the flat image the kernel loads. Release by default (an unoptimised core runs far slower than the panel); GENESIS_PROFILE=debug for a development build. Pass extra cargo arguments after the script name.
set -e
here=$(cd "$(dirname "$0")/.." && pwd)
code=/mnt/Harbor/Code
kist="$code/genesis-kist"
export __CARGO_TESTS_ONLY_SRC_ROOT="$code/ferros-rust/library"
export CC_aarch64_unknown_ferros=aarch64-linux-gnu-gcc
export AR_aarch64_unknown_ferros=aarch64-linux-gnu-ar
# No branch protection in the C: Fedora's cross gcc defaults to pac-ret, and its PACIASP/AUTIASP would sign with the pointer-authentication keys, which hold shares of the ira and wairua (the kernel keeps them switched off for EL0 as well, cachemap.rs).
# The registers the target reserves (targets/aarch64-unknown-ferros.json: x24 to x28; x28 the arc base and x27 its mask, which ferros_rt reads) are reserved in the C too (2026-11-17, the Pro: gcc used x27 and x28 freely in PQClean, so a C KEM calling back into Rust for its random bytes made ferros_rt read a C value as the arc base, and the entropy syscall trapped on an address below the arc, killing every CLUTCH encapsulation).
export CFLAGS_aarch64_unknown_ferros="-ffreestanding -fno-stack-protector -fno-builtin -mbranch-protection=none -ffixed-x24 -ffixed-x25 -ffixed-x26 -ffixed-x27 -ffixed-x28 -O2 -isystem $here/tools/std-sysroot/include"
# getrandom's custom backend is ferros_rt's; jgenesis uses float_algebraic and cfg_select, stable after the fork's nightly, so they are switched on for every crate until ferros-rust moves.
export CARGO_TARGET_AARCH64_UNKNOWN_FERROS_RUSTFLAGS='--cfg getrandom_backend="custom" -Zcrate-attr=feature(float_algebraic) -Zcrate-attr=feature(cfg_select) -Aunused_features -Astable_features'
export CARGO_TARGET_DIR="$kist/target/ferros"
profile=${GENESIS_PROFILE:-release}
profile_flag=$( [ "$profile" = release ] && echo --release )
cd "$kist"
cargo +nightly build -Zjson-target-spec -Zbuild-std=core,alloc,std,panic_abort -Zbuild-std-features=compiler-builtins-mem \
    --target "$here/targets/aarch64-unknown-ferros.json" \
    --config "patch.crates-io.libc.path='$code/ferros-libc'" \
    $profile_flag "$@"
out="$CARGO_TARGET_DIR/aarch64-unknown-ferros/$profile/genesis"
aarch64-linux-gnu-objcopy -O binary "$out" "$out.bin" && echo "$out.bin: $(stat -c %s "$out.bin") bytes"
