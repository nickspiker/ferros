# NFC on husky

The controller is an ST54L (ST21NFC NCI 2.0 interface plus an eSE) on hsi2c_0 (PERIC1 USI0, G#10C8_0000), target G#08. It shares the bus with the vibration motor, whose driver is this process too (below).

## Split

- **Kernel** (`ferros_kernel/src/nfc.rs`, power switch `nfc`):
  - the bus's pin modes (gpp19-0/1, function 3) and the controller from cold, if nothing has brought it up;
  - the PMU clock pad: CLKOUT1 G#1546_3E84 bit 16 EXTPAD_CONTROL_EN, set through EL3's atomic alias G#1546_FE84;
  - the reset pulse on gph1-6. The modem process drives other pins in that bank (gph1-0/1/2/7), so the pulse stays in the kernel;
  - the motor's reset, gph2-3, in the same block (below).
- **Process** (`payloads/nfc`, endpoint `nfc`):
  - NCI over the bus grant;
  - the interrupt line (gpa7-6 in gpio-alive): with the kernel's `nfc-irq` line (SPI 42, granted with hsi2c0, 2026-11-13) a level-high external interrupt the serving loop waits on, with a call, the screen preference changing, and discovery's retry while stopped; without it, its level read every millisecond as before (~950 wakes a second on the 8a with the screen on);
  - Photon's tag;
  - the vibration motor (ops 7 and 8).
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
8. `21 03` discover: listen as a card on A and B, and poll for tags on A, B and V. A polled tag is logged (its kind and its UID's length, never the UID), an unlock tag's UID goes to whoever holds op 9, and the tag is released back to discovery.

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
| 4 READER, 5 READER STOP, 6 READ SECRET | (payloads/nfc's header) | |
| 7 MOTOR | | the motor's DEVID, REVID (u32 LE each, 0 for a failed read); its state on the console |
| 8 BUZZ | n (one byte, at most G#26) | [0] played, [1] refused or a write failed |
| 9 TAG | — | held until discovery reads an unlock tag, then its UID (7 or 10 bytes); nothing when let go |
| 10 TAG STOP | — | [0]; every held TAG answered with nothing |
| 11 PROBE | — | [0] at once; then the chip made sure of ("A chip that stops answering" below) |

## Unlock tags (attest with a tag, 2026-11-11)

AUTH.md "Attest with a tag" has the whole of it; this is the driver's part.

- **Nothing new is switched on.** Discovery (`21 03`) already polls for tags on A, B and V between its listens whenever the screen is on, attest screen or not, and stops when the screen goes off (`Nfc::screen`). So NFC is on at the attest screen exactly while the screen is, and a tag held to a dark device is never read. Photon's tag (card emulation) and the sponsor's reader (op 4) run in the same discovery, unchanged.
- **Which tags:** an RF_INTF_ACTIVATED_NTF from a poll on NFC-A (mode 0) with protocol T2T (G#02) and a 7- or 10-byte UID. A 4-byte UID may be random at every read (ISO/IEC 14443-3), so none is offered.
- **The doorbell:** op 9 is held (at most four callers, the oldest giving way) and answered with the UID when such a tag is read, so nobody asks the driver on a timer. The attest screen holds it while its entry is up; the settings NFC page holds it while it adds a tag. Op 10 lets every held call go: the page going, an add cancelled, the attest screen closing. A tag held to the device is read again each discovery cycle (TOTAL_DURATION, 500 ms), so a caller still waiting hears it again then.
- **Not gated on a person present,** unlike ops 1 and 4: the attest screen needs it while locked, the owner's exception to "no NFC action while locked" (AUTH.md "Radios while locked"). While locked the attest screen is the endpoint's only holder; it hands the UID to the kernel and nowhere else.
- **The UID is never logged:** a tag's line says its mode, protocol, interface, the UID's length and SENS_RES.
- **For as long as the screen is up** (owner, 2026-11-17: "why's it time out? Like if I don't tap within the first min, I have to type my handle"): nothing in the driver or the attest screen gives up on a timer, so a tag that reads nothing is a chip that stopped, handled below.

## A chip that stops answering (2026-11-17)

Measured on the Pro:
- **After a RAM reload** (b918, at the attest screen) the chip answered CORE_RESET with its notification A#590 ms after the command, past the 512 ms a command waited, so the bring-up failed; every retry sent CORE_RESET again, and the chip never came up (CORE_INIT unanswered), retried every 64 s for good.
- **After A#42 minutes dark** (2026-11-14) it answered neither the screen-on power sub-state nor RF_DISCOVER, and discovery was retried for good without the chip being reset.
- **After 11 hours at the attest screen** (b913, the night of 2026-11-16) the owner's tag read nothing (no activation, no line), while the driver believed discovery ran; its loop had made 70 passes in those hours, and the one just before the owner typed said nothing (ST's own notifications were dropped unseen then).

Still open (b919, the fixed driver after a RAM reload): every reset pulse gives the chip's boot notification (reason 1) and two ST notifications (`6F 02 14 31 20 00 07 07 00 23 F7 …`, `6F 02 16 31 20 00 82 06 01 …`), then CORE_RESET goes unanswered for the whole 2 s, so on a reload the chip is never up and no tag is read. Not the reference clock's pad: CLKOUT1 (G#1546_3E84) reads G#0001_0000 after the reload, and bit 0 is a disable (Linux's cal-if `ra_enable_clkout` clears it to enable, sets it to disable), so that is the output on, TCXO selected, gated by the chip's clock request; a poke of bit 0 on 2026-11-17 was undone 32 s later, with no retry in between. Next: the same reading and the two ST notifications on a cold boot, where the chip answers CORE_RESET in about 10 ms.

**Cold boot against reload, the notifications (2026-11-18, from console captures of 2026-11-17, their NFC and modem lines kept in keys/husky/captures/nfc-cold-vs-reload-2026-11-17.txt; no device run):**

| | cold boot (2026-11-17 evening, t25 s) | reload (b930, t2303 s, three pulses) |
|---|---|---|
| after the pulse | reset notification, reason 01 | reset notification, reason 01 |
| ST's first | `6F 02 14 31 20 00 07 07 00 23 F7 00 00 00 F5 83` | the same (its byte before 83 F4 to F6) |
| next | reset notification, reason 02: CORE_RESET answered, 3 ms after the first | `6F 02 0B 31 20 00 81 06 01 00 00 00 ..` (twice) or `6F 02 16 31 20 00 82 06 01 00 00 00 5C 97 82 09 ..` (once), then nothing to CORE_RESET for 2 s |
| the modem's CP at the pulse | off (the modem process read `PS_HOLD 0 ... PHONE_ACTIVE 0` before powering it, at t28 s, after NFC was up at t25.5 s) | on, left running by the kernel before the reload (`PS_HOLD 1 CP_WRST_N 1 PHONE_ACTIVE 1`, boot_stage G#3FFF; the reload's crowbar takes only its PCIe link down), until the modem process cycled it at t2308.4 s, after all three pulses |

A later pulse in the same reload session (t2717.9 s, the CP up again under ferros's own modem process) gave the same 81 notification; the capture ends there.
The 81 and 82 notifications are what a cold boot never sent before CORE_RESET was answered, and every pulse that sent one had the CP powered.
Not yet proven: whether the CP (and with it the SIM, which the ST54L reaches over SWP: host G#81 is the UICC in libnfc-hal-st.conf) is what holds the chip, or something else a reload leaves.
What would decide it (each needs a device run): on a cold session with the modem up, kill and respawn `nfc` (a pulse with the CP live, which no cold boot has done yet): a reason-02 answer clears the CP, an 81 or 82 and silence points at it; and a reload with the CP off at the pulse.
ST notifications are said whole up to 64 bytes since 2026-11-18 (the 82 one was cut at 16).

What the driver does since:
- A command waits two seconds for its response, the NCI stack's command-complete timeout (libnfc-nci NFC_CMD_CMPL_TIMEOUT).
- A command left unanswered, a bring-up that failed, and a CORE_RESET notification outside the bring-up (the chip resetting by itself, which loses its discovery) all bring the chip up again from a reset pulse (the `nfc` power switch on again: the kernel's pulse, as at the start; `Nfc::restart`), never with NCI commands alone.
- The retry waits a second, doubling to 64 (`retry_wait`), and three doorbells retry at once: the screen coming on, a TAG held, a PROBE.
- **PROBE (op 11)** is the doorbell for the silent case: the attest screen rings it when a finger lands while its entry is empty (`TAG_PROBE`, its own thread, so the screen never waits). The driver answers first, then sends RF_DISCOVER: G#A0 (already started) is a chip alive and discovering; a start means discovery had stopped with no word from the chip, said and counted; no answer brings the chip up again from a reset pulse.
- ST's own notifications (G#6F) are counted, the last kept for STATE, and said (at most 16 in a 64 s report window).
- STATE says whether the chip is to be brought up again, the TAGs held, discovery failures, reset pulses, probes and how many found discovery stopped, the ST notifications, and how long ago the chip last sent anything.

## The vibration motor

The CS40L26A at G#43 (zuma-shusky-haptics.dtsi: reset gph2-3 active low, trigger gph2-0 and interrupt gpa6-3 unused) shares hsi2c_0, so its driver is this process's, the bus's one owner (KERNEL-OUT.md decision 8, 2026-11-10; the kernel's haptics.rs and `diag haptic` before). Registers are 32-bit, addressed by a 32-bit big-endian word: a read writes the address without STOP, then reads four bytes. It runs its ROM firmware out of reset; op 8 is the driver's `cirrus,fw-rom-only` play: "prevent hibernate" (G#0200_0003) to the mailbox G#13020, 2 ms, then G#0180_0000 + n, then 20 ms and the state again. The tuning (boost, peak current, f0, ReDC) is the RAM firmware's and is not applied.

The `nfc` switch takes the motor's reset low with the NFC chip's first pulse and high at its end: 20 ms low, then 46 ms for its ROM (it needs 10) inside the NFC chip's waits; off holds both chips in reset. Nothing else drives gph2 (the modem drives gph0 and gph1).

While NFC is switched off (its status circle) the process does not run and the motor has no driver. Nothing buzzes yet; when something does (a key's feedback), either the status circle's off becomes the radio's off inside this process (discovery stopped, the controller in reset, the bus still served) or the bus gets a server of its own.

```
tools/husky-radio.sh call nfc 07       # DEVID G#40A260 (A) or G#40A26B (B), REVID; the state line on the console
tools/husky-radio.sh call nfc 0801     # ROM waveform 1
```

## Status (2026-10-31)

- Up from cold on ferros: listening, polling.
- RF path verified: the owner's tag read as NFC-A Type 2, a 7-byte UID (not written down since 2026-11-11: with this device it opens the owner's identity, AUTH.md "Attest with a tag") (SENS_RES 0044, SEL_RES 00), re-read each discovery cycle while held.
- Next:
  - a sponsor device's read of S (the APDU path);
  - the interrupt line instead of polling;
  - an end-to-end ceremony (wired: Photon serves each binding request's S on the tag, docs/BT.md "Photon").
