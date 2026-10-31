# NFC on husky

The controller is an ST54L (ST21NFC NCI 2.0 interface plus an eSE) on hsi2c_0 (PERIC1 USI0, G#10C8_0000), target G#08. It shares the bus with the vibration motor (haptics.rs).

## Split

- **Kernel** (`ferros_kernel/src/nfc.rs`, power switch `nfc`):
  - the bus's pin modes (gpp19-0/1, function 3) and the controller from cold, if nothing has brought it up;
  - the PMU clock pad: CLKOUT1 G#1546_3E84 bit 16 EXTPAD_CONTROL_EN, set through EL3's atomic alias G#1546_FE84;
  - the reset pulse on gph1-6. The modem process drives other pins in that bank (gph1-0/1/2/7), so the pulse stays in the kernel.
- **Process** (`payloads/nfc`, endpoint `nfc`):
  - NCI over the bus grant;
  - the interrupt line's level (gpa7-6, read in gpio-alive, polled every millisecond);
  - Photon's tag.
- **Dev tool:** `tools/husky-radio.sh nfc | call nfc <hex> | kill nfc`.

## Bring-up

ABL leaves the chip running and the bus up. The NCI sequence is Android's, captured on 2026-10-31 from the i2c tracepoints. The stack's half comes from the August bugreport's NFC snoop.

1. Reset pulse: low 20 ms, high 10 ms, low 20 ms, high, then 16 ms. The chip then holds a CORE_RESET_NTF.
2. `20 00 01 01` CORE_RESET, config kept.
3. `20 01 02 00 00` CORE_INIT.
4. `2F 02 02 02 01` PROP_NFC_MODE_SET. The chip resets into NFC mode and reports it with reason A0.
5. CORE_INIT again.
6. `20 02 0A 03 A1 01 1E A2 01 19 80 01 01` CORE_CONF_PROP.
7. RF set-up:
   - the discover map (ISO-DEP and NFC-DEP);
   - NFCC_CONFIG_CONTROL;
   - power sub-state "screen on";
   - routing, all to the host: the empty-AID default, the ISO-DEP protocol, technologies A and B;
   - listen parameters: SEL_INFO G#20 (ISO-DEP);
   - field-info notifications;
   - TOTAL_DURATION 500 ms.
8. `21 03` discover: listen as a card on A and B, and poll for tags on A, B and V. A polled tag's UID is logged, then the tag is released back to discovery.

Not touched:
- the eSE (NFCEE G#86);
- firmware and RF config. The chip reports firmware 02 06 17 50 and config 76 DA, which match the vendor image's st54l files, so nothing is flashed. A loader-mode reset notification would mean stop.

Reads are two I2C transactions: the 3-byte header, then the payload. Leading idle bytes (G#7E) in the header are dropped. Right after reset the chip NACKs its address for a moment; the driver retries.

## Photon's tag

Photon's tag is the joiner's half of tap-to-bind (photon `docs/pairing-v2.md`, `PhotonHceService.kt`). It answers under Photon's AID `F050484F544F4E01`:

| APDU | Answer |
|---|---|
| SELECT | 9000 (6A82 for any other AID: every ISO-DEP AID is routed here) |
| GET DATA `80 CA 00 00 00` | S (32) ‖ 9000 while a ceremony is live, else 6A82 |
| anything else | 6D00 |

The `nfc` endpoint:

| Op | Request | Reply |
|---|---|---|
| 0 STATE | — | text |
| 1 SERVE | S (32 bytes) | [0] |
| 2 STOP | — | [0] |
| 3 READS | — | activations, S served, field-on events (u32 LE each) |

## Status (2026-10-31)

- Up from cold on ferros: listening, polling.
- RF path verified: the owner's tag read as NFC-A Type 2, UID 04 6D F0 EA D2 5B 80 (SENS_RES 0044, SEL_RES 00), re-read each discovery cycle while held.
- Next:
  - a sponsor phone's read of S (the APDU path);
  - the interrupt line instead of polling;
  - wiring to Photon's join ceremony.
