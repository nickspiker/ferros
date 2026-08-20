---
name: gs-google-reference-tree
description: Full gs-google Pixel 8 kernel source cloned locally at /mnt/Harbor/ferros-ref — grep it instead of fetching files piecemeal
metadata: 
  node_type: memory
  type: reference
  originSessionId: 873bc7fe-6da9-4b85-b1db-5d51c15b7843
---

The full gs-google kernel source for the Pixel 8 (Tensor G3) is cloned on this dev host — use it as the reference codebase for any Samsung/Exynos/Tensor driver, register sequence, or DT address instead of fetching files one at a time from googlesource (which 404s on awkward paths and was a recurring friction point).

**Location:** `/mnt/Harbor/ferros-ref/` (on Harbor — 995G free; Octopus is chronically ~99% full, do NOT clone big things there).
- `soc-gs/` — `kernel/google-modules/soc/gs`, branch `android-gs-shusky-5.15-android15-qpr1` (the exact branch husky runs), shallow (--depth 1), 31M. Has ALL SoC drivers (`drivers/phy/samsung/`, `drivers/i2c/`, PMU, UFS, display, reboot…) AND the SoC device tree (`arch/arm64/boot/dts/google/zuma-*.dtsi`).
- `shusky/` — `device/google/shusky`, 45M — device configs only, NO dts source (the husky overlay ships as a compiled dtbo on the device; read binding-specific nodes like the eusb-repeater bus/address from the live DT via Magisk root, or decompile the on-device dtbo).

`tools/pixel8/phy-ref/` in the repo is the old piecemeal subset (gitignored); the Harbor tree supersedes it. Not for compiling — pure-Rust-no-C stands; this is read-only reference.

To refresh or add another gs repo: `git clone --depth 1 --branch android-gs-shusky-5.15-android15-qpr1 https://android.googlesource.com/kernel/google-modules/soc/gs`. Probe repo existence cheaply with `git ls-remote --heads <url>` before cloning. See [[pixel8_boot_findings]] and REPEATER.md (in-repo) which reference this tree.
