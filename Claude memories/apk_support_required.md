---
name: apk-support-required
description: owner (2026-10-26) — ferros cannot ship without APK support; expected to be the biggest pain of the project; the arm64 translator (vendor-blob lift) is the seed of the APK native-code path
metadata:
  node_type: memory
  type: project
  originSessionId: b668c420-897f-4700-9fe3-8f3c4472a1c2
  modified: 2026-10-26T17:15:22.056Z
---

Owner, 2026-10-26: "we cannot ship ferros without apk support, which is probably going to be the biggest PITA of the whole project". Said while choosing how to run Broadcom's gpsd LTO routine on ferros: an aarch64 interpreter as an ordinary toka-compiled capsule, with vendor machine code treated as data (owner liked it), and a translator aarch64 → toka/WASM that the device then compiles.

Framing agreed in that conversation: APK = DEX bytecode + optional arm64 JNI .so + bionic/Linux syscalls + the Android framework. The translator is how third-party arm64 native code can obey "device compiles everything" (machine code becomes one more source language, sandboxed by construction); its import/syscall boundary is the future bionic + Linux personality. The gpsd LTO lift is the small, byte-verifiable pilot of exactly that path.

Owner correction, 2026-10-27: APKs are NEVER translated at runtime ("a massive security nightmare"). An APK is compiled on device at install, for that device only, and the capsule is sealed and keyed to the hardware, which proves it passed the checks (no jumps outside its arcs, no unsafe instructions). Compiled capsules are never distributed. Legal side of the APK plan: decisions get a dated log with hashed snapshots of the terms relied on, and code comments point at the log entries.

**Why:** shipping requirement; shapes every runtime decision from here.
**How to apply:** the translator is an install-time compiler stage (aarch64/DEX → toka → sealed capsule), not a runtime JIT or interpreter for APKs; imports and `svc` route to ferros-provided shims/servers; keep it consistent with [[toka_pipeline_trust_model_a]] and userspace drivers ([[ferros_process_model_husky]]).
