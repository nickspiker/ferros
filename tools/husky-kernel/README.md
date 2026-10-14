# husky-kernel patches

The Linux side of the ferros husky (Pixel 8 Pro) handoff.
The kernel tree itself lives at `/mnt/Harbor/husky-kernel` (Google's Kleaf manifest for shusky); these patches are the only ferros changes to it, kept here so they are versioned.

| Patch | Applies in | What it does |
|---|---|---|
| `ferros-handoff.patch` | `aosp/` (GKI common, base `8769cc47188c`) | `ferros_handoff.c` + `ferros_reset.S`: stage an image in the carveout, freeze CPUs with stop_machine, re-enter at EL2 via the hyp stub's soft restart. `/sys/kernel/ferros/stage` + the ufshcd.c reboot notifier = the runtime hot-load trigger used by `tools/husky-hotload.sh`. The boot-time trigger in `ufshcd_async_scan` fires only if `/ferros.bin` is in the vendor ramdisk. `ferros_ramoops_read` prints ferros's report from `0xFD60_0000` on the next boot. Also carries the FCORE register trace and a link-startup-failure → bootloader verdict from the UFS bisect. |
| `zuma-rmem-carveout.patch` | `private/devices/google/zuma/` | The 12 MB no-map `ferros_handoff@92400000` reserved-memory carveout ferros is linked to. |
| `ufs-exynos-debug-trace.patch` | `private/google-modules/soc/gs/` | UFS bisect instrumentation (FWTRACE/FRTRACE/FINVIS). Not needed by the handoff; kept because the captured traces in UFS.md came from it. |

Build and flash (bootloader unlocked, all three vbmetas disabled, pKVM off):

```bash
cd /mnt/Harbor/husky-kernel && ./build_shusky.sh --config=use_source_tree_aosp   # ~7 min
adb reboot bootloader
fastboot flash boot out/shusky/dist/boot.img
fastboot flash vendor_kernel_boot out/shusky/dist/vendor_kernel_boot.img
fastboot reboot
```

Only `boot` and `vendor_kernel_boot` need reflashing while the kernel release string stays `6.1.124-android14-11-g8769cc47188c-dirty`; otherwise `vendor_dlkm`/`system_dlkm` must follow (fastbootd).
