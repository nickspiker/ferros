---
name: PT over USB status and DWC3 quirks
description: Photon Transport over USB — what works, known issues, DWC3 quirks discovered
type: project
---

## PT over USB — Current Status (2026-03-17)

### Working
- Hot-reload: `ferros-bridge reload <kernel>` — 72KB in ~15s
- Diag: 5/5 multi-session with WFI
- Blast mode: sender blasts DATA, no per-packet software ACK
- Per-chunk BLAKE3 verification
- Outbound response: no SPEC ACK needed, kernel auto-blasts via idle-poll pump
- WFI power management: CPU sleeps between USB events

### DWC3 Quirks
- Short OUT packets terminate TRB chain regardless of ISP_IMI
- ISP_IMI required on bulk IN TRBs for short packet completion
- ENDTRANSFER before STARTTRANSFER needed for multi-packet inbound
- 1ms pacing between bridge OUT sends (kernel single-TRB re-arm time)
- Bridge pads all OUT to 512 bytes
- GEVNTCOUNT=0 when drained — event buffer not the bottleneck
- Transfer resources freed on TransferComplete automatically
- ICC_PMR_EL1 + ICC_IGRPEN1_EL1 enable WFI wake on USB (no GICD needed)

### Known Issues
- USB endpoint stale after failed transfer (needs power cycle)
- 1ms pacing is a workaround — proper fix needs TRB ring or UPDATETRANSFER
- No timeout on kernel side — bridge disconnect mid-transfer = hang
- UFS SDHCI reset fails after hot-reload (controller state dirty)

### QHEE/EL2 Status
- Boot EL = 1 (QHEE owns EL2)
- GICD at G#17A00000 is TZ-protected (MMIO crash)
- HVC #0x8000 (Gunyah identify) — faults, old QHEE, no API
- QCOM vendor HVC — also faults
- hyp partition: 6MB at 0x80000000, signed, can't replace safely
- pKVM not used on Qualcomm (they use QHEE/Gunyah)
- Custom hyp flash = HIGH brick risk (secure boot chain)
