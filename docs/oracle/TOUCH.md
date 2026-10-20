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
- `FB aa aa aa aa` then read 1+n: hardware register read (32-bit big-endian address). `FA aa aa aa aa dd..`: hardware register write.
- `A7 00 00` then read: framebuffer read (the system-info block). Android runs this one with RX DMA, so the oracle saw only the command.
- `A0 00 FF`: scan mode active, all sensing on. `C0 ..`: Google custom commands (grip suppression `C0 03 10 FF 03 ..`, `C0 0B 03`, `C0 0F 00`, `C0 0E 00`), each echoed as a `43 01 <cmd>` status event.

Probe order: poll `87` until `03 00 ..` (controller ready) after the reset pin rises; read hw reg G#2000_00DE and write it back with bit 7 set (`FTS_GPIO6_UNUSED`); read system info; custom commands; reset again; `A0 00 FF`; then the event stream.

Touch events (8 bytes): `13` enter, `23` motion, `33` leave. `id = e[1]>>4`, `type = e[1]&F`, `x = (e[3]&F)<<8 | e[2]`, `y = e[4]<<4 | e[3]>>4`, `z = e[5]`.

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
