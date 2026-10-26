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

## The on-chip engine: a fix under ferros (2026-10-25 night)

**How gpsd turns it on:** gpsd's config has `IgnoreOnChipRequest` (a `hal` attribute; with it true the `$pglirm,req_breadcrumb,pvtOnChip,…` request on `glgpsctrl` is dropped, which is why it did nothing before). With it false (a copy of gps.xml bind-mounted over /vendor's, `Log="LOGCAT"` and `LogPriMask="LOG_DEBUG"` for gpsd's own log) and no other location client active, `$pglirm,req_breadcrumb,pvtOnChip,periodSec,1,batchOption,2` makes gpsd switch "from on-host ME to on-chip ME": it restarts the chip, sends its configuration, LTO orbits, ephemerides from its store, the time (`87 81`) and its last position with the time (`d0 81`), then a breadcrumb request. `batchOption` 2 is `GL_BATCH_OPT_MASK_ON_LOCATION_FIX` (each fix reported, period 1 s); with 0 the chip batches silently. Then gpsd sends nothing more: the chip navigates alone and sends a fix a second.

**The fix message:** chip payload `1f 80 …`, inner frame `c0 c1 … c0 c4` (G#C0 inside escaped as `c0 c0`; inner header: class `01 81`, length u16, checksum u16 = the 16-bit sum of class, length and body bytes without the 2-byte trailer, then the body), body id byte G#16 (the tenth byte from the class). gpsd's decoder (the `BlobStatus V:%c Lms:%u Lat:…` log, its bit reader at G#49C6F0) reads the message as **little-endian 32-bit words, most significant bit first**; from the class's first bit: error estimate u10 m at bit 221, the chip's ms clock u32 at 231, latitude s32 at 311 and longitude s32 at 343 in 2⁻²³ degrees, altitude s16 m at 375, speed s16 /64 m/s at 407, track s16 /64 ° at 439. Checked against gpsd's own decode of all 213 fixes in the capture.

**Under ferros:** `diag gnss boot [s]` replays that session (the vault's `gnss.replay`: 533 host records, 18 s; the patch, LTO and this place's last position are in it, so vault and Harbor only), with the two time messages rewritten from the RTC (GPS time = UTC + 18 s; the checksum adjusted by the bytes' difference, the packet CRC redone; a time of week whose bytes would need the C0 escape moves on a millisecond). Result: **the first fix 11.2 s after power-up, 84 fixes in 90 s, settling at ±8 m**. The session stays up after the diag: the service loop reads the chip when HOST_REQ rises (`gnss::poll`), `diag gnss fix` shows the count and the latest, `diag gnss off` drops it (nSTANDBY low, the rails off). The chip draws about 39 mA tracking (its own `$PGLOR,…,PWR` report under Android).

**Agreement:** the same engine under Android gave the same place (its fixes under gpsd's log, the same spot to a few metres); both sit about 150 m north and 40 m east of the fix Android's host engine (gpsd with Maps) reported indoors an hour earlier. Which of the two engines is right indoors is open; an outdoor fix with both settles it.

## Aiding: Broadcom's LTO (measured 2026-10-26)

The replay carries gpsd's aiding from the capture night: the time (ferros rewrites it from the RTC), the last position, the ephemerides gpsd kept, and an LTO file — why the first fix takes ~11 s. LTO ("Long Term Orbits") is Broadcom's service, from Global Locate (hence glpals.com and gpsd's `Gl*` names): every satellite's orbit and clock predicted days ahead. gpsd keeps checking it against the broadcast ephemerides (`CompareLtoBEPos … deltaPos`, 145 lines in one night's log) and flags satellites where they disagree.

- **Where the Pixel gets it:** its `/data/vendor/gps/lto2.dat` is byte-for-byte `http://gllto2.glpals.com/7day/v5/latest/lto2.dat` (~188 KB), fetched daily (gps.xml `HttpSyncLto`, `LtoSyncThresholdDays=1`; `ltoStatus.txt` = unix time of the last fetch). The server regenerates it a few times a day.
- **Open:** plain HTTP and HTTPS both answer; no key, account, signature or terms shown anywhere. Variants on the same path: `7day/v2` 59 KB, `v3` 103 KB, `v4` 146 KB, `v5` 188 KB, `4day/v5` 123 KB (gpsd's compiled-in URL is the 4day one). Also `http://gllto.glpals.com/rtistatus5.dat` (240 bytes, real-time satellite health); gpsd's `rto.dat` URL answers 404.
- **The file:** magic `ff ca de 00`, u32 length (file − 8), u32 1, 16 bytes that change per file (likely the "LTO file id" gpsd logs and sends with its upload), u32 G#75A0DE5 and u32 5292 (the header's end) in every variant, u32 70, then 70 × (index, offset) into G#44-byte records, then the bulk. gpsd logs "LTO under license to 'google', license #69" on reading one: the feed is made for Google's licence; the bytes are served to anyone.
- **Our stance:** published openly, unposted and unauthenticated, exactly as the Pixel fetches it; ferros fetches it the same way for its own chip. If Broadcom or Google ask, it comes down.
- **Not done yet:** a fresh file into the chip. gpsd's upload is the `1e83`/`1e82` messages; one capture on Android with the exact file it uploads shows the layout, then ferros builds them from a file it fetched (desktop over USB/Photon, or WiFi). Alongside, not instead: the chip's own learned ephemerides kept in the vault, and public broadcast ephemerides (IGS/NASA), so one company's feed is never the only path.

## Next

1. ~~Navigation~~ — done above.
2. ~~A position~~ — the on-chip engine, above.
3. Without gpsd's aiding: the replay carries the capture's LTO orbits and ephemerides (good for days and hours) and the last position; a cold replay without them, and ferros's own store of what the chip learns, are what make it independent of that capture.
4. The fix to processes (a SYS_FACTS slot like gravity's) and the session on a policy (on while something asks, off otherwise).

Earlier plan, kept for reference:


1. One capture on rooted Android: `strace -f -tt -x -s 65536 -e read,write,openat -p <lhd>` across a stop/start of lhd + gpsd: the whole transport stream both ways and the timed nSTANDBY writes. (An hv trap of the SPI page misses the PDMA transfers; a kprobe on `bcm_spi_sync` or a rebuilt module with `ssi_dbg` sees the SSI layer.)
2. The patch into the vault (Google's file: Harbor, never the repo); the download replayed from the capture, each reply checked.
3. The pvtOnChip request: does an on-chip fix reach the AP over SPI?

Assistance (SUPL, LTO) is network-only and optional: without it, a standalone cold start.
