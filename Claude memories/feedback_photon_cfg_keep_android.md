---
name: feedback_photon_cfg_keep_android
description: "When gating Photon for target_os = ferros, never turn a not(android) gate into not(ferros) — that drops the Android exclusion and desktop-only code compiles for Android. Make it not(any(android, ferros)) and run Photon's Android check; a desktop cargo check cannot see it (2026-10-27, the photon session fixed it in 990947d2)"
metadata:
  type: feedback
---

Gating Photon for the ferros target, I replaced two `#[cfg(not(target_os = "android"))]` gates with `#[cfg(not(target_os = "ferros"))]` (`HandleQuery::spawn_connectivity_worker`, `updates::apply_desktop_blocking`), which made desktop-only code compile for Android. The photon session caught it and fixed it (990947d2).

**Why:** a `not(android)` gate already excludes Android; swapping its target loses that. A desktop `cargo check` compiles neither Android nor ferros, so neither break shows there.

**How to apply:** keep the original exclusion — `not(any(target_os = "android", target_os = "ferros"))`. After touching Photon cfgs, run all three: desktop `cargo check`, the ferros build (`tools/photon-build.sh` in ferros), and Photon's Android check: `bash -c 'source scripts/lib/android-env.sh; PHOTON_ALLOW_RELEASE=1 cargo check --release --lib --target aarch64-linux-android'` (if audiopus_sys fails, delete a stale CMakeCache with a /usr/local prefix under `target/aarch64-linux-android/*/build/audiopus_sys-*/out/build`).
