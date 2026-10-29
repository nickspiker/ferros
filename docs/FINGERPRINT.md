# Fingerprint (Goodix under-display, husky)

## The question
Can ferros pull raw images off the Goodix under-display sensor?
Short answer today: no path exists that is reachable now, but the bus is physically there, so the real question is whether ferros at NS-EL2 can reach the controller or whether the bootloader locked it to the secure world.
`diag fp` settles that empirically.

## What the hardware exposes
The sensor node (`goodixfp`, compatible `goodix,fingerprint`, zuma-board-udfps.dtsi) sits under `&odm` (a `simple-bus`), not under any SPI controller.
It carries only an IRQ (gpa7-3), a reset (gpp1-0), a 3.3 V always-on rail (s_ldo15), and a lone `spi-max-frequency = 1 MHz` hint with no `reg` and no SPI parent.
The normal-world driver (`gf_spi.c` + `platform.c` + `netlink.c`) binds as a `platform_driver` and does zero SPI: no `spi_*`, no `readl`/`writel`, no `ioremap`, no image/frame/capture/DMA.
It allocates an input device, gates a clock, toggles reset/power, and opens a netlink socket (channel 25) to the Goodix HAL daemon, which talks to the Trusty TA.
So the capture and the match live entirely in the secure world; the normal world only hears "a finger happened" and forwards it.

## Where the bus is - and why it cannot be read off the tree
Every SPI controller in zuma-usi.dtsi ships `status = "disabled"`; the board files re-enable only the known devices (touch spi_20, GPS spi_21, eSE spi_12, UWB spi_18, amps spi_19), and the camera uses PERIC0 USI1-4 (spi_1/2/3/4, gpp2/4/6/8) as I2C.
The sensor exposes no SPI data pins at all: the only pins in zuma-board-udfps.dtsi are the reset (gpp1-0) and the IRQ (gpa7-3), and no `spiN_bus` pin group anywhere in zuma-pinctrl uses gpp0 or gpp1.
So the SPI data lines (CLK/MOSI/MISO/CS) are absent from the normal-world tree entirely, and which controller the secure world drives the sensor from cannot be read off it.

**A wrong turn worth recording (2026-10-29):** the first guess was PERIC0 USI1 (`spi_1`, G#1088_0000), because a `spi1_bus` pin group on gpp2 exists.
But gpp2 / USI1 is the **camera's sensor I2C bus** (camera.rs, the one that answers G#08EF), not the fingerprint's - a `spi1_bus` definition in pinctrl is only one possible muxing of gpp2, not evidence the fingerprint uses it.
A reachability probe of G#1088_0000 came back NS-reachable, but it was reading ferros's own camera controller.
Do not treat USI1/gpp2 as the fingerprint again.

The personality of each USI (UART/SPI/I2C) is a 3-bit SW_CONF field in its syscon (sysreg_peric0 G#1082_1000, sysreg_peric1 G#10C2_1000, sysreg_hsi0 G#1102_0000) at the USI's usi-offset: bit0 UART, bit1 SPI, bit2 I2C (spi-s3c64xx.c USI_SW_CONF_MASK).
The hunt is by elimination: the unclaimed controllers are spi_0, spi_5, spi_6, spi_9, spi_10, spi_11, spi_13, spi_15 and spi_22; whichever of those the secure world has switched to SPI (with Trusty live) is the fingerprint's.

## The hunt: which USI, by elimination
ferros already drives this exact s3c64xx IP on the HSI0 bus (spi.rs: touch on USI2, the amps on USI1, GNSS on USI3), so once the fingerprint's controller is known, driving it is just a new `Port` with that bus's clock path.
The problem is finding it: the sensor's SPI pins are not in the tree, so we identify the controller by elimination.

`diag fp` (fp.rs) scans every USI the normal world can name across PERIC0, PERIC1 and HSI0, read-only:
0. It reads a known normal-world block (USI0 UART, the console) as a control, so a fault in the scan cannot be blamed on a broken probe.
1. For each USI it reads the SW_CONF personality from the syscon (always clocked), and for any USI that is configured (SW_CONF non-zero, hence clocked) it also reads the SPI block's CH_CFG/MODE_CFG/STATUS.
2. It flags any controller in SPI mode that no known device owns (the camera's I2C USIs, the eSE, UWB, amps, touch and GNSS are labelled) - the odd one out is the fingerprint.

On the handoff build the synchronous-abort fault sink is terminal by design (main.rs `.Lexc_recover` warm-resets to preserve the report).
fp.rs disarms that sink for the window of each read and restores it after, so a secure-locked read skip-and-counts and `probe32` reports it as a fault instead of resetting the phone.
Idle (unconfigured) USIs' SPI blocks are not read, since a clock-gated block can hang.

### Running it
It only pays off under the handoff build (Trusty + Android live), where the secure world has configured the sensor's USI; a cold boot leaves every PERIC0 USI at reset.
Boot the handoff (tools/husky-hotload.sh), then:
```
cargo run -p ferros-bridge -- diag fp
```

### Reading the verdict
- **one unclaimed SPI controller**: that CANDIDATE is the fingerprint's bus; next is to drive it (a new spi.rs `Port`) and see if the Goodix chip answers.
- **more than one**: narrow by reset/IRQ proximity or a flex trace before driving one.
- **every USI reads reset/none**: cold boot, no secure world ran - re-run under the handoff build.
- **no unclaimed SPI controller visible**: the fingerprint's USI is idle or secure-hidden (SW_CONF reads 0 from NS-EL2); a logic analyser on the flex is then the only way to find the bus.
- **control faulted**: the probe path is suspect, the scan is unreliable.

## The three ways to learn the protocol (once the bus is in reach)
0. **EL2 MMIO trap** (software): mark the USI1 page trapping in stage-2; a normal-world owner's accesses fault into ferros, which logs register and value, emulates, continues. Only works if the access is normal-world, which the reachability probe is the precondition for.
1. **Logic analyser on the flex** (hardware): four wires, world-agnostic, captures the commands and the raw frames during a normal Android enrollment. The reliable way to reverse the Goodix command set; needs the back off and a person present.
2. **SMC/FF-A trace** (bonus): ferros already sits between the normal world and EL3; logging the call envelopes gives the HAL to TA control verbs (enroll/capture/match), not the bus bytes.

## Why this matters
A Goodix template sealed in Google's TEE is device-bound and useless off the phone, which is why SENSORS.md marks the sensor "never" under the biometrics-move-with-identity rule.
If ferros does its own capture on raw frames and its own matching, the enrollment binds to the ira and travels with the identity across the fleet.
That is the prize: not reading Google's sensor, but owning the whole biometric.

## The real wall
Even with the bus up, the sensor speaks a proprietary Goodix command set that lives only in the TA and the sensor firmware, with no datasheet.
Getting raw frames means reversing that protocol (logic analyser or careful register work), which is the long part of the project; the bus bring-up is the short part.
