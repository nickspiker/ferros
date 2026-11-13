---
name: std-rebuild-ferros-rt
description: "A change to ferros_rt is not picked up by std apps' builds (-Zbuild-std does not fingerprint std's path deps): delete the app target's .fingerprint/ferros-rt-* and deps/*ferros_rt* before rebuilding Mahere/Photon"
metadata:
  type: feedback
---

2026-11-11: the ferros_rt fix for the stack-guard heap fault (bd8a19a) did not reach Mahere: `tools/mahere-build.sh` finished in 0.6 s and the image still carried the old runtime (its ferros-rt fingerprint predated the fix). The std app's cargo build (-Zbuild-std with ferros-rust's std depending on ferros_rt by path) does not notice changes in std's own dependencies.

**Why:** a stale runtime looks rebuilt (the .bin timestamp moves with objcopy) while the ELF is old.

**How to apply:** after any ferros_rt change, before building a std app (Mahere: `mahere/target/ferros/aarch64-unknown-ferros/<profile>/`, Photon: `std/photon/target/...`): `rm -rf <T>/.fingerprint/ferros-rt-* <T>/deps/libferros_rt-* <T>/deps/ferros_rt-*`, then build and check "Compiling ferros-rt" appears and the ELF's time is new. Related: [[std-target-status]].
