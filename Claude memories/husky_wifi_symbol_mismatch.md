---
name: husky-wifi-symbol-mismatch
description: "husky WiFi is dead because the rebuilt -dirty kernel's symbols don't match the stock WiFi modules (cfg80211->bcmdhd4398 Unknown symbol), NOT MODULE_SIG_PROTECT"
metadata: 
  node_type: memory
  type: project
  originSessionId: 873bc7fe-6da9-4b85-b1db-5d51c15b7843
---

Root-caused 2026-08-20 on hardware (was previously mis-recorded as "MODULE_SIG_PROTECT"). The custom ferros-chainload kernel `6.1.124-android14-11-g8769cc47188c-dirty` runs, but WiFi is dead:

- No `wlan0` interface; `lsmod` shows only `wlan_ptracker`, not the driver.
- The WiFi modules ARE present and version-dir-matched at `/vendor_dlkm/lib/modules/<kver>/`: `cfg80211.ko`, `bcmdhd4398.ko` (BCM4398 = Pixel 8 Pro WiFi chip), `wlan_ptracker.ko`.
- Manual `insmod bcmdhd4398.ko` → dozens of `Unknown symbol cfg80211_*  (err -2)`; `cfg80211.ko` itself won't load either. So the chain is broken at cfg80211.
- SELinux was Enforcing and drowning the WiFi HAL in `avc: denied` for `persist_vendor_debug_wifi_prop`; `setenforce 0` (Magisk root available) did NOT bring up wlan0 — so SELinux is a red herring, the real fault is symbol resolution.

**Conclusion (refined):** the failure is a FLASH MISMATCH, not a source/compile problem. `insmod cfg80211.ko` returns ENOENT (a kernel symbol it needs is absent) -> the flashed GKI kernel in boot.img and the flashed WiFi modules in vendor_dlkm came from DIFFERENT builds (version strings both read `...-dirty` but the CRCs/symbols skew). The Bazel dist at /mnt/Harbor/husky-kernel/out/shusky/dist/ has a FULLY MATCHED set built together: boot.img, vendor_kernel_boot.img, vendor_dlkm.img, system_dlkm.img, dtbo.img, plus matched bcmdhd4398.ko/cfg80211.ko. FIX = flash the matched set from ONE build (boot + vendor_kernel_boot + vendor_dlkm + system_dlkm + dtbo together), not mix-and-match.

Build system: full repo tree at /mnt/Harbor/husky-kernel (`.repo`, common/ GKI, aosp/, private/devices/google/shusky). Build = `./build_shusky.sh` (Bazel: `//private/devices/google/shusky:zuma_shusky_dist`) — compiles kernel + ALL google-modules (incl. WiFi) together, guaranteeing matched symbols. ferros hook is currently POST-build only: `repack-ferros-vkb.sh` injects `/ferros.bin` into vendor_kernel_boot.img (armed = with, .orig = without). To integrate hooks into the source (user's goal), add them to the kernel/module tree and let build_shusky.sh build+sign everything consistently.

**Why:** blocks any radio-layer work and eSIM provisioning (no network). Corrects [[pipe_...]]-adjacent notes and todo #8 which said MODULE_SIG_PROTECT.

**How to apply:** don't chase module signing. To get WiFi (e.g. to provision the Verizon eSIM), flash stock kernel+modules from the factory image, or reverse-tether over USB (gnirehtet) to sidestep WiFi entirely. eSIM lives in the eUICC and survives any reflash.
