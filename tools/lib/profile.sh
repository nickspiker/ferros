# FERROS_PROFILE=1: the kernel and the processes built with frame pointers, so tools/flame's samples carry whole call chains (ferros_kernel/src/prof.rs). Off by default: it costs the kernel ~2 % of its code (one frame record per call). Sourced by the scripts that build; they pass "${PROFILE_FLAGS[@]}" to cargo.
PROFILE_FLAGS=()
if [ "${FERROS_PROFILE:-0}" = 1 ]; then
    PROFILE_FLAGS=(--config 'target.aarch64-unknown-none.rustflags=["-C","force-frame-pointers=yes"]')
fi
