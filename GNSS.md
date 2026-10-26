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

## Captured and decoded (2026-10-25)

Captures on rooted Android with `tools/gtrace` (a freestanding ptrace tracer, built for aarch64 Android without libc or the NDK): lhd started under it with the system's lhd and gpsd stopped, every read and write on `/dev/ttyBCM`, `/dev/bbd_control` and the nSTANDBY sysfs file logged with its bytes. Captures, the patch and lhd/gpsd binaries stay on Harbor (/mnt/Harbor/tmp/gnss-capture-2026-10-25): they are Google's and this phone's.

- **What lhd does:** nSTANDBY `0`, read back, `1`; `BBD:PassThru=1` to `/dev/bbd_control` (the kernel's bridge driver then passes bytes straight through); bursts of G#80 (autobaud); then Broadcom's packet transport on `/dev/ttyBCM`. The kernel driver adds only the SSI framing below.
- **SSI (bcm_gps_spi.c, SSI_MODE 1, SSI_LEN 2):** full duplex, control byte G#50; out `[G#50, len lo, len hi, data]`, in the same clocks `[status, its len lo, hi, its data]`; a read is a three-byte probe then a transfer of that length; MCU_REQ/MCU_RESP wake around it, HOST_REQ says the chip has data.
- **The packet transport (from lhd's builder, G#42368):** `b0 00` opens, `b0 01` closes; G#B0, G#11, G#13 inside are escaped as `b0 03`, `b0 04`, `b0 05`. A frame: sequence (host and chip each count their own), length low, flags, then one byte per set flag bit in bit order (G#02 the reliability/channel byte on host packets, G#40 the length's high byte, G#80 the flags' high byte, G#100/G#200 zero bytes on the sync packets), the payload, then **CRC-8 (polynomial G#4D, MSB first, initial 0) over everything after the sequence byte, nibbles swapped**. Verified on all 50,244 frames of the first capture. Host packets with flag G#02 are acked by the chip (flags G#05 or G#25, no payload, the acknowledged sequence first); the chip's own packets are never acked by the host. Sessions open with sync packets (host flags G#380, chip G#180).
- **Above it:** host RPCs start G#1E, chip messages G#1F, then an id and a length, wrapping an inner `c0 c1 … c0 c4` frame. The patch goes as 239 messages `05 83 96 …` with an address and a 912-byte chunk each; a second stage as `1e 83`.
- **The position on Android is gpsd's**, computed on the host from the chip's measurements: no NMEA and no plain position travel over this transport (the chip's ~20 KB/s after the fix carries measurements). The patch has an on-chip engine and NMEA generator (`ONCHIP_NMEA`, `$PGLOR`) and formats `$pglirm,req_breadcrumb,pvtOnChip,…` itself; gpsd only parses that string (G#31E7C4, from message type 11 of its dispatcher) and starts an on-chip batching session from it — the request seems to come from the AoC side (the chip's UART to it), and the on-chip fixes go back there. Writing the string into gpsd's `glgpsctrl` pipe did nothing.

**Under ferros (`diag gnss boot`):** the chip powered from cold, the transport synced and the patch downloaded by ferros itself — lhd's opening replayed from the vault (`gnss.replay`, records cut from the capture; Google's patch is in it, so vault and Harbor only): 554 records in 3.3 s, every reliable packet acked, no CRC failures, and the chip's opening messages byte-identical to what it sent lhd, then its periodic `1F80` reports.

**Navigating under ferros:** the first 60 s of lhd's session replayed in real time (each record waits for its acks and lhd's own gap): 987 records, all 980 reliable packets acked, 9,163 frames from the chip with no CRC failure, and in the ten seconds after, **18.1 KB/s from the chip** — the rate it streamed measurements to gpsd on Android once tracking — its messages now mostly `1F80` with `1F81`. The configuration and aiding in that session are stale (the capture's time and position); the chip navigates regardless.

## Next

1. ~~Navigation~~ — done above.
2. **A position:** either decode the measurement messages (pseudoranges, Doppler, the navigation data) and solve on the AP — ferros's own position engine — or reach the on-chip engine through the AoC's path. Both are real work; the first keeps everything on the AP.

Earlier plan, kept for reference:


1. One capture on rooted Android: `strace -f -tt -x -s 65536 -e read,write,openat -p <lhd>` across a stop/start of lhd + gpsd: the whole transport stream both ways and the timed nSTANDBY writes. (An hv trap of the SPI page misses the PDMA transfers; a kprobe on `bcm_spi_sync` or a rebuilt module with `ssi_dbg` sees the SSI layer.)
2. The patch into the vault (Google's file: Harbor, never the repo); the download replayed from the capture, each reply checked.
3. The pvtOnChip request: does an on-chip fix reach the AP over SPI?

Assistance (SUPL, LTO) is network-only and optional: without it, a standalone cold start.
