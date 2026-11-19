---
name: akita-kepler-gnss
description: "akita's GNSS (Samsung KEPLER) is CHPP over spi_21 in PIO with two handshake GPIOs, BETP inside handle G#10; captured under the hv 2026-11-18; the firmware load needs a cold power-on capture"
metadata:
  type: project
---

Captured 2026-11-18 (Android 17 on slot a under an oracle hv, images `gnss` and `gnss-boot` of tools/akita-hv-camcap.sh, decoder tools/gnss-oracle; local in keys/akita/captures-2026-11-18/).
docs/GPS.md "akita's KEPLER" has the detail.

- **Bus:** spi_21 G#111E_0000 only, PIO, 32-bit words (wire order = each word's LE bytes), PACKET_CNT per transfer; handshake gpp4-3 (G#1084_0084 bit 3, AP to GNSS) and gpa5-3 (the chip's interrupt). No modem, no shared memory, no mailbox, no kernel power control.
- **Framing:** CHPP (68 43, 8-byte header, CRC-32), MTU 1280, window 1; discovery G#0F, timesync G#02, the service on handle G#10; Samsung's BETP rides in its notifications (AP cmd 2, chip cmd 3). Every packet of both captures checks its CRC.
- **Firmware:** /vendor/firmware/kepler.bin goes over the same SPI (BETP bootloader messages) but was NOT in any capture: the chip keeps it across warm resets. A cold power-on (the owner's hand) under `gnss-boot` would show it.
- **A fix:** 10 s after Maps asked, indoors. The session grants Maps location and takes it back to its original flags (nothing on the 8a asks for GPS otherwise).

**Why:** a ferros driver for akita's GPS needs exactly this; husky's BCM4776 path does not apply.
**How to apply:** never print or commit positions (the decoder hides payload bytes unless --hex); quiet spi_21's set-up words (GNSS_QUIET) or the cap fills before the fix. Related: [[akita_pixel8a]], [[gps_onchip_fix]].
