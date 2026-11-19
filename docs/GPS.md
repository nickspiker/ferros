# GPS on husky

The receiver is a **Broadcom BCM4776** (dual band, L1 + L5; the bugreport's "GNSS Hardware Model Name: Broadcom, BCM4776", the patch's `AsicVersion=0x004776A0`), on its own SPI port to the AP. No TEE, no modem and no AoC are needed on this path. Sources: google-modules/gps/broadcom/bcm47765 (`bcm_gps_spi.c`, `bcm_gps_regs.c`, `bbd.h`), zuma-board-gps.dtsi (husky includes it through zuma-shusky-board.dtsi), the factory vendor image, the 2026-08 bugreport.

## Wiring

| Line | Pin | Bank | Direction |
|---|---|---|---|
| HOST_REQ (the chip has data) | gpa5-3 | FAR_ALIVE G#154E_0000 | in (EINT53, level high) |
| MCU_REQ (the AP wants the chip awake) | gpp4-3 | PERIC0 G#1084_0080 | out |
| MCU_RESP (the chip is awake) | gph3-2 | HSI2 G#1304_0000 | in, pull-down |
| nSTANDBY (its only power/reset line; high = run) | gph3-3 | HSI2 G#1304_0000 | out |
| PPS (optional) | gpp14-1 | PERIC0 | in |

SPI: spi_21 = HSI0 USI3 at G#111E_0000, mode 3, feedback delay 1, at most 26 MHz, the controller's own chip select (fixed-function pads; Linux uses PDMA above 64 bytes). Clock path: CMU_HSI0 mux +G#1020, divider +G#1820, gates +G#2124 / +G#2104 / +G#2108 / +G#20D0, Q-channel +G#30D0; USI mode SYSREG_HSI0 + G#1020 = 2 (ferros `spi::GPS`).

Rails on the sub PMIC (enable bit 7, voltage bits 5:0): L9S_GNSS_CORE G#37 (725 mV + 12.5 mV/step), L10S_GNSS_RF G#38 and L11S_GNSS_AUX G#39 (700 mV + 25 mV/step). ABL leaves them **programmed but off** (1.20 V, 1.80 V, 1.80 V), as it does the sensor rails.

## Measured under ferros (2026-10-24 night)

