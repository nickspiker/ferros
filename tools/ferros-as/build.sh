#!/bin/bash
# Build ferros-as (docs/SANDBOX.md "The masking assembler") against LLVM's MC libraries at the version rustc emits (rustc --version --verbose: LLVM 22.1.4).
# Only LLVM's MC layer and the AArch64 target are built, once, from llvm-project's tag into FERROS_LLVM (default /mnt/Harbor/ferros-ref/llvm-build-22.1.4; the cmake line is in SANDBOX.md).
# Usage: tools/ferros-as/build.sh   → target/ferros-as/ferros-as
set -e
here=$(cd "$(dirname "$0")/../.." && pwd)
build=${FERROS_LLVM:-/mnt/Harbor/ferros-ref/llvm-build-22.1.4}
src=$(sed -n 's/^CMAKE_HOME_DIRECTORY:INTERNAL=//p' "$build/CMakeCache.txt")
[ -f "$build/lib/libLLVMMC.a" ] && [ -d "$src/include/llvm" ] || { echo "no LLVM MC build at $build: build LLVM 22.1.4's MC first (SANDBOX.md \"The masking assembler\")" >&2; exit 1; }
out="$here/target/ferros-as"
mkdir -p "$out"
libs=""
for l in AArch64AsmParser AArch64Desc AArch64Info AArch64Utils MCParser MC DebugInfoDWARFLowLevel BinaryFormat TargetParser Support Demangle; do libs="$libs $build/lib/libLLVM$l.a"; done
clang++ -std=c++17 -O2 -fno-rtti -I"$src/include" -I"$build/include" -I"$src/lib/Target/AArch64" -I"$build/lib/Target/AArch64" -o "$out/ferros-as" "$here/tools/ferros-as/ferros-as.cpp" -Wl,--start-group $libs -Wl,--end-group -lpthread
echo "$out/ferros-as"
