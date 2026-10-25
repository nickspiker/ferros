# Edge TPU (and the GXP DSP) on husky

The TPU ("rio", `google,edgetpu-zuma`) at G#1A00_0000 is started exactly as the AoC is: **GSA's non-secure mailbox loads and authenticates its signed image (command 50, `LOAD_TPU_FW_IMG`), and GSA's Trusty service `com.android.trusty.gsa.hwmgr.tpu` starts the Cortex-R52**. ferros has every piece of that path (gsa.rs, trusty.rs).

## Measured under ferros (2026-10-24 night)

- pd-tpu (G#1546_2504) and pd-aur, the DSP's (G#1546_2304), are on from ABL.
- `diag tpu state` (cold boot, after the sensors bring Trusty up): **`hwmgr.tpu` answers state 0 (inactive)**, and `hwmgr.dsp` answers state 0 too — a second and third tipc connection beside the AoC's in the same boot (trusty.rs now remembers GO_ONLINE).

**The firmware runs and answers (2026-10-25):** `diag tpu load` puts `tpu.fw` (tools/husky-tpu-fw.sh) in the carve-out and GSA's mailbox command 50 authenticates it (state 1); `diag tpu start` brings the TPU's local power manager up, sets up the KCI mailbox in shared data and zeroes the SSMT VIDs, then Trusty START (state 2) — and EXCHANGE_INFO is answered at once: status 0, the info buffer carrying build time G#6925_271A (2025-11-25, the image's `zum_tpu_20251124` label), flavor 3 and **changelist 836291931** (G#31D8_CD5B), the image's own. The trap on the way: ABL leaves pd_tpu on but the LPM idle (core and cluster power requests zero, the PSMs without their valid bit), so GSA reported "running" while the R52 never ran and the queue sat untouched; the LPM bring-up (`rio_patch_lpm` microcode, core state 4, clusters 1, PSM start, clock gating; mobile-soc-gsx01.c `edgetpu_soc_pm_lpm_up`) has to come before START. No SYSMMU programming was needed: the KCI queues and the info buffer sit in the TZ-mapped shared data, and sysmmu_tpu stays in bypass (VM0 control G#24) with no fault.

## The firmware and the first handshake

`/vendor/firmware/google/edgetpu-rio.fw` (1135488 bytes; Google's: Harbor and the vault, never the repo): a 4 KiB signed header ("TPUF", SHA-512 body hash, image config at G#580: privilege TZ, carve-out G#9300_0000 16 MiB, shared data G#9312_0000 = device G#1012_0000, one non-secure 54 MiB mapping at device G#1200_0000), then Cortex-R52 code; build zum_tpu_20251124.0.0, changelist 836291931.

Linux's order: power + LPM microcode patch and PSM start (mobile-soc-gsx01.c) → Trusty GET_STATE (unload if > 0) → body to G#9300_0000 → GSA mailbox 50 with the header in GSA-reachable memory → the KCI mailbox (G#1A03_0000; command queue regs +G#1000, response +G#1800; 1023 entries, wrap bit G#400) and the in-kernel VII mailbox reset and their queues placed in shared data → every SSMT VID zeroed (G#1A3E_0000, G#1A3F_0000) → Trusty START → KCI EXCHANGE_INFO (code 11) with a 64-byte info buffer: **the cheapest proof of life is its reply carrying changelist 836291931**. Then MAP_LOG_BUFFER reads the firmware's own log.

A real inference is the hard part: VII version 2 (FlatBuffer requests on mailbox 1), per-client vmboxes and page tables, and models compiled on the device by Google's proprietary compiler into "DarwiNN executables" with no public schema. The realistic route is the USF method: capture one trivial model's whole exchange on Android (kprobes on the in-kernel VII send/response, every mapped buffer with its IOVA) and replay it.

The GXP DSP ("callisto") is the same pattern with four images (GSA command 240 each, `hwmgr.dsp`), and its MCU image needs no non-secure IOMMU mapping: a fallback first target if the TPU's 54 MiB heap is needed at boot.