`diag gps up`: the three rails on, the lines set, nSTANDBY high, 250 ms: **MCU_RESP answers at once** (the chip's wake handshake), and the SPI framing answers: every reply starts with status G#19 (no error, 16 KB free in the chip's receive ring by the driver's encoding). The indirect register read (`[G#80, G#24, 5, address LE, G#02]` then `[G#A0, 0 × 11]`) echoes the address one byte later than the driver's comments say (`08 90 40 10 40` for G#4010_4090), so the value field is at bytes 6–9; the first read after power-up only primes the channel. HOST_REQ stays low: the ROM waits for the host.

## The protocol above (from the driver and lhd)

- **SSI framing** (kernel): full duplex; control byte G#50; header `[G#50, len lo, len hi]`; receive = send `[G#50,0,0]`, get `[status, len lo, len hi]`, then clock len+3 more; the status byte's bits 2:0 × {32, 256, 2048, 16384}[bits 4:3] is the chip's free receive space (never send more), bit 6 an error.
- **Wake**: MCU_REQ high, MCU_RESP within 100 polls of 1–2 ms (re-pulse every 20), transfer while HOST_REQ is high or there is data to send, MCU_REQ low.
- **Firmware**: ROM plus a RAM patch loaded every boot, /vendor/firmware/SensorHub.patch (336212 bytes of ASCII hex; blob 164413 bytes, a Cortex-M patch with an on-chip position engine and NMEA generator). lhd's download states: AUTOBAUD → SYNC → VERSION_INIT → REQUEST_DOWNLOAD → DOWNLOAD_START/COMPLETE → VERSION_CONFIRM → "Patch loaded and verified" → transport synchronised. The transport above SSI is Broadcom's proprietary "TransportLayer" (reliable packets, sequence numbers) carrying RPC; not NMEA. lhd also reads the die's lot/wafer position: identity data, never logged or committed.
- **Getting a position**, cheapest first: the on-chip engine (`$pglirm,req_breadcrumb,pvtOnChip,periodSec,%d,...` in lhd, gpsd and the patch); else gpsd's session replayed and its measurements decoded (ferros's own position engine); else CHPP through the AoC's CHRE (the chip has a UART to the AoC).

## Captured and decoded (2026-10-25)

Captures on rooted Android with `tools/gtrace` (a freestanding ptrace tracer, built for aarch64 Android without libc or the NDK): lhd started under it with the system's lhd and gpsd stopped, every read and write on `/dev/ttyBCM`, `/dev/bbd_control` and the nSTANDBY sysfs file logged with its bytes. Captures, the patch and lhd/gpsd binaries stay on Harbor (/mnt/Harbor/tmp/gps-capture-2026-10-25): they are Google's and this phone's.

- **What lhd does:** nSTANDBY `0`, read back, `1`; `BBD:PassThru=1` to `/dev/bbd_control` (the kernel's bridge driver then passes bytes straight through); bursts of G#80 (autobaud); then Broadcom's packet transport on `/dev/ttyBCM`. The kernel driver adds only the SSI framing below.
- **SSI (bcm_gps_spi.c, SSI_MODE 1, SSI_LEN 2):** full duplex, control byte G#50; out `[G#50, len lo, len hi, data]`, in the same clocks `[status, its len lo, hi, its data]`; a read is a three-byte probe then a transfer of that length; MCU_REQ/MCU_RESP wake around it, HOST_REQ says the chip has data.
- **The packet transport (from lhd's builder, G#42368):** `b0 00` opens, `b0 01` closes; G#B0, G#11, G#13 inside are escaped as `b0 03`, `b0 04`, `b0 05`. A frame: sequence (host and chip each count their own), length low, flags, then one byte per set flag bit in bit order (G#02 the reliability/channel byte on host packets, G#40 the length's high byte, G#80 the flags' high byte, G#100/G#200 zero bytes on the sync packets), the payload, then **CRC-8 (polynomial G#4D, MSB first, initial 0) over everything after the sequence byte, nibbles swapped**. Verified on all 50,244 frames of the first capture. Host packets with flag G#02 are acked by the chip (flags G#05 or G#25, no payload, the acknowledged sequence first); the chip's own packets are never acked by the host. Sessions open with sync packets (host flags G#380, chip G#180).
- **Above it:** host RPCs start G#1E, chip messages G#1F, then an id and a length, wrapping an inner `c0 c1 … c0 c4` frame. The patch goes as 239 messages `05 83 96 …` with an address and a 912-byte chunk each; a second stage as `1e 83`.
- **The position on Android is gpsd's**, computed on the host from the chip's measurements: no NMEA and no plain position travel over this transport (the chip's ~20 KB/s after the fix carries measurements). The patch has an on-chip engine and NMEA generator (`ONCHIP_NMEA`, `$PGLOR`) and formats `$pglirm,req_breadcrumb,pvtOnChip,…` itself; gpsd only parses that string (G#31E7C4, from message type 11 of its dispatcher) and starts an on-chip batching session from it — the request seems to come from the AoC side (the chip's UART to it), and the on-chip fixes go back there. Writing the string into gpsd's `glgpsctrl` pipe did nothing.

**Under ferros (`diag gps boot`):** the chip powered from cold, the transport synced and the patch downloaded by ferros itself — lhd's opening replayed from the vault (`gps.replay`, records cut from the capture; Google's patch is in it, so vault and Harbor only): 554 records in 3.3 s, every reliable packet acked, no CRC failures, and the chip's opening messages byte-identical to what it sent lhd, then its periodic `1F80` reports.

**Navigating under ferros:** the first 60 s of lhd's session replayed in real time (each record waits for its acks and lhd's own gap): 987 records, all 980 reliable packets acked, 9,163 frames from the chip with no CRC failure, and in the ten seconds after, **18.1 KB/s from the chip** — the rate it streamed measurements to gpsd on Android once tracking — its messages now mostly `1F80` with `1F81`. The configuration and aiding in that session are stale (the capture's time and position); the chip navigates regardless.

## The on-chip engine: a fix under ferros (2026-10-25 night)

**How gpsd turns it on:** gpsd's config has `IgnoreOnChipRequest` (a `hal` attribute; with it true the `$pglirm,req_breadcrumb,pvtOnChip,…` request on `glgpsctrl` is dropped, which is why it did nothing before). With it false (a copy of gps.xml bind-mounted over /vendor's, `Log="LOGCAT"` and `LogPriMask="LOG_DEBUG"` for gpsd's own log) and no other location client active, `$pglirm,req_breadcrumb,pvtOnChip,periodSec,1,batchOption,2` makes gpsd switch "from on-host ME to on-chip ME": it restarts the chip, sends its configuration, LTO orbits, ephemerides from its store, the time (`87 81`) and its last position with the time (`d0 81`), then a breadcrumb request. `batchOption` 2 is `GL_BATCH_OPT_MASK_ON_LOCATION_FIX` (each fix reported, period 1 s); with 0 the chip batches silently. Then gpsd sends nothing more: the chip navigates alone and sends a fix a second.

**The fix message:** chip payload `1f 80 …`, inner frame `c0 c1 … c0 c4` (G#C0 inside escaped as `c0 c0`; inner header: class `01 81`, length u16, checksum u16 = the 16-bit sum of class, length and body bytes without the 2-byte trailer, then the body), body id byte G#16 (the tenth byte from the class). gpsd's decoder (the `BlobStatus V:%c Lms:%u Lat:…` log, its bit reader at G#49C6F0) reads the message as **little-endian 32-bit words, most significant bit first**; from the class's first bit: error estimate u10 m at bit 221, the chip's ms clock u32 at 231, latitude s32 at 311 and longitude s32 at 343 in 2⁻²³ degrees, altitude s16 m at 375, speed s16 /64 m/s at 407, track s16 /64 ° at 439. Checked against gpsd's own decode of all 213 fixes in the capture.

**Under ferros:** `diag gps boot [s]` (now the driver process, below) replays that session (the vault's `gps.replay`: 533 host records, 18 s; the patch, LTO and this place's last position are in it, so vault and Harbor only), with the two time messages rewritten from the RTC (GPS time = UTC + 18 s; the checksum adjusted by the bytes' difference, the packet CRC redone; a time of week whose bytes would need the C0 escape moves on a millisecond). Result: **the first fix 11.2 s after power-up, 84 fixes in 90 s, settling at ±8 m**. The session stays up after the diag: the service loop reads the chip when HOST_REQ rises (`gps::poll`), `diag gps fix` shows the count and the latest, `diag gps off` drops it (nSTANDBY low, the rails off). The chip draws about 39 mA tracking (its own `$PGLOR,…,PWR` report under Android).

**Agreement:** the same engine under Android gave the same place (its fixes under gpsd's log, the same spot to a few metres); both sit about 150 m north and 40 m east of the fix Android's host engine (gpsd with Maps) reported indoors an hour earlier. Which of the two engines is right indoors is open; an outdoor fix with both settles it.

## From the sky, on ferros's own aiding (2026-10-26)

What gpsd sends after the patch download, from the on-chip capture (lhd4): the chip's second firmware stage (`02 80` writes with ~1 KB code chunks, ~58 KB), configuration (`03 00`, `06 00`, `10 00`, `17 00` strings), then the aiding — `87 81` time, `d0 81` position and time (sent three times), `8e 81` ×3, `83 81`, `92 81` (not identified), and the LTO subset as `82 81` messages (`1e83` + continuations, ~10 KB). In on-chip mode gpsd uploads no ephemerides ("GenerateAiding() skip because it's onchip ME").

- **Cutting messages out of a replay:** the transport sequence is not under the CRC, so a frame after a cut gets its sequence moved back by the frames cut; every RPC carries a trailer counter (+1 per message, restarting with the second stage), moved back the same way. A rebuild that cuts nothing is byte-identical to the capture.
- **Without the LTO** (`gps.replay.sky`, 517 records): first fix 166 s once, nothing in 300 s once (indoors); the chip decodes its own orbits.
- **`d0 81` is needed:** cut, or with its position zeroed, the chip stays silent for 5-8 minutes (a zero position with whatever uncertainty the message carries points it at the wrong sky). Decoded so far: latitude u32 in 2⁻¹⁶ degrees at body offset 2 (10 bytes after `c0 c1`), longitude u32 in 2⁻¹⁵ degrees, west positive, at body offset 14; the rest (likely altitude and uncertainties) not yet.
- **ferros's own position:** every session's last fix goes into the vault (`gps.last`), and the next session's `d0 81` carries it (gps.rs `patch_aiding`). **With no LTO, ferros's last fix and the RTC's time: first fix at 92 s, 393 fixes in 8 minutes, ±6 m** (`diag gps boot 480 sky`). What remains from the capture is Broadcom's firmware and gpsd's configuration, not data about the sky.
- The replay with the LTO kept (`gps.replay`) slowed as its aiding aged: 11-18 s on 2026-10-25, 78 s and once nothing in 90 s the next morning.
- Open: the three unidentified messages; the uncertainty field is narrowed to one of two (below, "The position aiding, d0 81", 2026-11-11), and a start with no last fix now carries the zone's place ("A rough start from the zone").

## The driver is a process (2026-10-26)

`payloads/gps` runs the chip from EL0 (`tools/husky-gps.sh [sky]`): the SPI port and the four lines thru MMIO grants (`usi3-spi`, `gpio-far-alive`, `gpio-gpp0`, `gpio-hsi2`), HOST_REQ as its own interrupt (`gps-host-req`, EINT53 = SPI 63, level high), the replay and the last fix thru the vault scope `gps.` (values larger than a transfer read piecewise: `store_get_at`, the kernel keeping the value between reads), and the power thru a new kind of capability: `--power gps`, `SYS_POWER`, the kernel's sequence for the rails (ACPM), the port's clocks and the pin modes, run only on the X3 so it cannot interleave with the service loop's own firmware calls. The `gps` endpoint answers FIX (33 bytes: valid, count, age, lat, lon, alt, error, speed, track) and STOP (power down, exit); `payloads/gpstest` is its test client. The kernel's gps.rs is down to the power switch (its `diag gps` tools gone, the last, `off`, on 2026-11-10: a driver that ends has its switch turned off by the kernel); the port runs thru `ferros_spi`, the crate the switch sets the port up with.

Measured: the replay in 7.9 s (533 records, every packet acked, no CRC failure), a fix decoded and served on the endpoint, STOP powering the chip down. The same afternoon neither the driver nor the kernel's own path (before it was removed) got fixes reliably from where the phone then was: one fix at 144 s and none after, then nothing in 4 minutes; the sky, not the driver, is the open question there.

## Aiding: Broadcom's LTO (measured 2026-10-26)

The replay carries gpsd's aiding from the capture night: the time (ferros rewrites it from the RTC), the last position, the ephemerides gpsd kept, and an LTO file — why the first fix takes ~11 s. LTO ("Long Term Orbits") is Broadcom's service, from Global Locate (hence glpals.com and gpsd's `Gl*` names): every satellite's orbit and clock predicted days ahead. gpsd keeps checking it against the broadcast ephemerides (`CompareLtoBEPos … deltaPos`, 145 lines in one night's log) and flags satellites where they disagree.

- **Where the Pixel gets it:** its `/data/vendor/gps/lto2.dat` is byte-for-byte `http://gllto2.glpals.com/7day/v5/latest/lto2.dat` (~188 KB), fetched daily (gps.xml `HttpSyncLto`, `LtoSyncThresholdDays=1`; `ltoStatus.txt` = unix time of the last fetch). The server regenerates it a few times a day.
- **Open:** plain HTTP and HTTPS both answer; no key, account, signature or terms shown anywhere. Variants on the same path: `7day/v2` 59 KB, `v3` 103 KB, `v4` 146 KB, `v5` 188 KB, `4day/v5` 123 KB (gpsd's compiled-in URL is the 4day one). Also `http://gllto.glpals.com/rtistatus5.dat` (240 bytes, real-time satellite health); gpsd's `rto.dat` URL answers 404.
- **The file:** magic `ff ca de 00`, u32 length (file − 8), u32 1, 16 bytes that change per file (likely the "LTO file id" gpsd logs and sends with its upload), u32 G#75A0DE5 and u32 5292 (the header's end) in every variant, u32 70, then 70 × (index, offset) into G#44-byte records, then the bulk. gpsd logs "LTO under license to 'google', license #69" on reading one: the feed is made for Google's licence; the bytes are served to anyone.
- **Our stance:** published openly, unposted and unauthenticated, exactly as the Pixel fetches it; ferros fetches it the same way for its own chip. If Broadcom or Google ask, it comes down.
- **Not done yet:** a fresh file into the chip. gpsd's upload is the `1e83`/`1e82` messages; one capture on Android with the exact file it uploads shows the layout, then ferros builds them from a file it fetched (desktop over USB/Photon, or WiFi). Alongside, not instead: the chip's own learned ephemerides kept in the vault, and public broadcast ephemerides (IGS/NASA), so one company's feed is never the only path.

## Location is the person's (owner, 2026-11-04)

- **No GPS while nobody has attested.** The one exception is locate (AUTH.md "Locate", a plan).
- **On demand (owner, 2026-11-10: "Wire it so GPS stays green but no visuals for satellites since we're not rx'ing and all that until something like mahere calls it or I open the GPS screen (which should turn it on whilist GPS screen is open) or Photon a friend pings for my location").** The switch on means GPS is allowed, not that the chip receives. The driver runs only while a process holds a claim (`SYS_CLAIM` 37, kernel process.rs `CLAIMS`, ferros_sys `claim`):
  - Op 0 claims a right (only `gps` today), proved by Settings (the map's GPS page, while it shows with the screen on) or by the `gps` endpoint's send right (a kist the person granted GPS). Refused while nobody is present or GPS is switched off. Op 1 releases it. A process's end, any way, drops its claims (`end_process` and `kill_locked`).
  - The service loop starts the driver as the claims go from none to one and kills it (chip powered down) as they go back to none, woken by the change (`sev`), never a timer. A driver that ends by itself while claimed is not started again until the claims go to none and back. Switching GPS off, or nobody present, drops every claim.
  - Op 2 lists who claims a right (a kist by its capsule address, a system process with zeros), for Settings: the GPS and permissions pages say "using now". Fact 85 carries the count and a change count.
  - Op 3 asks the driver FIX or SATS for a Settings holder that holds a claim, as its own CALL_LIVE call (the map has no slot left on init's grants page for a `use gps` of its own). The position passes thru; the kernel keeps none of it.
  - Doorbells, no polling (owner, 2026-11-10: "Needs a doorbell on the driver's fix counter, for sure. 1s poll can desync too"): the driver's op 3 WATCH (`count u32` → the count now) is held until its fixes or fix messages move; the map's GPS page holds one thru the kernel (op 3 with x1 = 3 | count << 8, not CALL_LIVE, so a driver not started yet takes it when it first receives) on a thread of its own, and reads FIX when it rings. Op 4 registers a word in a Settings holder's arc that the kernel writes GPS's claims into at every change, waking its waiters: the permissions page's "using now".
  - The driver sleeps in one wait for HOST_REQ's interrupt and a call to `gps`: an interrupt wait with no deadline, the kernel waking the endpoint's poller out of it for a call (and returning at once when one was queued before the wait). It looked at its endpoint every 1/64 s before.
  - The coordinates follow the device's base (owner: "follow the base, yes"): decimal degrees at six arabic places, five hex or six dozenal; degrees, minutes and seconds each in the base, seconds to one digit after its point.
  - A kist claims, then calls its `gps` slot with `call_live` (ERR while the driver is still replaying its session), and releases or simply ends.
  - The console's GPS circle is lit while GPS is allowed and a person is present; its satellite rings and numbers only while the driver runs. A driver's measures and tracks are emptied as it ends, so a stopped driver's satellites are not drawn.
  - The GPS page shows the seven Dymaxion words (vsf WorldCoord), degrees with their fraction and degrees, minutes and seconds once there is a fix, on the screen only.
- **The last fix is the person's.** `gps.last` is identity class (kvault.rs `IDENTITY`): while locked the driver can neither read nor write it.
  - The old device-class entry was deleted the same night.
- **The replays keep no place.** They held the capture's position (three `d0 81` messages each, the place they were captured), and they are device class.
  - The driver's scrub mode (input G#02) zeroes those positions through `patch_aiding`, so the checksum and CRC are redone, and writes the replays back in pieces. It never powers the chip.
  - Run once on 2026-11-04. Verified on the stored bytes: no latitude or longitude left in either replay.
  - The messages themselves stay: cut or zeroed, the chip waits 5-8 minutes alike, and zeroed keeps the record sequence the chip acknowledges.
  - The driver says where the aid's place came from: "position from the last fix", "from the zone in effect, <zone>" (2026-11-11), or "from none".
- **Not checked:** the replays' `92 81` and `8e 81` messages are not decoded, so I can't rule out that they also carry a place. The capture's own files stay on Harbor.
- **Passive reads and the hourly fix** (proposed 2026-11-11, LOCATION.md): a `place` right that reads the last place without turning the radio on, a place service that keeps it from every fix and the serving cell, and at most one fix an hour of its own, screen on. Nothing the chip learns survives a session today (its rails go off with the driver, its firmware is reloaded each start): LOCATION.md "GPS: what a fix costs".
- **A first fix with no last position starts cold** (about 5-8 minutes, measured with a zeroed position); since 2026-11-11 it starts from the zone in effect's principal place when there is one ("A rough start from the zone"). Every fix after it, in that person's session, starts from their own last fix.

## The satellites (2026-11-07)

The fix message also carries every signal the chip tracks, one 56-bit record each at its end.
Decoded against gpsd's own output for all 213 fixes of the on-chip capture (lhd4.trace with logcat3.txt: `$GPGSV`/`$GLGSV`, `$GNGSA`, `$PGLOR,SAT`), with a scratch Rust decoder that includes the driver's own `payloads/gps/src/sky.rs`.

Where the records are, bit numbers as in "The fix message" (little-endian 32-bit words, MSB first, from the class's first bit):
- They start at bit 585, or 617 when bit 213 (the track flag) is set: the track brings a 32-bit block (the track s16 at 439 and 16 bits more), and everything after it moves.
  Bit 213 is the only flag in bits 200-220 that varied in the capture (167 of 213 set).
- The count is a u7 17 bits before the first record (bit 568 or 600).
- The message ends 16 bits after its body, and the body is the records' end less one bit rounded up to a whole 32-bit word: `len × 8 = ⌈(start + 56 n − 1) / 32⌉ × 32 + 16`, true of all 213.
  The driver checks it before reading a record; a message that fails is counted (`fix messages laid out otherwise` in its 32 s report) and gives no satellites.
- There is no other per-satellite message in on-chip mode: besides the fix (G#16) the chip sends only a 14-byte `01 81` id 08, a 122-byte class 04 message, and 22-byte class 03 messages, none of which grows with the satellites.

One record, its bits in order:

| Bits | Field | Values seen |
|---|---|---|
| 0-5 | PRN u6 | GPS PRN; GLONASS the orbital slot (NMEA 64 + slot, `$PGLOR,SAT` `R` + slot) |
| 6 | used | `$PGLOR,SAT` flag G#02 |
| 7 | always 1 | |
| 8 | unnamed flag | `$PGLOR,SAT` flag G#20; clear on the strongest satellites in the fix |
| 9 | always 1 | |
| 10-15 | system u6 | 0 GPS, 8 GLONASS |
| 16-23 | elevation s8, degrees | 5 to 75 |
| 24-39 | azimuth u16, degrees | 0 to 360 (360 is north as well) |
| 40-47 | band u8 | 0 L1 (GLONASS G1), 2 L5 |
| 48-55 | C/N0 u8 | gpsd's dB-Hz = ⌊(raw + 16) / 4⌋ |

**Match:** 3032 records in the 213 messages, each one found as a row of gpsd's GSV (3032 rows): elevation, azimuth and C/N0 3032 of 3032; the count against gpsd's distinct satellites 213 of 213 fixes; C/N0 and both flags against `$PGLOR,SAT` 2857 of 2857 L1 records.
The used bit against `$GNGSA` agrees on 2844 of 2857 L1 records: the 13 others are `$PGLOR,SAT` flag G#33, which gpsd leaves out of GSA (it lists G#3F, G#3B and G#1F), and nothing in the record tells G#33 from G#3F; the satellites-in-fix count then matches gpsd's on 203 of 213 fixes.
L5 records carry no used bit of their own (always clear), and gpsd's GSV gives them no elevation or azimuth; the chip's are the same as the satellite's L1 record (166 of the 175 L5 records had one, all 166 the same).
The C/N0 offset is fixed only to gpsd's integer: ⌊(raw + 16) / 4⌋, round((raw + 14) / 4) and ⌈(raw + 13) / 4⌉ are the same function, so the true value is (raw + 13 to 16) / 4; the driver keeps raw + 16 in quarter dB-Hz.
Not seen in the capture, so not known: Galileo, BeiDou and QZSS system codes, L1C/E1/E5a/B1/B2a band codes, and a GLONASS frequency channel (it is not in the record).

**No Doppler.**
The record is fully accounted for above: no Doppler, range rate or pseudorange in it, and no other message carries them in on-chip mode (in host mode the chip streams measurements at ~18 KB/s, which would have them, but then the position is ours to compute).
A satellite's line-of-sight velocity therefore needs its orbit (the ephemeris or LTO), not the chip's report; its motion across the sky comes from successive elevations and azimuths.
Those are whole degrees, and satellites cross the sky at about half a degree a minute: in the capture's 2908 pairs of successive fixes the elevation changed 18 times and the azimuth 15, never by more than one degree.
So the driver keeps both: the plain change since the previous fix that had the signal (mostly 0), and the rate between the first and the latest one-degree step seen on each axis, in millidegrees a second (the capture's 3.5 minutes gave 7 to 9 mdeg/s where two steps were seen, about 0.5° a minute).

**In the driver** (`payloads/gps/src/sky.rs`): a fixed table of 64 signals, two of them swapped at each message, a signal found in the previous one by (system, band, PRN).
Op 2 SATS on the `gps` endpoint serves the latest list (layout in main.rs's header: 25 bytes a signal), and the measure slots carry SATS_SEEN (a satellite on L1 and L5 counts once), SATS_USED and CN0 (the best, milli-dB-Hz) beside the state and the error estimate.
`gpstest 02` prints the list.
Satellites come only with fix messages: while the chip searches before its first fix there is no list (next section).

## What the chip sends before a fix (2026-11-11)

Owner, 2026-11-11, the Pro's GPS page showing zero satellites for hours: "can't we figure that out from the orbital data anyway? Like there's zero satellites showing on this page is the thing" ... "Please. I'd like to see what's actually going on in this thing."
The driver's 32 s report then said about 2,900 frames per 32 s (90 a second), some 2 KB/s, 0 fix messages: some 22 bytes a frame on the wire, none decoded.

**The capture has no message from the chip before an on-chip fix.**
Every CRC-good frame of the captures, counted by its kind (flags, the second flags byte, the payload's first two bytes, the inner frame's class; the scratch decoder that found the satellites):

| Capture, stretch | Kinds (count, bytes a frame deframed) |
|---|---|
| lhd4 (on-chip), the configuration's end (406.2 s) to the first fix (415.2 s) | acks `05` ×5 (6 B): nothing else |
| lhd4, 32 s after the fix (450-482 s) | `1f80/0181` ×64 (108 B: the fix G#16 and the 14-byte id 08, one each a second), `1f80/0381` ×32 (35 B), `1f80/0481` ×32 (135 B) |
| lhd3 (gpsd's own engine on the chip's measurements, 60-92 s) | `1f80/0280` ×5403 (114 B), `1f80/0481` ×814 (135 B), acks ×231, flags G#40 `1f80`/`1f81` `0280` ×258 (some 265 B) |

- In on-chip mode the chip says nothing between the end of its configuration and its first fix (9 s in lhd4); after it, the four messages a second above, satellites only inside the fix.
- `02 80` is the measurement stream gpsd's own engine reads (its log's `GlPe_MeIfImpl::ChipData`): 100 to 200 messages a second, 10 to 25 KB/s, a byte stream split across messages (a length u16 after the checksum, then the stream, G#E0 marking records of its own). It carries the per-satellite measurements gpsd turns into a position, so it is the place tracking state would be; it is not decoded, and it comes only in host mode, which the driver's replay is not.
- The Pro's 90 frames a second at 22 bytes match neither: no layout for satellites before a fix can be decided from the capture.

**So the driver counts what comes, by kind, on the device** (payloads/gps `tally`, op 4 FRAMES):
- Every CRC-good frame is counted by its kind (key `[flags, second flags byte or 0, the payload's first two bytes, the inner class]`, 16 kinds at most, more counted as "past the table"), with its bytes and when the latest came; the latest two frames of each kind are kept (256 bytes each), never a fix message (its position is the person's).
- The 32 s report has a second line: `gps: frames by kind, the last 32 s: 05/0000/0000 x5 (6 B) 00/1f80/0381 x32 (35 B) …` (flags, `.`second flags byte, `/` the RPC, `/` the class, the count since the last report, the mean bytes deframed). It says what the chip sends without a byte of what it says.
- `gpstest 04` lists the kinds with their counts; `gpstest 04<k>` (`0402`: kind 2) prints kind k's latest two frames in hex, 32 bytes a line. Those bytes are the chip's: a kind other than the fix could echo the aiding's place, so a capture of them stays on Harbor.
- Reading a kind: `80.01/0101/0000` is the chip's sync packet (it restarted its transport and waits for the host's); `05/0000/0000` an ack; `00/1f80/0280` the measurement stream (the chip working as gpsd's measurement engine); `00/1f80/0381` and `0481` its per-second companions of the fix.
- FIX's reply carries the frame count and the kinds after the fix (bytes 33-37), and WATCH's change count moves every 64 frames, so the map's GPS page says, before a fix, how many messages of how many kinds have come (ferros_lang `Say::ChipHeard`), moving as the chip talks. The console's GPS circle says "searching" (the driver's STATE 1) instead of "no satellites"; it has no rings to draw until the first fix brings satellites.

The owner's other thought, the sky from the orbits: the satellites above a place at a time follow from an almanac or the LTO file and a rough position, with no word from the chip. ferros has no almanac of its own yet (the replay's LTO is the capture's, past its 7 days); a predicted sky would say which satellites should be up, not which the chip hears, so it would sit beside this, not replace it.

## The position aiding, d0 81 (2026-11-11)

Decided from the two sessions that sent it: lhd4 (three identical messages at 406.1-407.0 s) and lhd2 (three at 1478.7-1479.7 s), 82 bytes from the class, byte offsets from the class, little-endian (no place written here: the position words are masked in what was read):

| Offset | Size | Field |
|---|---|---|
| 0 | 2 | class `d0 81` |
| 2 | 2 | length, 80 (the bytes after it) |
| 4 | 2 | checksum (`87 81`'s rule) |
| 6 | 2 | 0 in both |
| 8 | 4 + 4 | latitude, 32.32 fixed: the high word in 2⁻¹⁶ degrees (what the driver writes), then its fraction word |
| 16 | 4 | 6 in both |
| 20 | 4 + 4 | longitude, 32.32, the high word in 2⁻¹⁵ degrees west positive, then its fraction |
| 28 | 4 | 7 in both |
| 32 | 4 + 4 | a 32.32 value, a few tens (of metres, if metres) in each, the two sessions eight apart |
| 40 | 4 | 8 in lhd4, 7 in lhd2 |
| 44 | 4 + 4 | a 32.32 value, a few tens in each, the two thirteen apart |
| 52 | 4 | 13 in lhd4, 10 in lhd2 |
| 56 | 2 + 2 | GPS week, then 0 |
| 60 | 4 | time of week, ms |
| 64 | 4 | 0 |
| 68 | 2 | 436 in lhd4, 455 in lhd2 |
| 70 | 2 | 98 in both |
| 72 | 8 | 0 |
| 80 | 2 | the RPC trailer counter |

- The fraction words are why the driver writing the high words alone works: they are under 2⁻¹⁶ degrees, two metres at most.
- **The uncertainty field is not decided.** The two values at +32 and +44 are an altitude and an uncertainty by their size and their pattern (each a 32.32 followed by a small integer, like the position's), but which is which the capture cannot say: gpsd logs its aiding position with `HorzUnc` and `VertUnc` (`GlPeAsstPosMgr: New position`) only in host mode, which never sends `d0 81`, and the two sessions that did have no plain log of it (lhd4's gpsd writes it in its encoded form, `~^~{…}`; lhd2 has no log). The small integers (6, 7, 7 or 8, 10 or 13) and the u16 at +68 are not known either.
- The candidate for the horizontal uncertainty is +44 (its high word whole metres): a few minutes after a session of fixes, tens of metres is what gpsd's propagation gives. The driver writes it only when asked (input bit 2, `Aid::unc`): the test is one start with the zone's place and the candidate set against one without, from the same place. A first fix much sooner with it set says +44 is the uncertainty; a chip that never fixes says it is the altitude.

## A rough start from the zone (2026-11-11)

With no last fix of the person's (`gps.last`), the `d0 81` aiding carries the zone in effect's principal place instead of the zeroed one: the zones document's city for that zone (zone1970.tab's, TIME.md "The zones document"), from the kernel (SYS_ZONE op 7, the GPS driver only, ferros_sys `zone::place`, arc-seconds to 2⁻²³ degrees in the driver).
- Never a precise place: a city anyone can read in tzdata, which says only which part of the Earth the device is in. The console line says which source was used and the zone's name, never the numbers: `gps: position from the zone in effect, <zone id> (its principal place)`. No zone in effect (a fixed offset), or no zones document: the zeroed place, as before (`position from none`).
- A zone's principal place can be 2000 km from the person (the Pacific zone from Los Angeles reaches Seattle at 1550 km), about 18° of the Earth's arc: the chip still looks at the right half of the sky, where the zeroed place (the Gulf of Guinea) is the wrong one for most of the world.
- The uncertainty stays the replay's (above) unless the driver is asked to write the candidate: `ZONE_UNC_M` = 1 << 21 m (some 2100 km), covering nearly every zone from its principal place.
- The zone follows the device's choice: found by the cellular network (NITZ and the MCC's country, before any fix) or by an earlier position, or set by hand.

## No fix, 2026-11-18: the replays lost their aiding

Owner, 2026-11-18: "GPS. what's the deal there? ... right now I get nothing. it did work sporatically before."
Measured on the Pro (boot b976, indoors, night), the driver started by the GPS puck's claim, then twice more by hand.

**What the chip did.** `gpstest 04` (the driver's frames by kind) and its 32 s reports, three starts:

| Start | Replay | What came | Then |
|---|---|---|---|
| the puck's claim, 19:57 | `gps.replay` (the kernel's restart gives no input) | 3455 frames in ~95 s: `02 80` ×1768 and its large forms (the measurement stream), `04 81` ×997, `a3 81` ×2, no `01 81` | silent: no HOST_REQ for 23 minutes, the driver asleep in its interrupt wait |
| `diag "restart gps"`, 20:24 | `gps.replay` | the same: HOST_REQ 2551, 3240, 3139 times in three 32 s reports, 4900 frames, 0 fix messages | silent again ~110 s after power-up |
| `kill gps`, then the driver spawned with input 01, 20:33 | `gps.replay.sky` | 812 frames, nearly all during the replay; `a0 81` ×5 (the chip's time: GPS week 2445 and a time of week matching the clock: it took the `87 81`) | silent for the 9 minutes watched, no fix (then, about 20:44, the Pro left ferros for Android; the test had only killed and spawned processes, so most likely another session's doing, not checked) |

So the screens showed nothing because there was nothing: no fix message ever came, so no satellites (they ride only in the fix message) and no position. The driver ran and answered; its 32 s report stops when the chip stops, because the loop only wakes for HOST_REQ or a call.

**Why: the replays in the vault are not the ones this document describes.** `tools/gps-replay-scan` (a host tool: every message a replay sends, by RPC and inner class, no payload bytes) on the vault export of that morning (keys/husky/vault-export-2026-11-18, taken before the fresh start and restored after it, so the vault held them before as well):
- `gps.replay`: 554 records, 9.2 s of lhd's gaps: the patch, the second stage, a host-mode session's start (`1e 81` carrying `8c 81` ×3, `83 81` ×2, `8f 81`, `92 81`, gpsd's periodic `1e 48`), cut off at 9.2 s. No `87 81` time, no `d0 81` place, no `82 81` LTO windows, no configuration strings `10 00`/`17 00`, no `09 00`. The on-chip session of 2026-10-25 had 533 records over 18 s with all of those. This is a host-mode session: the chip becomes gpsd's measurement engine (`02 80`), nobody answers as gpsd would, and it stops after ~100 s. The 554-record replay of lhd's opening (2026-10-25, "554 records in 3.3 s" above) is the likely source; how it came back is not known.
- `gps.replay.sky`: 509 records, 17.3 s: the on-chip session with its configuration and the `87 81` time, but no `d0 81` (the 517-record sky replay of 2026-10-26 had three, and `8e 81` ×3, `83 81`, `92 81`: by the count, those eight messages are gone, the sequences closed up over them). With no place the chip starts cold; above, a cut or zeroed place cost 5-8 minutes outdoors, and indoors at night it found nothing in 9.
- The driver's line "position from the zone in effect" was true of what it chose, but neither replay had a `d0 81` to carry it: the place never reached the chip. The driver now counts what the replay carried and says so after the replay (`gps: the replay carried <n> time (87 81), <n> place (d0 81) and <n> 09 00 messages`).
- The capture traces (lhd2, lhd3, lhd4) were on /mnt/Harbor/tmp, which has been cleared: the 533-record on-chip replay cannot be rebuilt from them.

"It did work sporadically before" fits: the replays with the place and the LTO gave fixes in late October (11-18 s, then 78 s as the LTO aged, 92 s with the last fix and no LTO); by 2026-11-11 the Pro sat for hours with no fix, the first sign of these replays.

Not explained yet: the kernel counted 3.3 million unmasks of `gps-host-req` in that boot before the first start went quiet, against some 2500 HOST_REQ passes the driver counts per 32 s; worth a look at the interrupt's level handling once the chip runs again.

## Proposed: a sky before the lock, and a faster lock (2026-11-18, for the owner)

### The birds before a lock

The sky above a place at a time follows from three things, none of them the chip:
- **Orbits.** CelesTrak's GP elements (`http://celestrak.org/NORAD/elements/gp.php?GROUP=gnss&FORMAT=tle`, 28 KB for every GNSS satellite; `GROUP=gps-ops` 5 KB) and its GPS almanac (`http://celestrak.org/GPS/almanac/Yuma/almanac.yuma.txt`, 19 KB, the current week's) both answer plain HTTP 200 (checked 2026-11-18), so ferros's own TCP reaches them. NAVCEN's YUMA/SEM did not answer plain HTTP (a timeout; HTTPS redirects). GP elements with SGP4 cover GPS, GLONASS, Galileo, BeiDou and QZSS in one form; a GPS almanac is the IS-GPS-200 Kepler equations, a few dozen lines. Either is good to a few km for a week, under 0.05° of sky.
- **Time.** ferros's clock: the satellites move about half a degree a minute, so a second of error is nothing.
- **A rough place.** Its error is what matters: moving the observer 100 km tilts the local horizon by 0.9° and shifts the parallax by up to 0.3°, so about 0.7° of sky per 100 km. A city-level place (10-50 km) is a few tenths of a degree; the zone's principal place (up to ~2000 km) up to ~15°, still the right birds in the right part of the sky except near the horizon.

About 30-45 satellites are above 5° at any time with the four constellations, inside the 64 tracks the driver publishes.

**Where the place comes from, best first, combined (the single egg rule):**
0. `gps.last`, the person's last fix (identity class, Eagle time stamped): metres, widening with its age.
1. A cell map the device learns itself: each fix stamps the serving cell (the modem's MCC, MNC, TAC and ECI) with the place, in the identity's vault; a later start in a known cell is good to the cell's size (a few km), and LTE's timing advance gives the distance to the tower in 78 m steps. No outside database, nothing asked of anyone.
2. fgtw.org's view of the request: the worker's `request.cf` carries a latitude and longitude at city level (workers-rs `Request::cf()`), so the reply to a call the device already makes (the signed exchange) can carry it, signed with the seed's key. Good to tens of km on a home WiFi; over LTE the address is the carrier's gateway, which can be far off (not measured yet).
3. The zone in effect's principal place (built, 2026-11-11): up to ~2000 km.
4. The MCC's country.

Each is a circle (a centre and a radius). The tightest circle is used only when every independent looser one agrees with it (its centre inside theirs, widened by both radii); a tight one that two looser ones contradict (an address behind a VPN in another country, say) is dropped and the console says which. The place is the person's: identity class like `gps.last`, never on a measure anyone reads, never in a log.

**Who computes it:** the GPS driver, while it runs (claimed), so the sky still shows only while GPS is in use. It reads `gps.almanac`, a VSF document of the elements (device class: orbits are public), refreshed at most daily by the FGTW node from fgtw.org (fgtw.org fetches CelesTrak and, for more than one source, NAVCEN over HTTPS, and signs the document), or by a plain HTTP fetch of CelesTrak's text with a fixed-column, checksummed parser in one place, as a second path. Each predicted bird is published as a track (`ferros_sys::Sat`) with C/N0 0 and not used: no new bit, a signal heard replaces its prediction by system and number.

**On the GPS rings** (`gps_rings`, the circle's satellites as bubble limbs meeting over the person): three looks, every one C∞.
- **Predicted** (from the orbits, not heard): the limb at a weight of 2⁻³, in the flavour moved toward white, and faded near the disc's centre by 1 − e^(−(r/r₀)²) (r₀ a quarter of the disc), so the meeting point over the person brightens only with real signal. Faint arcs pointing where each bird is.
- **Heard** (C/N0 from the chip, not in the fix): weight 1/(1 + e^(−(C/N0 − 28)/4)) / 2, about 0.06 at 20 dB-Hz and 0.48 at 40.
- **Used in the fix**: weight 1, the full 550 nm flavour, as built.

The GPS puck's sky plot the same way: predicted a hollow ring, heard a disc sized by C/N0, used the `USED` colour.

Pre-fix, in on-chip mode, the chip says nothing per satellite (above, "What the chip sends before a fix"), so before the first fix the circle can show predicted birds only. Heard-but-not-used before a fix needs the host-mode `02 80` measurement stream decoded; not proposed now.

### Aiding the chip so it locks faster

Known, enough to build: the SSI framing, the packet transport and its CRC, the RPC wrapper (`1e`, the inner frame's length + 2, the inner frame's length, 0; e.g. `1e 18 16 00` before the 22-byte time message), the inner frame (class, length, the 16-bit checksum, the trailer counter), `87 81` (week u16, time of week u32 ms, then 1000, likely the time's uncertainty in ms), `d0 81`'s layout (the table above, all but which of +32 and +44 is the uncertainty), and cutting messages from a replay with the sequences and trailer counters closed up (proven 2026-10-26).
- **Now:** the place is missing from both replays. The driver can put one back: build a `d0 81` from the table (the capture's small integers as they are listed, the place from the list above, +32 and +44 tested both ways) and insert it after the `87 81`, every later frame's sequence and trailer moved on by one, the inverse of the proven cut. With the person's last fix and no LTO this replay gave a first fix at 92 s (2026-10-26).
- **And the kernel should start the driver on the on-chip replay:** `gps.replay` as stored is a host-mode session (it can never give an on-chip fix); the restart passes no input, so it is what the puck and the map's page get. Either the restart passes input 01 (`gps.replay.sky`), or the store gets a corrected `gps.replay` (the owner's call; nothing was stored on 2026-11-18).
- **Not known, to reverse:** `8e 81`, `83 81`, `92 81`, `8c 81`, `8f 81`, and `82 81` (the LTO windows gpsd computes from `lto2.dat`). In on-chip mode gpsd uploads no ephemerides (its log: "GenerateAiding() skip because it's onchip ME"), so the on-chip engine's orbit aid is the LTO windows. Two ways in: a fresh capture on rooted Android with gpsd at LOG_DEBUG (its log names what it injects) beside the same day's broadcast ephemerides to correlate field by field, or gpsd's builders read from its disassembly. The capture also brings back a whole on-chip replay.

### Fresh aiding without Google's servers

- The LTO file is Broadcom's (glpals.com, Global Locate), licensed to Google; it still answers plain HTTP (`7day/v5/latest/lto2.dat`, 186 KB on 2026-11-18). Kept as the fallback only, and useless until `82 81` is decoded.
- Broadcast ephemerides, every constellation, from the IGS: BKG (`https://igs.bkg.bund.de/root_ftp/IGS/BRDC/<year>/<day>/`, `BRDC00WRD_S_…_01D_MN.rnx.gz`, the day so far, updated through the day) and ESA (`ftp://gssc.esa.int/gnss/data/daily/<year>/<day>/`, `brdc<day>0.<yy>n.gz` 58 KB for GPS, `.g` 83 KB for GLONASS, `BRDC00IGS_R_…_MN.rnx.gz` 1.2 MB for all). Neither answers plain HTTP and CDDIS wants an Earthdata login, so they are fetched and converted on fgtw.org or the host: RINEX to a VSF ephemeris document per day, signed, served to the device over plain HTTP. Two sources for one input. Into the chip once its ephemeris or LTO message is decoded; on the screen they are not needed (the almanac is plenty).

### What the owner would see

0. Today: the GPS circle lit and "searching", no rings; the puck's 0 and 0 and an empty sky plot; the map's GPS page counting the chip's messages, which stops ~100 s after GPS turns on, because the chip has gone quiet.
1. The driver started on the on-chip replay with a place put back: the same screens until the first fix, then rings and dots; about 1.5 minutes near a window (the 2026-10-26 figure), longer indoors.
2. The predicted sky: the moment GPS turns on, faint arcs for every bird above the horizon, before any lock and indoors too; heard birds brighten as the chip reports them, used birds go full. The point over the person stays dark until there is real signal.
3. Aided starts (the LTO windows or broadcast ephemerides decoded and injected, the place from the list above): first fixes in seconds to tens of seconds, and more of them indoors.

The 8a (Samsung KEPLER, booted through the modem's `gnss_boot`, data on SPI 21, BOARDS.md): out of scope here. It needs an Android capture of its HAL's traffic (akita-root has Magisk), its firmware path, then a driver of its own; the predicted sky above works there unchanged, since it needs no chip.

## Built 2026-11-18 night: the right replay, a place put back, the predicted birds

Branch gps-rough-fix. Measured on the Pro (b981, its flashed kernel, the driver hand-spawned, indoors at night).

**The right replay.** The driver takes no input as the on-chip replay (`gps.replay.sky`), as input bit 0 is; the kernel's start on a claim now passes 01 as well, so either side alone is enough. `gps.replay` (the host-mode session) is used only when asked by name (input byte with bit 0 clear).

**The reliable counter (found on the device).** The byte flag G#02 brings on a host frame is not lhd's ack of the chip: it is a counter, +1 for every reliable frame (0, 1, 2 … from the second stage's sync, every reliable frame of both replays). The chip acks a frame with flags G#25 (bit 0 the sequence, bit 2 the counter it took, bit 5 a byte not known) and refuses one whose counter is not the last plus one with flags G#29 (bit 3 in place of bit 2, the last counter it took beside it), and refuses every frame after it. Cutting messages closed up the sequences and the trailer counters but not this counter: `gps.replay.sky` jumps G#3D → G#43 (record 505, the `d0 81` and others cut) and G#43 → G#56 (record 506, the LTO windows cut on 2026-10-26), so **its last four messages (`03 00`, both `09 00`, `06 00`) were refused at every start**, the 2026-10-26 runs included. `tools/gps-replay-scan` now lists the jumps. The driver gives every reliable frame the next counter as it replays (`Renumber`): 5 renumbered, 0 not acked in time, against 5 refused (five G#29 replies in `gpstest 04`) in the first run, which renumbered nothing.

**The place put back.** After the replay's time message (`87 81`), when the replay carries no `d0 81`, the driver builds one (`place_record`: the table above, the capture's small integers, 32 at +32 and +44) as its own record, the time frame's sequence, counter and trailer plus one, then `patch_aiding` fills the place and the time as it does every replay's; every frame after it moves on by one (sequence, trailer, counter). The chip acked it. Which field is the uncertainty is still open: input bit 2 writes the place's radius at +44, bit 3 into the words after the latitude and the longitude (6 and 7 in the captures: plausible as metres after a session of fixes at ±6-8 m); neither was tried with a sky in view.

**The place's sources** (ferros_gnss::place, the single egg rule): each a circle; the tightest stands while each looser one overlaps it, and one that does not widens it to the smallest circle holding both, so no source silently wins. Today: the last fix (`gps.last`, its circle 2^6 m + 2^5 m/s of its age, 2^21 m with no time kept) and the zone's principal place (2^21 m). On the Pro there was no `gps.last`, so the zone alone: "position from the zone in effect's principal place (1 source …), the circle A#2097 km". fgtw.org's rough place is proposed below; the cell map (above) is not built.

**First fix: none in 9.5 minutes** (indoors, night, the zone's place 2000 km wide, no LTO), as on 2026-11-18 before; the chip took its time (`a0 81` ×5). The place and the counters were the two things known wrong; whether they shorten a start needs a run with the sky in view, and the uncertainty field decided (bits 2 and 3).

**The predicted birds** (payloads/gps predict.rs, ferros_gnss):
- The orbits: CelesTrak's GNSS GP elements (`http://celestrak.org/NORAD/elements/gp.php?GROUP=gnss&FORMAT=tle`) over plain HTTP thru `ip` (grant slot 9, `udp`, added to the kernel's list), read by a fixed-column parser with both lines' checksums, kept as `gps.orbits` (ferros_kept::gnss: a VSF document, `fetched` Eagle time, a column per element; device class), fetched again after 2^16 s. On the Pro: 143 satellites of the four systems, 0 refused, in some 0.4 s after the replay.
- The maths: the elements carried by SGP4's own secular rates (its Kozai undone, WGS-72, J2, J2², J4), TEME to the earth by the mean sidereal time, the place on WGS-84. Against the same day's GPS almanac (Yuma, IS-GPS-200's equations): every GPS satellite within 40 km and 0.1 degree of sky (src/tests.rs).
- Published as tracks after the heard signals: C/N0 0, not used (no new bit); a heard signal of the same system (or a code the chip names that is not known yet) within 4 degrees, or the same GPS or BeiDou number, stands for its bird. Worked out again every 2^6 s while they show (the wait's deadline: half a degree of a satellite's travel), from the fix once there is one. On the Pro: 51 birds the moment the orbits came (GPS 14, GLONASS 8, Galileo 12, BeiDou 17; `gpstest 05` reads the tracks as the display does).
- The console's GPS rings: used at full weight, heard by 1/(1 + e^(−(C/N0 − 28)/4)) / 2, predicted at 2^-3 as a second layer under the heard ones, in the flavour halfway to white, faded toward the centre by 1 − e^(−16 t²). Built and run on the Pro (the display restarted from a capsule of this branch: its console draws took 2.4 ms each against 1.6 ms before, the 51 rings the likely cost), not seen by eye: the capture showed the deck, not the corners. The stored display was put back after.
- The GPS puck's sky plot reads the same tracks: used a full dot, heard a dot sized and weighted by C/N0, predicted a hollow ring at an eighth, faded toward the zenith. Built and host-tested; not on a device.
- The tracks are readable by anyone (SYS_TRACKS), as the heard ones were: a sky in whole degrees says where the device is to some tens of km.

### Proposed for fgtw.org: a signed rough place

For the Photon session that owns fgtw-bootstrap; nothing here changes fgtw.org.

- **Request:** `POST http://fgtw.org/` (plain HTTP, as every seed call) with a VSF document holding one section `rough_place_ask`: `nonce` (`hb`, 32 bytes fresh from the device's entropy). No identity, no device key: the question is about the request's address, not the asker.
- **The worker:** dispatched by the section name as the others; reads `req.cf()` (workers-rs `Cf`): `coordinates()` (latitude and longitude, city level), `asn()`, `country()`. Keeps nothing, logs nothing.
- **Reply:** a VSF document signed with the seed's key (as its other signed replies), section `rough_place`:
  - `at` (`w`, the coordinates as vsf's WorldCoord), rounded to 2^-7 degree (under a kilometre: Cloudflare's own figure is a city's centre already);
  - `radius` (`u`, metres): 2^15 when Cloudflare gave a city, 2^19 when only a region, no section at all when it gave no coordinates;
  - `asn` (`u`), so the device can tell a carrier's gateway (often far off) from a home line;
  - `country` (`a`, the two ISO letters as Cloudflare gives them; a code, not a sentence);
  - `nonce` (`hb`, the request's, echoed: the reply answers this question and no replayed one);
  - the document's creation time (Eagle) is when it was answered.
- **The device:** checks the signature against the pinned seed key and the nonce, keeps the answer as `gps.rough` (identity class, beside `gps.last`: kvault.rs `IDENTITY` needs the prefix), and the driver adds it as a source circle. A carrier ASN widens it (2^19 m) unless the zone and the last fix agree with it. Over a VPN it lands in the wrong country, which the zone then contradicts and the combination widens to cover both.

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

## akita's KEPLER, captured under the hv (2026-11-18)

The 8a's GNSS is a Samsung part Google's tree calls KEPLER (`samsung,exynos-gnss`, `gnssif`), not husky's BCM4776 (BOARDS.md "akita's radios").
Captured with Android 17 on slot a under an oracle hv (tools/akita-hv-camcap.sh, images `gnss` and `gnss-boot`), decoded with `tools/gnss-oracle`; local only, keys/akita/captures-2026-11-18/ (`gnss-*`), since the session's reports carry the place.

**The bus is SPI, nothing else.** spi_21 (USI3_HSI0, G#111E_0000, 3.125 MHz, mode 0) in PIO: no DMA, no shared memory, no mailbox, and the kernel's driver (`gnss_spi.ko`, `gnssif.ko`) neither powers nor resets the chip.
- Transfers are 32-bit words both ways (TX_DATA +G#18 writes, RX_DATA +G#1C reads; with SWAP_CFG on, each word's little-endian bytes are wire order); PACKET_CNT (+G#20) is written with bit 16 and the word count before each transfer and 0 after.
- The handshake: AP to GNSS is gpp4-3 (PERIC0 G#1084_0084 bit 3) raised before the AP sends and pulsed before each read; GNSS to AP is gpa5-3, the chip's interrupt (Android's IRQ 226).
- A send: raise gpp4-3, wait for gpa5-3, one transfer of the message's words, then a read of as many, gpp4-3 low. A read: the chip raises gpa5-3, the AP pulses gpp4-3 and reads 1024 bytes (256 words), again while gpa5-3 stays high; the reads are mostly fill, so a packet can span two.
- The framing is CHPP (the context hub peripheral protocol): preamble 68 43, an 8-byte transport header (flags, packet code, ack sequence, sequence, length, reserved), the payload, CRC-32; every packet of both captures checks (25 candidates in 99 KB of fill fail, none of them a real packet). Both ends offer CHPP 1.0.0, an MTU of 1280, a window of 1 and a 1000 ms timeout in their transport reset; the AP's NAK code 5 (21 times in the session) is CHPP's out-of-order answer.

**At gnssd's start (a boot):** transport reset and its ack, discovery (handle G#0F, a 42-byte answer naming the chip's one service), the time sync (handle G#02), then the service's open on handle G#10 (cmd 0 asked and answered), its first notification (994 bytes, more following), and the AP's first message to it.
**A session to a fix** (Maps asked for location; the first fix 10 s after it opened, indoors): every message from then on is on handle G#10, the AP's as client notifications (cmd 2), the chip's as service notifications (cmd 3): Samsung's own protocol (gnssd's BETP client) inside CHPP.
The AP sent four (46 or 47 bytes; their inner ids G#12DE, G#04D4, G#0398, G#12DE); the chip 189 (28 KB), most of them 34-byte G#3448 messages, the rest G#035B, G#0249 and fragmented 994-byte G#710A to G#730A ones (measurements and reports: the place among them, never printed).
**The firmware load was not seen:** `/vendor/firmware/kepler.bin` (1026920 bytes on this build) goes over the same SPI as BETP bootloader messages (gnssd's `spi_codeload`), but neither capture holds it, from the hv's start or in the session: the chip kept its firmware across every warm reset (rails on), and gnssd found it running. A cold start (power off and on, the owner's hand on the power key) under the `gnss-boot` image would hold it.

**What a ferros driver needs:** spi_21 in PIO at 32 bits a word with the two handshake lines (no modem, no shared memory); CHPP's transport (reset, sequence and ack, CRC-32, NAK, 1280-byte MTU), discovery and the open of handle G#10; then gnssd's BETP messages: the four the AP sends are in the capture, the chip's answers to read; and, from a cold chip, the code load (kepler.bin over BETP's bootloader messages) once its capture exists.
