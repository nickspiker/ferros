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
