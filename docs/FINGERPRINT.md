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

## Where the bus is
Every SPI controller in zuma-usi.dtsi ships `status = "disabled"`.
The normal-world board files re-enable exactly five: spi_20 (touch), spi_21 (GPS), spi_12 (eSE), spi_18 (UWB), spi_19 (audio).
None is enabled for the fingerprint.
The pin map points at **PERIC0 USI1** (`spi_1`, G#1088_0000): the 4-pin SPI group is gpp2-0/1/2 (CLK/MOSI/MISO) plus gpp2-3 (CS), right beside the sensor's reset pin gpp1-0.
That controller is the one disabled-and-never-re-enabled block that fits the sensor, the shape of a peripheral handed to the secure world.
Its register window is a shared USI: hsi2c_1, spi_1, and serial_1 all live at G#1088_0000, one USI in different personalities.
The personality select (UART/SPI/I2C) is a 3-bit SW_CONF field in sysreg_peric0 (G#1082_1000) at the USI's usi-offset (USI1 = +0): bit0 UART, bit1 SPI, bit2 I2C (spi-s3c64xx.c USI_SW_CONF_MASK).

## The decider: NS-reachable or secure-locked
ferros already drives this exact s3c64xx IP on the HSI0 bus (spi.rs: touch on USI2, the amps on USI1, GNSS on USI3).
So if PERIC0 USI1 reads back from NS-EL2, driving it later is just a new `Port` with the PERIC0 clock path.
If it faults, the bootloader locked it to the secure world (TZPC/TZASC) and no amount of normal-world code reaches it.

`diag fp` (fp.rs) answers this, read-only:
0. It reads the PERIC0 USI personality map from sysreg_peric0 (the syscon the normal world owns, always clocked) so we see whether USI1 is even in SPI mode.
1. It reads a known normal-world, clocked PERIC0 block (USI0 UART, the console) as a control, so a fault on USI1 cannot be blamed on a broken probe.
2. It fault-safely probes the USI1 SPI block (USI_CON, CH_CFG, MODE_CFG, SLAVE_SEL, STATUS).

On the handoff build the synchronous-abort fault sink is terminal by design (main.rs `.Lexc_recover` warm-resets to preserve the report).
fp.rs disarms that sink for the window of each probe and restores it after, so a secure-locked read skip-and-counts and `probe32` reports it as a fault instead of resetting the phone.

### Running it
Boot ferros via the handoff (tools/husky-hotload.sh), then:
```
cargo run -p ferros-bridge -- diag fp
```

### Reading the verdict
- **NORMAL-WORLD REACHABLE**: ferros can drive the sensor itself once Trusty is gone, and a stage-2 MMIO trap could watch a normal-world owner to learn the protocol.
- **NOT reachable (secure-locked or clock-gated)**: only a logic analyser on the flex can see the bus, and ferros-native driving would first need the secure assignment cleared.
- **inconclusive**: the control read faulted, so the probe path is suspect, not USI1.

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
