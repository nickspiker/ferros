---
name: ira_entropy_sources_husky
description: "Grounded catalog of per-die ira entropy sources on husky (Pixel 8, Tensor G3) with register addresses; full doc at ferros/IRA-ENTROPY-SOURCES.md"
metadata: 
  node_type: memory
  type: reference
  originSessionId: 35f77793-aafd-4fba-bd97-4cd4e60013b9
---

Grounded 2026-08-20 against the gs-google tree ([[gs_google_reference_tree]] at /mnt/Harbor/ferros-ref). Full write-up: ferros/IRA-ENTROPY-SOURCES.md.

**Reachable NOW (direct MMIO / the UFS link we hold):**
- chipid block base G#1000_0000: `unique_id0/1` at +G#4/+G#8 (Tier 0 serial), plus MORE per-die trim than ira.rs currently uses — `dvfs_version` +G#900C (1B), `asv_tbl` +G#9000 (64B), `hpm_asv` +G#A000 (64B), `ap_hw_tune` +G#C300 (32B, already stubbed). All `readl`/`readb`, LE.
- UFS HCI base G#1320_0000: Device Descriptor `iSerialNumber` (~+G#18) Tier 0; Geometry/Health descriptors for defect delta. The factory bad-block map is the Tier-2 prize but needs a Samsung/SK-Hynix VENDOR query, not a standard descriptor — not yet grounded.
- MCT counter base G#100D_0000, CNT_L +G#110: timebase for a clock-ratio ring-osc PUF against a CMU-divided PLL.

**Blocked:** TMU TRIMINFO thermal fuses are behind ACPM IPC (not direct AP MMIO on zuma). DRAM PHY training is walled off (secure, not EL2-mapped). Drop both until ACPM exists.

**Needs subsystem up:** display panel id (DSI DCS G#A1/G#D6), Cirrus CS35L41 `CAL_R`/`DIAG_F0` (SPI/DSP), camera OTP + fuel-gauge (drivers NOT in the ref tree — proprietary), IMEI + modem RF cal (radio bring-up).

**Why/apply:** these feed the multi-source measure-before-keying probe. Extend the `entry_vault` ramoops instrumentation (already dumps `ap_hw_tune` + wairua) to read all reachable-now candidates across two boots + a temp swing and rank by entropy x stability x unforgeability. See [[ira_entropy_is_unforgeability]].
