# Touch (ST FTM5 "fts") — register oracle and cold bring-up

The touch controller is an ST FTM5 on SPI, behind `spi@111D0000` (BLK_HSI0 USI2, `spi_20`), driven on Android by Google's public GPL driver `kernel/google-modules/touch/fts_touch` (`ftm5/`, branch `android-gs-shusky-6.1-android15-qpr2`, cloned at `/mnt/Harbor/ferros-ref/touch-fts`).
Source plus one clean oracle capture of the boot-time probe are enough to drive it; no unbind/rebind capture is needed (unbinding `fts` panics the kernel in `of_drm_find_bridge`, 2026-10-19).

## Capture

Boot-time probe with only the SPI page trapped and the kernel booted at `loglevel=4` (the hv script's `HV_CMDLINE`), so the boot log stays out of the pstore ring the capture travels through:

```bash
FERROS_HV_NO_PMU_READ=1 FERROS_HV_STAGE2=1 FERROS_HV_ORACLE=1 FERROS_HV_ORACLE_PAGES="G#111D_0000" FERROS_HV_ORACLE_CAP=30000 HV_CMDLINE="loglevel=4" tools/husky-hv.sh --flash
# after boot: marker, sysrq crash to flush, read console-ramoops-0
python3 tools/oracle-decode-spi.py < raw.log > touch-boot-probe-transfers.txt
```

`touch-boot-probe-transfers.txt` is the decoded result: 27 SPI transactions, no integrity gaps.
Leave this build only as long as the capture takes: idling in Android with every touch read trapped ended in a watchdog reset.

## Wire protocol (from the capture, matched against `ftsIO.c` / `ftsCore.c`)

- SPI mode 0, byte transfers, CS held across a command and its read. Every read returns one dummy byte first (the command echoed back).
- `87` then read 1+8: one FIFO event. `00`/`FF` in byte 0 = nothing pending.
  The queue is drained the way `fts_interrupt_handler` drains it: byte 7 of an event carries in its low five bits how many events are still queued, so one read is followed by bursts of the rest (seven events and the dummy byte fill one 64-byte SPI segment, so a full 32-event queue takes five transfers).
  Reading one event at a time races the queue cursor: the same event comes back a second time with its tail bytes zeroed, which decodes as a contact jumping to the top of the screen at zero pressure.
- `FB aa aa aa aa` then read 1+n: hardware register read (32-bit big-endian address). `FA aa aa aa aa dd..`: hardware register write.
- `A7 00 00` then read: framebuffer read (the system-info block). Android runs this one with RX DMA, so the oracle saw only the command.
- `A0 00 FF`: scan mode active, all sensing on — but `setScanMode` (ftsCore.c) is **two** writes, and the first is what makes it work: `FA 20 00 00 00 00 00`, a two-byte zero write to hw reg G#2000_0000, precedes the mode command every time.
  Without it the mode command is accepted silently and the controller never reports a contact, while the capacitance frames still show a finger perfectly — the chip senses but never runs the reporting pipeline.
  G#2000_0000 is the address the chip ID is read from, so in the capture the write reads like a mistake; it is not. `C0 ..`: Google custom commands (grip suppression `C0 03 10 FF 03 ..`, `C0 0B 03`, `C0 0F 00`, `C0 0E 00`), each echoed as a `43 01 <cmd>` status event.

Probe order: poll `87` until `03 00 ..` (controller ready) after the reset pin rises; read hw reg G#2000_00DE and write it back with bit 7 set (`FTS_GPIO6_UNUSED`); read system info; custom commands; reset again; `A0 00 FF`; then the event stream.

Touch events (8 bytes). The identifier is the **high nibble of byte 0 alone** (`eventId = evt_data[0] >> 4`): `1` enter, `2` motion, `3` leave, `4` status, `0` controller ready, `F` error.
The low nibble is payload, not part of the identifier — it carries the top two bits of the major axis — so comparing the whole byte drops every contact wide enough to set them (108 of 704 in the first run that reported anything).
Because nibble `0` is both controller-ready and an empty queue (`00`), nothing-pending has to be ruled out before identifiers are matched.

`id = e[1]>>4`, `type = e[1]&F`, `x = (e[3]&F)<<8 | e[2]`, `y = e[4]<<4 | e[3]>>4`, `z = e[5]`,
`major = (((e[0]&0x0C)<<2) | (e[6]>>4)) * 16`, `minor = (((e[7]&0xC0)>>2) | (e[6]&0x0F)) * 16` (`AREA_SCALE` is 16; byte 7's top two bits are the minor axis, its low five the queue depth).

## Hardware (cold)

| What | Where |
|---|---|
| SPI controller | G#111D_0000, s3c64xx-family, 64-byte FIFO, CPU mode (no `dma-mode` in the tree), swap-mode for word transfers |
| USI mode | `sysreg_hsi0` G#1102_101C [2:0] = 2 (SPI); controller USI_CON G#C4 = 0 releases the USI reset |
| Clock | CMU_HSI0 G#1100_0000: mux G#101C bit0 (1 = OSCCLK), div G#181C, gate G#2120, IP gates G#20FC/G#2100/G#20BC, QCH G#30CC. The controller divides by 4 internally: oscillator /1 gives ~6.1 MHz (max 10 MHz) |
| Pins | not in Android's pinctrl (`pinctrl-0 = <>`): fixed-function pads |
| Reset | gpp21-2 = `pinctrl@10C40000` bank G#40, output, high = run |
| IRQ | gpn0-0 = `pinctrl@15060000` bank 0, active low (Android leaves it CON=G#F, EINT) |
| DVDD 1.8 V | S2MPG14 LDO25M, ctrl G#43 (type PMIC), bit 7 enable, [5:0] = (uV-700000)/25000 = G#2C |
| AVDD 3.3 V | S2MPG14 LDO4M, ctrl G#2E, bit 7 enable, [5:0] = (uV-1800000)/25000 = G#3C |

Power-up (`fts_chip_powercycle`): reset low, DVDD on, 1 ms, AVDD on, 5 ms, 10 ms, reset high.

## Cold bring-up in ferros (2026-10-19): WORKS

`ferros_kernel/src/touch.rs`, driven over PT:

```bash
ferros-bridge diag "touch state"   # clocks, pins, rails, read only
ferros-bridge diag "touch up"      # SPI up, rails, reset, controller ready, scan mode on
ferros-bridge diag "touch watch"   # 8 s of touch events as text
ferros-bridge diag "touch paint"   # 16 s: draw finger strokes on the panel (by-eye test)
ferros-bridge diag "touch off"
```

First cold run, slot b, no Linux anywhere:

```
SPI up: USI_CON G#0 CH_CFG G#0 MODE G#1FF80000 STATUS G#0
DVDD (LDO25M): G#AC
AVDD (LDO4M): G#BC
controller ready after A#12 ms: G#0300000000000000
GPIO6 register G#80 -> G#80
touch system info: largest coordinates A#2991 and A#1343
chip ID G#3640 (FTM5 parts: G#3648 ALIX, G#3640 SALIXP, G#3639), fw G#0083
scan mode active: G#4305010000000000
```

Then `touch watch` reports contacts: measured 1344 events in 8192 ms, 1267 of them motion, coordinates tracking the finger.
It also reports how often the controller's interrupt line (gpn0-0) was asserted, which is what tells an empty window apart from a broken read — the line is asserted only when the chip has something queued, so "0 events, line asserted 2 of 513 polls" means nobody was touching, not that the driver is broken.
Cold state before anything is written: the USI2 gates are in automatic mode (open), USI_CON = 1 (USI held in reset), both rails programmed to the right voltage but disabled (G#2C, G#3C), reset pin an input.
The firmware in the controller's flash runs as is; no firmware download is needed.

Finger-verified 2026-10-20: the axes are x across the short side (0-1343) and y the long side (0-2991), matching the system-info block.
`diag touch decay` is the instrument that proved it — a 1:1 decay screen in 10-bit-per-channel colour (DPP base layer ARGB2101010, format 19) where every frame each channel of every pixel loses one and each reading stamps the contact's ellipse split across the channels: major axis in red, minor in blue, pressure in green, so a fading trail's red fringe is the eccentricity of the contact.
Measured 2035 readings in 30 s over 586 frames, 29 ms a decay pass, pressure to A#123 and a largest contact of A#352 by A#224.
The canvas lives in the cacheable DISPLAY_HEAP and is blitted write-only; a read-modify-write straight on the uncached framebuffer costs 1079 ms a frame against 29 ms this way.
The system-info block also holds a per-die identifier at offsets 32-47; ferros reads only the header and the coordinates.

## The sensor grid and the raw capacitance frames (2026-10-20)

`diag touch info` reads the chip's system-information block and reports the grid and the addresses of its live frames. Measured on husky:

```
grid: A#18 transmit x A#39 receive = A#702 nodes; largest coordinates A#2991 and A#1343
mutual-sense frames: raw G#BACC filtered G#7924 strength G#86E4 baseline G#9678
self-sense transmit: raw G#B948 strength G#1352; receive: raw G#B974 strength G#137E
one mutual-sense frame is A#1404 bytes
```

So 702 crossings over a 1344 x 2992 panel: about 75 pixels, near 4 mm, per node either way. A mutual-sense frame is one 16-bit value per crossing; a self-sense frame is one per row or column (18 and 39 values).

Each frame is read with the same framebuffer command as the system info (`A7`, 16-bit address, one dummy byte first), at the address the block gives, for `transmit x receive x 2` bytes. No command is needed to prepare them — they are live. Only the factory compensation data needs a load command first (`SYS_CMD_LOAD_DATA`, ftsCompensation.c).

Block offsets, from `readSysInfo` (ftsCore.c): header `A5 01` at 0, largest coordinates at 80 and 82, transmit and receive channel counts at 84 and 85, key and force lengths at 86 and 87, the four mutual-sense addresses from 136, the self-sense ones from 144. The block also holds a per-die identifier at 32-47 which ferros does not read.

### `diag touch map` — the capacitance frame, live (2026-10-20)

Reads the mutual-sense strength frame (raw minus the chip's own baseline) in a loop, paints it on the panel as nearest-neighbour blocks on ferros's own framebuffer, and prints the last frame as an ASCII map with the strongest nodes. First run, one finger on the glass:

```
strength frame at G#86E4, A#18 x A#39, A#1404 bytes
A#703 frames in A#12000 ms; values A#-498..A#1656; A#13 ms per paint
  ..........:**:....
  ..........+%%+....
  ..........*%@= ...
  ......... -*+. ...
strongest nodes: (tx A#12, rx A#12) = A#1562; (tx A#12, rx A#11) = A#1551; (tx A#11, rx A#12) = A#1491; ...
```

58 frames a second, of which 4 ms is the frame read over SPI and 13 ms the paint. A fingertip is a bump about four nodes across (around 15 mm at this pitch) peaking near 1560 against a quiet floor inside about plus or minus 200, so roughly eight times the noise. The screen mapping is transmit across the width and receive down the height; the first run came out the right way round.

### The frame layers this panel carries (2026-10-20)

The driver defines 40 frame addresses in the information block (ftsCore.c `readSysInfo`, offsets 136-215); a zero means the panel does not carry that layer. husky carries **16**, four measurement stages across four sensing modes:

| Mode | Values per frame | raw | filtered | strength | baseline |
|---|---|---|---|---|---|
| Mutual (the 18 x 39 grid) | 702, 1404 bytes | G#BACC | G#7924 | G#86E4 | G#9678 |
| Self, transmit lines | 18, 36 bytes | G#B948 | G#12D6 | G#1352 | G#AB18 |
| Self, receive lines | 39, 78 bytes | G#B974 | G#1302 | G#137E | G#AB44 |
| Detect (the low-power wake scan) | — | G#B858 | G#10EE | G#111A | G#1096 |

Zero, so absent on this panel: the key, force, hover and proximity groups (24 addresses). Key length and force length both read 0 in the block, which agrees.

`strength` is `raw` minus that mode's own `baseline`, and is the layer to look at; `filtered` is what the firmware's own detection runs on.
