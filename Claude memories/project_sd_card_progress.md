---
name: SD card init progress
description: SD/MMC card identification working through CMD3, key fixes and remaining work
type: project
---

SD card on FP5 is responding through full identification sequence (CMD0→CMD8→ACMD41→CMD2→CMD3).

## Key fixes applied (2026-03-14)
1. **TLMM pad config must run BEFORE probe** — ABL tears down SDC2 pads at handoff
2. **Qualcomm PWRCTL handshake required** — write VENDOR_SPEC POR (0x0A0C to reg 0x20C), clear PWRCTL_STATUS (0x240), ACK via PWRCTL_CTL (0x24C) after power writes
3. **No SDHCI clock divider** — Qualcomm SDHCI bypasses internal divider, clock comes from GCC RCG. Write 0 to CLOCK_CTRL, enable INT_EN+CARD_EN only (0x0005)
4. **32-bit command write** — COMMAND+TRANSFER_MODE must be written as 32-bit to offset 0x0C (Linux sdhci.c does this), not 16-bit write to 0x0E. CMD2 R2 responses fail with 16-bit writes.
5. **ACMD41 needs ~1ms delays** — 500K nop delay between retries. Card takes ~164 tries (~80ms) to power up.

## Current state
- CMD0: OK, CMD8: OK (resp 0x1AA), ACMD41: OK (OCR 0xC0FF8000, SDHC/SDXC)
- CMD2: OK — CID shows SanDisk ("DSR01"), MFR byte needs shift fix (SDHCI shifts R2 by 8 bits)
- CMD3: OK — RCA = 0xAAAA
- Next: CMD7 (SELECT_CARD), set 25MHz clock, read/write blocks

## Card detect
- FP5 does NOT use GPIO 91 for CD (that's SC7280 IDP reference board)
- FP5 relies on SDHCI native card detect (PRESENT_STATE bit 16)
- CARD_INSERTED bit stays 0 but doesn't gate command dispatch

**Why:** Getting SD card working enables vault/ledger persistent storage.
**How to apply:** When continuing SD card work, skip re-discovering the above — go straight to CMD7 + block read/write.
