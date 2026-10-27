---
name: std_target_status
description: "Rust std runs on the phone since 2026-10-27: targets/aarch64-unknown-ferros.json + ferros_rt + a forked std at /mnt/Harbor/Code/ferros-rust (library/std/src/sys/pal/ferros). Build with tools/std-build.sh <crate>; spawn the .bin as any payload. Traps: build-std caches ferros_rt (clear fingerprints), pic codegen + GOT in relocate(), the image header the kernel reads for .bss + heap"
metadata:
  type: project
---

Where the std target stands (STD-TARGET.md in the ferros repo has the mechanics):
- Target: `targets/aarch64-unknown-ferros.json` (target_os ferros, static PIE, native TLS, panic abort, links through `ferros_rt/ferros.ld`). std fork: `/mnt/Harbor/Code/ferros-rust/library` (rustc 1.97.0-nightly d7f14d3d8 rust-src + `sys/pal/ferros` + `target_os = "ferros"` arms; depends on `ferros-rt` by path). Runtime: `ferros_rt/` in the ferros repo (excluded from the workspace).
- Build: `tools/std-build.sh std/hello` → `stdhello.bin`; `ferros-bridge spawn … --name stdhello --entropy`. First run 2026-10-27 passed everything (println, TLS, threads, Mutex/Condvar, HashMap, sleep, SystemTime).
- kete on ferros keeps entries by their 32-byte addresses straight through SYS_STORE bit 5 (no names, no encoding); the kernel scopes an address through its keyed hash and keeps the scope's address list.
- Kernel: trap Frame carries TPIDR_EL0; `process.rs image_tail` reads the `ferrosRT` header at image offset 8 and grows the arc by .bss + heap (2 MiB default, `--defsym __ferros_heap=`). The arc pool is 16 MiB: Photon will need an app memory region.
- Traps: cargo build-std does NOT rebuild `ferros_rt` on edit — clear `target/aarch64-unknown-ferros/release/{.fingerprint,deps}` of `ferros-rt-*` and `std-*`; under pic codegen `relocate()` must use `pc_rel!` (asm adrp) for its symbols; `compiler-builtins-mem` in build-std-features; `panic_immediate_abort` is gone.
- Next per the owner: kete gets a native-vault backend (no std::fs ever), fluor host over the app surface, tokio over a ferros mio on the `ip` server, Photon with `cfg(target_os = "ferros")`; STOP at the input device (owner's text-input idea first).

**How to apply:** build std programs only through tools/std-build.sh; when ferros_rt changes, clear the fingerprints before trusting a build; never add std::fs for Photon on ferros.
