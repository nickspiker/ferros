# UFS on Pixel 8 (Zuma) — status and findings — Zil (0)

## THE FINDING (2026-08-16): our UFS commands have NEVER completed on husky

Discovered while building `payloads/miscprobe` (GPT scan → misc partition → boot-control block). Evidence chain:

- On a **fresh boot**, before any payload ran: `UTRLBA=G#8008D400` (the kernel's own buffer) with **doorbell slot 0 already stuck at 1**. The kernel's boot-time `write_block(FERROS_PART_LBA)` (the "FERROS v3" boot log) rang the doorbell and the command never completed. The kernel discards `write_block`'s return value, so this was silent — `link_is_up()` passes, transfers don't.
- No error bits: `IS=0` after 10M-spin poll, `HCS=G#10F` (all ready bits), link up. The command just never finishes.
- A stuck slot is sticky: ringing an already-set doorbell is a no-op, `UTRLCLR` (active-low per-slot clear) did NOT clear it, so every later command silently never starts. Only a reboot (ABL re-init) clears it.
- The write may or may not have reached the device — assume the ferros partition boot-log block is NOT being written.

Consequences: the vault/manifestus work has no storage path on husky until this is fixed. The M1 path is unaffected.

## Leading hypothesis: Exynos UTRL_NEXUS_TYPE

Zuma's UFSHCI is `samsung,exynos-ufs` with a vendor-specific block. From the gs-google driver (`drivers/ufs/ufs-exynos.c`, `ufs-vs-regs.h`):

- **`HCI_UTRL_NEXUS_TYPE` (VS+G#40)**: per-tag bit — 1 = SCSI/nexus command, 0 = query/NOP. The Linux driver sets it **per command** before ringing the doorbell (`exynos_ufs_set_nexus_t_xfer_req`). If ABL's last slot-0 command was non-SCSI, bit 0 is clear and our SCSI READ/WRITE gets mishandled → exactly our never-completes symptom.
- Init also writes: `HCI_DATA_REORDER=G#A`, TX/RXPRDT entry sizes, `NEXUS_TYPE=G#FFFFFFFF` (both), AXIDMA burst config — but those are global and ABL's own transfers needed them, so they're likely fine.

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

## Next steps (in order)

1. **Kernel-boot experiment**: before `init_transfer_list`, try enabling the VS APB path — candidates: CMU HSI2 gates (find the APB gate for HCI VS in the zuma clock driver), `sysreg_ufs`, or reading `HCI_FORCE_HCS` immediately (maybe accessible at boot before something gates it — ABL might hand off ungated and only idle gates it later). Instrument with framebuffer/boot-log-to-DIAG breadcrumbs BEFORE each poke so a hang identifies the culprit line.
2. Once VS is accessible: set `NEXUS_TYPE |= 1<<tag` before each SCSI command (or `G#FFFFFFFF` once, Linux-style) and retest `read_block(1)` (GPT header, read-only).
3. Then `miscprobe` works → boot-control block → and the vault storage path opens.

## Misc / boot-control context

The slot-rollback fix does NOT wait on this: `tools/pixel8/mark-slot-a-successful.py` patches the AOSP `bootloader_control` block from Android + Magisk root (misc partition offset 2048): `slot_suffix="_a"`, slot A `successful=1 tries=7 pri=15`, CRC32 recomputed. A successful slot is never decremented — one-time fix, then ferros on slot A survives unlimited reboots.
