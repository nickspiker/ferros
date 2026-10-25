# GNSS on husky

The receiver is a **Broadcom BCM4776** (dual band, L1 + L5; the bugreport's "GNSS Hardware Model Name: Broadcom, BCM4776", the patch's `AsicVersion=0x004776A0`), on its own SPI port to the AP. No TEE, no modem and no AoC are needed on this path. Sources: google-modules/gps/broadcom/bcm47765 (`bcm_gps_spi.c`, `bcm_gps_regs.c`, `bbd.h`), zuma-board-gps.dtsi (husky includes it through zuma-shusky-board.dtsi), the factory vendor image, the 2026-08 bugreport.

## Wiring

| Line | Pin | Bank | Direction |
|---|---|---|---|
| HOST_REQ (the chip has data) | gpa5-3 | FAR_ALIVE G#154E_0000 | in (EINT53, level high) |
| MCU_REQ (the AP wants the chip awake) | gpp4-3 | PERIC0 G#1084_0080 | out |
| MCU_RESP (the chip is awake) | gph3-2 | HSI2 G#1304_0000 | in, pull-down |
| nSTANDBY (its only power/reset line; high = run) | gph3-3 | HSI2 G#1304_0000 | out |
| PPS (optional) | gpp14-1 | PERIC0 | in |

SPI: spi_21 = HSI0 USI3 at G#111E_0000, mode 3, feedback delay 1, at most 26 MHz, the controller's own chip select (fixed-function pads; Linux uses PDMA above 64 bytes). Clock path: CMU_HSI0 mux +G#1020, divider +G#1820, gates +G#2124 / +G#2104 / +G#2108 / +G#20D0, Q-channel +G#30D0; USI mode SYSREG_HSI0 + G#1020 = 2 (ferros `spi::GNSS`).

Rails on the sub PMIC (enable bit 7, voltage bits 5:0): L9S_GNSS_CORE G#37 (725 mV + 12.5 mV/step), L10S_GNSS_RF G#38 and L11S_GNSS_AUX G#39 (700 mV + 25 mV/step). ABL leaves them **programmed but off** (1.20 V, 1.80 V, 1.80 V), as it does the sensor rails.

## Measured under ferros (2026-10-24 night)

`diag gnss up`: the three rails on, the lines set, nSTANDBY high, 250 ms: **MCU_RESP answers at once** (the chip's wake handshake), and the SPI framing answers: every reply starts with status G#19 (no error, 16 KB free in the chip's receive ring by the driver's encoding). The indirect register read (`[G#80, G#24, 5, address LE, G#02]` then `[G#A0, 0 × 11]`) echoes the address one byte later than the driver's comments say (`08 90 40 10 40` for G#4010_4090), so the value field is at bytes 6–9; the first read after power-up only primes the channel. HOST_REQ stays low: the ROM waits for the host.

## The protocol above (from the driver and lhd)

- **SSI framing** (kernel): full duplex; control byte G#50; header `[G#50, len lo, len hi]`; receive = send `[G#50,0,0]`, get `[status, len lo, len hi]`, then clock len+3 more; the status byte's bits 2:0 × {32, 256, 2048, 16384}[bits 4:3] is the chip's free receive space (never send more), bit 6 an error.
- **Wake**: MCU_REQ high, MCU_RESP within 100 polls of 1–2 ms (re-pulse every 20), transfer while HOST_REQ is high or there is data to send, MCU_REQ low.
- **Firmware**: ROM plus a RAM patch loaded every boot, /vendor/firmware/SensorHub.patch (336212 bytes of ASCII hex; blob 164413 bytes, a Cortex-M patch with an on-chip position engine and NMEA generator). lhd's download states: AUTOBAUD → SYNC → VERSION_INIT → REQUEST_DOWNLOAD → DOWNLOAD_START/COMPLETE → VERSION_CONFIRM → "Patch loaded and verified" → transport synchronised. The transport above SSI is Broadcom's proprietary "TransportLayer" (reliable packets, sequence numbers) carrying RPC; not NMEA. lhd also reads the die's lot/wafer position: identity data, never logged or committed.
- **Getting a position**, cheapest first: the on-chip engine (`$pglirm,req_breadcrumb,pvtOnChip,periodSec,%d,...` in lhd, gpsd and the patch); else gpsd's session replayed and its measurements decoded (ferros's own position engine); else CHPP through the AoC's CHRE (the chip has a UART to the AoC).

## Next

1. One capture on rooted Android: `strace -f -tt -x -s 65536 -e read,write,openat -p <lhd>` across a stop/start of lhd + gpsd: the whole transport stream both ways and the timed nSTANDBY writes. (An hv trap of the SPI page misses the PDMA transfers; a kprobe on `bcm_spi_sync` or a rebuilt module with `ssi_dbg` sees the SSI layer.)
2. The patch into the vault (Google's file: Harbor, never the repo); the download replayed from the capture, each reply checked.
3. The pvtOnChip request: does an on-chip fix reach the AP over SPI?

Assistance (SUPL, LTO) is network-only and optional: without it, a standalone cold start.
