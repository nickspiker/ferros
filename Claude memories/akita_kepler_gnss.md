---
name: akita-kepler-gnss
description: "akita's GNSS (the KEPLER core of its S5300 modem): CHPP over spi_21 in 32-bit PIO with two handshake GPIOs, BETP records inside handle G#10; decoded and a driver built 2026-11-19 (branch akita-gnss); the core is powered and loaded by the CP over the gnss_boot channel, dark under ferros until the modem process sends those commands"
metadata:
  type: project
---

Captured 2026-11-18 (Android 17 on slot a under an oracle hv, images `gnss` and `gnss-boot` of tools/akita-hv-camcap.sh; local in keys/akita/captures-2026-11-18/), decoded 2026-11-19 with `ferros_kepler` (host-tested) thru `tools/gnss-oracle --betp`; gnssd ships unstripped in /mnt/Harbor/ferros-ref/akita/location/gnssd/release/. docs/GPS.md "akita's KEPLER, decoded, and its driver" has the detail.

- **Bus:** spi_21 G#111E_0000, 32-bit words (SWAP_CFG G#DD, MODE_CFG G#5FFC_0000, SLAVE_SEL G#23, at most 16 words a transfer, the RX level counts bytes), 3.125 MHz max; gpp4-3 (G#1084_0084 bit 3) AP to GNSS, gpa5-3 the chip's ready and interrupt (`gps-host-req`): husky's MCU_REQ and HOST_REQ pins.
- **CHPP:** 68 43, 8-byte header, CRC-32; the AP's payload seq = the chip's latest ack, every chip payload acked bare (seq 0).
- **BETP:** a notification is a list of records (AP heads 20 bytes, chip heads 16). Fix = G#0143 (SiRF MID 67 SID 1, 160 B: lat/lon i32 1e-7 deg at +48/+52, errors cm at +80/+84); satellites = G#0003_710A (37 B head, 29 B a satellite, Android constellation and flag numbers). The AP sent only G#7304 (start), G#12DE, G#04D4 G#0F, G#0398 G#8E: no aiding.
- **The core is the CP's:** gnssd powers, resets and loads it over /dev/gnss_boot (SIT channel 240: G#0202 power, G#0203/G#0204 reset, IOCTL_LOAD_GNSS_IMAGE into GNSS_FW G#EAC0_0000 then G#0E05, G#0E09 start), never over SPI. The 2026-11-18 idea "a cold power-on capture of SPI shows the firmware load" was wrong.
- **Driver:** payloads/gnss_kepler (the gps endpoint, measures, tracks, gps.last as payloads/gps); the kernel's `gps` switch on the 8a is `kepler_power` (pins, spi_21 halved, no rails). On the 8a (b270): the ready line never rises: under ferros the modem process reboots the CP and nobody sends gnss_boot.

**Why:** a ferros fix on the 8a needs the modem process to boot the core first; the driver above it is done.
**How to apply:** next is `tools/akita-gnss-trace.sh run` from Android (root, no hands, no flashing) for gnssd's exact gnss_boot bytes, then channel 240 in payloads/modem. Never print or commit positions (the oracle masks the fix). On the 8a the vault's `gps` capsule is still the Pro's driver (it exits at once: no replay). Related: [[akita_pixel8a]], [[gps_onchip_fix]].
