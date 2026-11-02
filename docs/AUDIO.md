# Audio on husky

## Speaker amplifiers: control side cold (2026-10-20, WORKS)

Two Cirrus Logic CS35L41 smart amplifiers on `spi_19` (BLK_HSI0 USI1, G#111C_0000), from `zuma-shusky-audio.dtsi` and Google's driver (`private/google-modules/amplifiers/cs35l41` in the husky kernel tree).

| | Left (`cs35l41@0`) | Right (`cs35l41@1`, `cirrus,right-channel-amp`) |
|---|---|---|
| Chip select | the controller's own nSS, manual mode | GPIO gpn1-0 (`pinctrl@15060000` bank G#20), active low, controller in auto mode (`chip-select-mode = 2`) |
| Reset | gpp1-1 (PERIC0 bank G#20), active high | gph2-1 (HSI1 bank G#40), active high |
| VA | sub PMIC buck A (S2MPG15 G#29 ctrl / G#2A out; on at cold boot: ctrl G#F8) | same |
| VP | the battery directly | same |

SPI: 32-bit register address (bit 31 set for a read), 16 bits of padding, 32-bit value, all big-endian; mode 0, up to 25 MHz (ferros runs ~6.1 MHz from the oscillator, `spi.rs`). The controller only clocks with its select logic active: for the GPIO-selected right amplifier it must be in auto mode (`SLAVE_AUTO | NSC_CNT_2`); with the native select held inactive every transfer times out.
The HSI1 power-domain status (PMU G#1546_2B04) is a plain read; EL3's priv-reg read refuses it (-22), as it does for pd_ispfe.

`ferros-bridge diag "amp up"` from a cold ferros: reset 2 ms low / 2 ms high, IRQ1_STATUS4 (G#1_001C) OTP_BOOT_DONE already set, then

```
left: DEVID G#35A40 REVID G#B2 FABID G#69
right: DEVID G#35A40 REVID G#B2 FABID G#69
with the left held in reset: right DEVID G#35A40, left DEVID G#0
```

## Next: the audio path

The amplifiers take PCM over TDM0 from the always-on compute DSP (AoC, `dai_be_tdm0_rx/tx`), which runs its own firmware; the AP does not drive the serial port. Sound out of ferros needs either AoC brought up (its firmware and IPC) or the amplifiers' own sources (the CS35L41 DSP/tone paths) for a first beep. Microphones are on AoC as well.

## The path to a first sound (research from source, 2026-10-25 night; nothing here run on the device yet)

**Who drives the speakers' port:** the AoC. Both amplifiers sit on `dai_be_tdm0_rx/tx` (zuma-shusky-audio.dtsi:47-59), DSP_A format, the codec a clock consumer (gs101-audio.dtsi:678-708, aoc_alsa_card.c:1229-1233); the AoC maps `PORT_TDM_0_RX` to `SINK_SPEAKER` (aoc_alsa_hw.c:41) and Linux's back-end ops program nothing on the AP. The controller is a DesignWare I2S inside the AoC (aoc.bin's `TDMDesignWare`; its peripheral table puts TDM0 at AoC-local G#01C0_3000, AP G#18C0_3000, a G#58-byte DW_apb_i2s window — useful for watching IER/ITER/ISR0, not for driving it: the AoC's audio clock mux and pin muxing are undocumented).

**The AoC side, shortest:** the AoC firmware has a signal generator, so a first sound needs no PCM stream. On `audio_output_control` (8-byte header: type, counter, len u16, id u16, reply s16; a 4-byte reply is an error):
1. CFG_SIGGEN (id 225, 15 bytes: wf_type u8 @8, wf_period u16 @9, wf_amplitude u32 @11; never sent by Linux; the firmware's strings list DC, Square, Triangular, Sawtooth, so wf_type 0-3 is a guess).
2. EP_SETUP (id 206, 28 bytes; aoc-interface-zuma.h:1000-1017, filled at aoc_alsa_hw.c:1735-1798) with mode 5 = signal generator, on entry point 1; then EP_SETUP2 (id 298).
3. BIND2 (id 271: src entry point @8, dst 0 = speaker @9, bind 1 @10).
4. SOURCE (id 201: source @8, on @9 = 1).
PCM instead goes through the ring service `audio_playbackN` (S16_LE stereo 48 kHz; TDM0 runs 4 × 32-bit slots, BCLK 6.144 MHz).

**The amplifier side, no DSP firmware:** DAC_PCM1_SRC (G#4C00) defaults to ASPRX1, the bypass (cs35l41-tables.c:74). After reset: the B2 errata patch, the OTP trim unpack (SPI at 4 MHz or less; ferros runs ~6.1), the DSP held off, boost for 1 µH / 4 µF, ASP format G#4808 = G#2020_0000 with RX width 16, clocking from SCLK (REFCLK 6.144 MHz, PLL closed loop, G#2C04 = G#510), ASPRX1 on, AMP_EN, then GLOBAL_EN (cs35l41.c:2168-2296, 2594-3009, 3675-3686). **The amplifier's PLL needs the AoC's SCLK: nothing comes out until the AoC runs TDM0.** It has no internal source of its own, and VMON/IMON leave it only on the serial port's TX side, not over SPI.

**Verification without ears:** the replies; `diag aoc log` ("Speaker TDM Started with Clk…", the SigGen line); TDM0's registers moving; the amplifier's PLL_LOCK (G#1_0018 bit 1) and PUP_DONE (G#1_0010 bit 24) with no error bits; GET_EP_TOT_SAMPLES (id 231) counting. Strongest: an AoC microphone capture.

**Not done unattended on purpose:** without the DSP there is no speaker protection, so the first drive should be at low amplitude with the owner present. Unknowns: the generator's fields, which entry points take mode 5, whether the AoC wants more state first (DSP mode 277, SINK2 270, AP state), and whether the OTP unpack can be skipped.
