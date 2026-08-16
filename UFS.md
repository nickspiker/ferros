# UFS on Pixel 8 (Zuma) — status and findings — Zil (0)

## THE FINDING (2026-08-16): our UFS commands have NEVER completed on husky

Discovered while building `payloads/miscprobe` (GPT scan → misc partition → boot-control block). Evidence chain:

- On a **fresh boot**, before any payload ran: `UTRLBA=G#8008D400` (the kernel's own buffer) with **doorbell slot 0 already stuck at 1**. The kernel's boot-time `write_block(FERROS_PART_LBA)` (the "FERROS v3" boot log) rang the doorbell and the command never completed. The kernel discards `write_block`'s return value, so this was silent — `link_is_up()` passes, transfers don't.
- No error bits: `IS=0` after 10M-spin poll, `HCS=G#10F` (all ready bits), link up. The command just never finishes.
- A stuck slot is sticky: ringing an already-set doorbell is a no-op, `UTRLCLR` (active-low per-slot clear) did NOT clear it, so every later command silently never starts. Only a reboot (ABL re-init) clears it.
- The write may or may not have reached the device — assume the ferros partition boot-log block is NOT being written.

Consequences: the vault/manifestus work has no storage path on husky until this is fixed. The M1 path is unaffected.

## Forensics (2026-08-16) — every DMA/state hypothesis eliminated, isolated to the vendor region

Kernel boot now runs one read-only `read_block(1)` (GPT header) and dumps the full state via DIAG (`UFS_*` lines; code in `kernel_main`, `ufs_diag[]`). Measured on hardware via hot-reload:

```
UFS_LINK=1          link up
UFS_READ_OCS=F      our read returned the pre-armed INVALID sentinel — controller never wrote a real OCS
UFS_LASTOCS=F       UTRD OCS field never written back
UFS_IS=0            no completion interrupt, no error interrupt
UFS_HCS=10F         DP+UTRLRDY+UTMRLRDY+UCRDY all set, power mode healthy
UFS_DBR=1           doorbell slot 0 accepted, still set — command never completed
UFS_RSR=1           transfer list IS running
UFS_AHIT=0          auto-hibernate DISABLED
UFS_CAP=1383FF1F    32 slots, 64-bit addressing — sane
UFS_RSP0/1=0        response UPIU all zero — controller never wrote a response
UFS_DATA0/1=0       no data delivered (no "EFI PART")
UFS_S2MPU_CTRL0=0   HSI2 S2MPU disabled — our bypass took
UFS_UECPA=80000010  latched PHY-adapter UIC error (valid+code G#10); UECDL/UECN/UECT/UECDME all 0
```

**Eliminated:**
- **S2MPU** — CTRL0 reads back 0 (disabled), and USB DMA works through the identical disable on the HSI0 S2MPU.
- **SysMMU / IOMMU** — the UFS DT node has `dma-coherent` and NO `iommus`; the only HSI2 sysmmu (`sysmmu@131C0000`) is `samsung,pcie-sysmmu`, port `PCIe_CH1`, status **disabled**. UFS DMAs physical addresses directly.
- **Run-stop** (RSR=1), **list-not-ready** (UTRLRDY=1), **auto-hibernate** (AHIT=0).

**Conclusion:** the controller accepts the doorbell but never fetches/executes the request — no descriptor writeback, no response UPIU, no interrupt — with nothing in the DMA path blocking it. That isolates the cause to the **Samsung vendor-specific controller config**, i.e. `HCI_UTRL_NEXUS_TYPE`.

## The blocker: UTRL_NEXUS_TYPE is in a region that hangs on access

- **`HCI_UTRL_NEXUS_TYPE` (`reg_hci`+G#40 = `G#1320_1140`)**: per-tag bit, 1 = this tag is a SCSI/nexus transfer. The Linux driver sets `G#FFFFFFFF` at init (`config_host`) AND per-command (`exynos_ufs_set_nexus_t_xfer_req`). On a stock UFSHCI, RSR+doorbell+ready = execute; on Exynos the controller ignores the doorbell unless the tag's NEXUS bit is set. If ABL left it clear (or a reset cleared it), our command is silently ignored — matches every symptom above.
- **But `reg_hci` (the vendor region, base `G#1320_1100` — confirmed via the driver's ioremap order, resource index 1) HANGS the AP on any CPU read** (froze the phone via miscprobe; `NEXUS_TYPE` at `G#1320_1140` is inside it). And it is NOT auto-hibernate (AHIT=0), so the cause of the gating is something else — most likely a CMU HSI2 UFS clock gate (the vendor region's APB clock stopped) or the auto-clock-gating controlled by `HCI_FORCE_HCS` (VS+G#B4 — itself in the gated region, a catch-22).
- Init also programs `HCI_DATA_REORDER=G#A`, TX/RXPRDT entry sizes, AXIDMA burst — all in the same gated region.

## The trap (learned the hard way, phone frozen twice today)

**The VS block (`G#1320_1100`, from the zuma-ufs.dtsi reg list) is NOT CPU-accessible in our current boot state — a read hangs the AP dead.** No SError we can catch, no recovery, physical power-cycle required (we disable the cluster watchdogs). Likely APB clock gating (`HCI_FORCE_HCS` VS+G#B4 controls clock-stop enables — but it's in the same gated block) or a peripheral protector. The standard HCI region (`G#1320_0000`) reads fine.

**Rule: never touch a new MMIO region from a RUN payload.** A payload hang freezes the phone (no preemption, no watchdog). New-region experiments go in **kernel boot code** instead — if boot hangs, ABL's A/B fallback recovers the phone remotely (costs slot-A retries, resettable via `fastboot --set-active=a`).

## DT reg map (zuma-ufs.dtsi, `ufs@0x13200000`)

```
G#1320_0000  G#200   HCI standard      (validated: readable)
G#1320_1100  G#2000  Vendor specified  (HANGS on CPU read — see trap)
G#1328_0000  G#8000  UNIPRO
G#132A_0000  G#A014  UFS protector
G#1320_4000  G#4000  PHY
G#1320_8000  G#804   CPORT
```

Also relevant: `s2mpu_s0_hsi2@131f0000`, `sysreg_ufs@13020000`, CMU HSI2 clock domain.

## CMU clock gate checked (2026-08-16): core UFS clock appears ON

Found the UFS Q-channel gate: `QCH_CON_UFS_EMBD` at `CMU_HSI2`(`G#1300_0000`)+`G#30C4` = **`G#1300_30C4`** (bits [0]=ENABLE/HWACG, [1]=CLOCK_REQ, [2]=IGNORE_FORCE_PM). Base + offset from `cmucal-sfr.c` / `cmucal-qch.c`. **The CMU read did NOT hang** (it's core infra behind the already-disabled HSI2 S2MPU — safe), and returned:

```
UFS_QCH     = 00000002   ENABLE=0 (HWACG off), CLOCK_REQ=1 (clock requested/on)
UFS_QCH_FMP = 00000002   same for the FMP (inline-crypto) clock
```

So the UFS_EMBD **core clock is on and not auto-gating** — which *weakens* the "vendor region is clock-gated" theory. The vendor-register block must be on a separate gate (a UNIPRO/HCI APB PCLK), or the hang is not a clock issue at all (a link-level low-power stall, or a genuine bus fault on that sub-range).

**Caveat on the hang itself:** it has only ever been observed from the miscprobe *payload*, which also ran a doorbell-recovery loop, `UTRLCLR`, and `read_block` — the vendor read was never isolated as the sole cause. Before chasing the gate further, the hang needs clean confirmation.

## Next steps (in order)

The problem still reduces to: **set `NEXUS_TYPE=G#FFFFFFFF` in the `reg_hci` vendor region (`G#1320_1100`)** — everything else is proven fine. But the vendor region's accessibility is the open question.

1. **Cleanly confirm (or refute) the vendor-region hang** with an isolated single read of `NEXUS_TYPE` (`G#1320_1140`). Two blockers to doing it safely: the pixel8 boot path has NO framebuffer console (display SysMMU is write-protected), so a boot hang is *silent* (no screen, no USB) — indistinguishable from any other freeze; and the cluster watchdogs are disabled (e42b9a1), so a hang needs a physical power-cycle, not A/B recovery. **Prereq: either (a) bring up a framebuffer breadcrumb in the pixel8 path, or (b) temporarily re-enable a cluster watchdog so a hang self-resets (~60s) and becomes recoverable.** Then the isolated vendor read is safe to attempt.
2. If the vendor region reads fine → the whole saga was a payload-specific misfire; just set `NEXUS_TYPE=G#FFFFFFFF` + `DATA_REORDER=G#A` + PRDT sizes (per `config_host`), retest `read_block(1)` → expect OCS=0 and "EFI PART".
3. If it truly hangs → find the vendor-region APB/UNIPRO PCLK gate (separate from `UFS_EMBD` QCH) or the link low-power state keeping it stalled; ungate via a CMU/sysreg register (accessible), then step 2.
4. Then `miscprobe` works → boot-control block, and the vault/manifestus storage path opens.

The safety prereq in step 1 (framebuffer breadcrumb or recoverable watchdog) is itself the highest-value next task — it de-risks every future new-MMIO experiment on this device, not just UFS.

**Fallback if ungating proves too deep:** a full UFSHCI HCE reset + our own Samsung-style re-init (link startup DME commands + `config_host` VS programming) — heavier, and still needs vendor-region access, so ungating comes first regardless.

## Instrumentation left in place

`kernel_main` runs the read-only `read_block(1)` forensic every boot and reports `UFS_*` via DIAG (see `ufs_diag[]`). It leaves doorbell slot 0 stuck (UFS is non-functional anyway during bring-up). Remove once the command path works. The old boot-time `write_block(FERROS_PART_LBA)` (the "FERROS v3" log) is retired — it was an unverified write that never landed and polluted the partition LBA.

## Misc / boot-control context (slot-rollback fix — BLOCKED on format RE)

Goal was to stop ABL's A/B rollback by marking slot A "successful" (a successful slot is never decremented). Investigated 2026-08-16 from Android + Magisk root; **Pixel does NOT use the AOSP-standard layout**:

- `misc` offset 0: BCB command field (`bootonce-bootloader`). Offset 2048: the ASCII string `theme-dark`, **not** the `bootloader_control` struct. No `BCAB`/`ABAB` magic anywhere in the first 64 KB of misc.
- No dedicated `slot-metadata` partition. Slot state lives in the proprietary **`devinfo`** partition (`DEVI` magic; slot-looking bytes at offset ~32), managed by the gs-common boot HAL `device/google/gs-common/bootctrl/aidl/BootControl.cpp`.
- `tools/pixel8/mark-slot-a-successful.py` assumed the standard offset; its read-verify gate caught the mismatch and refused to write (working as intended — a blind write here is the S2MPU/PMU-blind-write class of bug). Kept as a runnable record of the finding.

**Workaround (works now):** `fastboot --set-active=a` — the bootloader writes the proprietary format correctly itself; resets slot A's retry count each time it rolls back. **Real fix (future):** get `device/google/gs-common` source, decode `BootControl.cpp`'s devinfo storage, then replicate from a rooted-Android script (short-term) or ferros itself (long-term, and itself blocked on the UFS command path above).
