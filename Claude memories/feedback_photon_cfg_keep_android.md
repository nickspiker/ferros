---
name: feedback_photon_cfg_keep_android
description: "Gating Photon for target_os = ferros broke Android and macOS twice (2026-10-27): inserting a ferros twin directly above a fn, BELOW its existing cfg, moves that cfg onto the twin and leaves the original under not(ferros) alone. Insert the twin ABOVE the original's attributes and give the original not(any(<its old cfg>, ferros)); check desktop + ferros + Android + Mac"
metadata:
  type: feedback
---

Gating Photon for the ferros target, I replaced two `#[cfg(not(target_os = "android"))]` gates with `#[cfg(not(target_os = "ferros"))]` (`HandleQuery::spawn_connectivity_worker`, `updates::apply_desktop_blocking`), which made desktop-only code compile for Android. The photon session caught it and fixed it (990947d2).

**The mechanism (the real bug):** my edits inserted `#[cfg(target_os = "ferros")] fn x() {..}` then `#[cfg(not(target_os = "ferros"))]` immediately above the original `fn x`, i.e. below its existing `#[cfg(..)]`. The original's attribute then attached to my twin, and the original kept only `not(ferros)`. Hits: spawn_connectivity_worker, apply_desktop_blocking (Android), launch_shim (macOS, E0428, fixed in dacfae7b), download_apk_blocking (android-only fn compiled on desktop) and resilient_copies (desktop-only fn on Android). Audit after any twin insertion: no non-ferros `#[cfg]` may sit directly above a `#[cfg(target_os = "ferros")]`.

**Why it hid:** a `not(android)` gate already excludes Android; moving it loses that. A desktop `cargo check` compiles neither Android nor ferros, so neither break shows there.

**How to apply:** insert a twin ABOVE the original's attributes, never between them and the fn; keep the original exclusion — `not(any(target_os = "android", target_os = "ferros"))`. After touching Photon cfgs, run all three: desktop `cargo check`, the ferros build (`tools/photon-build.sh` in ferros), and Photon's Android check: `bash -c 'source scripts/lib/android-env.sh; PHOTON_ALLOW_RELEASE=1 cargo check --release --lib --target aarch64-linux-android'` (if audiopus_sys fails, delete a stale CMakeCache with a /usr/local prefix under `target/aarch64-linux-android/*/build/audiopus_sys-*/out/build`). Gates touching macOS or Windows need the Mac dev build (`scripts/publish/dev-macos-arm64.sh`), which publishes — leave that to the photon session.
