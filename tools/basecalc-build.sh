#!/bin/bash
# basecalc for a ferros device as a ferros process, a kist (docs/KISTS.md "basecalc"): the std/basecalc package (basecalc's own engine, its library without the terminal, behind the kist's keypad under fluor's ferros host) on the ferros target, std from the ferros-rust fork, then the flat image the kernel loads.
# std/basecalc names basecalc, fluor, vsf and kete by path (siblings of this tree); the engine library is on basecalc's branch ferros-engine until it is on basecalc's main, so the sibling must be on that branch (or a worktree of it linked in the sibling's place).
# libc with a ferros arm is std/basecalc's own patch (its Cargo.toml), so the host's tests and this build share one lockfile.
# The engine's numbers are rug's, over GMP, MPFR and MPC in C: the gmp-mpfr-sys crate builds them with their own configure scripts, which know no ferros, so this script builds them first from the crate's own sources (freestanding, against std/basecalc/c/include, linking configure's checks against std/basecalc/c/configure-stub.c) and hands them to the crate as its cache (GMP_MPFR_SYS_CACHE); the crate then copies them in and builds nothing.
# Pass extra cargo arguments after the script name.
set -e
here=$(cd "$(dirname "$0")/.." && pwd)
code=$(cd "$here/.." && pwd)
# In a worktree of ferros (.claude/worktrees/<name>) the siblings are four levels up.
[ -d "$code/ferros-rust" ] || code=$(cd "$here/../../../.." && pwd)
pkg="$here/std/basecalc"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$pkg/target/ferros}"
target=aarch64-unknown-ferros

# GMP, MPFR and MPC from gmp-mpfr-sys's own sources, the version the lockfile names.
cargo fetch --manifest-path "$pkg/Cargo.toml" -q
version=$(grep -A1 '^name = "gmp-mpfr-sys"$' "$pkg/Cargo.lock" | sed -n -E 's/^version = "(.*)"$/\1/p')
src=$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/gmp-mpfr-sys-"$version" | head -n 1)
[ -d "$src" ] || { echo "no gmp-mpfr-sys $version sources (cargo fetch)" >&2; exit 1; }
# The crate looks in <cache>/<major.minor>/<target>/<version> (its build.rs, load_cache).
cache="$CARGO_TARGET_DIR/gmp-mpfr-sys-cache"
libs="$cache/${version%.*}/$target/$version"
work="$CARGO_TARGET_DIR/gmp-mpfr-sys-c"
inc="$pkg/c/include"
# GMP's configure tests are C17 (gcc 15 defaults to C23, where `int f()` takes no arguments).
# No branch protection, and x24 to x28 left alone, as the rest of a ferros image's C (tools/lumis-build.sh).
cflags="-std=gnu17 -O2 -fPIC -ffreestanding -fno-builtin -fno-stack-protector -mbranch-protection=none -ffixed-x24 -ffixed-x25 -ffixed-x26 -ffixed-x27 -ffixed-x28 -isystem $inc"
# The libraries are rebuilt when anything they were built from changes: the sources' version, the headers, the stub, the flags.
stamp=$( (echo "$version $cflags"; cat "$inc"/*.h "$inc"/sys/*.h "$pkg/c/configure-stub.c") | b3sum --no-names)
if [ "$(cat "$libs/stamp" 2>/dev/null)" != "$stamp" ]; then
    echo "basecalc: building GMP, MPFR and MPC for ferros (gmp-mpfr-sys $version) in $work"
    rm -rf "$work" "$libs"
    mkdir -p "$work/gmp" "$work/mpfr" "$work/mpc" "$work/prefix" "$libs"
    aarch64-linux-gnu-gcc -O2 -fno-builtin -c "$pkg/c/configure-stub.c" -o "$work/stub.o"
    (
        # configure's link checks link against the stub alone (no crt, no libc), so each finds just what a ferros process has.
        export CC="aarch64-linux-gnu-gcc -nostdlib -Wl,$work/stub.o" CC_FOR_BUILD=gcc
        export AR=aarch64-linux-gnu-ar RANLIB=aarch64-linux-gnu-ranlib NM=aarch64-linux-gnu-nm
        export CFLAGS="$cflags"
        hosts="--host=aarch64-unknown-linux-gnu --build=$(gcc -dumpmachine) --disable-shared --enable-static --with-pic --prefix=$work/prefix"
        jobs=$(nproc)
        # GMP's portable C, not its hand-written AArch64 assembly: the C is held off x24 to x28 by -ffixed, the assembly is not.
        cd "$work/gmp" && "$src"/gmp-*-c/configure $hosts --disable-assembly > configure.log 2>&1 && make -j"$jobs" > make.log 2>&1 && make install > install.log 2>&1
        # MPFR without thread-local state: one thread runs the engine, and no C thread-locals then sit in the TLS block ferros_rt lays out for Rust.
        cd "$work/mpfr" && "$src"/mpfr-*-c/configure $hosts --with-gmp="$work/prefix" --disable-thread-safe --disable-float128 --disable-decimal-float > configure.log 2>&1 && make -j"$jobs" > make.log 2>&1 && make install > install.log 2>&1
        cd "$work/mpc" && "$src"/mpc-*-c/configure $hosts --with-gmp="$work/prefix" --with-mpfr="$work/prefix" > configure.log 2>&1 && make -j"$jobs" > make.log 2>&1 && make install > install.log 2>&1
    ) || { echo "basecalc: the C libraries did not build; the logs are in $work/{gmp,mpfr,mpc}" >&2; exit 1; }
    for l in gmp mpfr mpc; do
        cp "$work/prefix/lib/lib$l.a" "$work/prefix/include/$l.h" "$libs/"
    done
    echo "$stamp" > "$libs/stamp"
    rm -rf "$work"
fi
export GMP_MPFR_SYS_CACHE="$cache"

export __CARGO_TESTS_ONLY_SRC_ROOT="$code/ferros-rust/library"
export CC_aarch64_unknown_ferros=aarch64-linux-gnu-gcc
export AR_aarch64_unknown_ferros=aarch64-linux-gnu-ar
export CFLAGS_aarch64_unknown_ferros="-ffreestanding -fno-stack-protector -fno-builtin -mbranch-protection=none -ffixed-x24 -ffixed-x25 -ffixed-x26 -ffixed-x27 -ffixed-x28 -O2 -isystem $here/tools/std-sysroot/include"
export CARGO_TARGET_AARCH64_UNKNOWN_FERROS_RUSTFLAGS='--cfg getrandom_backend="custom"'
profile=${BASECALC_PROFILE:-release}
# From the tree's root, so std/basecalc/.cargo/config.toml (the host tests' C flags) stays out of the ferros build: gmp-mpfr-sys keys its cache by CFLAGS.
cd "$here"
profile_flag=$( [ "$profile" = release ] && echo --release )
cargo +nightly build -Zjson-target-spec -Zbuild-std=core,alloc,std,panic_abort -Zbuild-std-features=compiler-builtins-mem \
    --manifest-path "$pkg/Cargo.toml" \
    --target "$here/targets/$target.json" \
    $profile_flag "$@"
out="$CARGO_TARGET_DIR/$target/$profile/basecalc"
aarch64-linux-gnu-objcopy -O binary "$out" "$out.bin" && echo "$out.bin: $(stat -c %s "$out.bin") bytes"
