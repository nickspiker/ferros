---
name: toka_pipeline_trust_model_a
description: Trust model A (device compiles everything it runs, never trusts precompiled capsules); toka bytecode = WASM forked + extended (i128, 64-bit addresses, spirix Scalar/Circle) at both ends of the rustc/LLVM pipe; no off-device masking pass; native aarch64 capsules are dev-only
metadata:
  type: project
---

Decided with the owner 2026-10-15:
- **Trust model A**: ferros compiles every program it runs (code arrives as bytecode; the on-device toka compiler emits masked native code). "No reason to trust pre-compiled capsules." So no off-device LLVM masking pass and no NaCl-style verifier — masking lives in the on-device compiler.
- **toka bytecode = WASM forked and extended**: fork LLVM's WebAssembly backend + rustc's wasm target into a toka target and grow both ends together — native i128/u128, 64-bit addresses (from memory64), then spirix Scalar / Circle as first-class value types with their ops. Plain WASM stays a valid subset (other languages' ingress).
- **Native `aarch64` capsule sections are dev-only** (hardware bring-up); a production build should refuse them. A cheap `aarch64-unknown-ferros` target spec (ABI: x27/x28 reserved, no rodata pointer tables) is still useful for the kernel and bring-up drivers.
- Order agreed earlier stands: native bring-up through the first Photon beam, then the toka pipeline; drivers stay behind `ferros_sys` so the move is a recompile.

Related: [[ferros_process_model_husky]], [[photon_on_ferros_no_bootstrap]].
