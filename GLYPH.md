# Glyph — Hardware Specification

**Status:** Living draft. Whittle freely.
**Started:** August 2026 (year 3 of the plan)
**Target:** Year 9-12 (~2032-2035)
**Owner:** Nobody. Author: Nick.

---

## Design Principles

1. Hermetic. Zero ports, zero membranes, zero acoustic openings. The body is sealed, period.
2. One physical control: the kill switch. Everything else is the glass.
3. Kill means dead. Hardware power cut, keys zeroed, no firmware in the loop, no software veto.
4. Passive auth always. The device knows you the way your dog does. No unlock ceremonies. Ever.
5. Design for the tech of 2032-2035, not 2026. Bet on maturing curves, not shipping parts.
6. 12-year design life. Every component choice answers to that number.
7. Fail toward off. Every ambiguous failure mode collapses to dead, never to open.

---

## Cover / Display Stack

The cover is not protecting the display. The cover IS the display substrate.

### Stack, front to back

| # | Layer | Notes |
|---|-------|-------|
| 1 | Hard sputtered AR stack | Multi-layer ion-beam oxide (SiO2/TiO2 class). Watch-industry process, not eyeglass coating. Sapphire's n=1.77 makes AR mandatory, not cosmetic. |
| 2 | Single-crystal sapphire, 0.1mm | Structural. Grown boule, diamond-wire sliced, lapped/polished. Flexes at this thickness — that's a feature (diaphragm, shock compliance). Edge finish quality is the fracture story; spec edge grind/polish hard. |
| 3 | Black matrix + color filters + IR-pass filter | Patterned photolithographically on the sapphire backside, first process step, on flat virgin substrate. IR subpixel filter passes ~1µm, blocks visible: looks like black matrix to the eye. No polarizer anywhere (Eco²OLED-style CF-on-stack). Blacks come from off-state + BM absorption. |
| 4 | QD conversion layers | Inkjet/litho patterned per subpixel. Red QDs, green QDs, IR QDs (InAs family, RoHS-clean) at ~1µm. Blue passes thru unconverted on the blue subpixel. |
| 5 | Blue GaN microLED array | Monolithic, one epitaxy, every emitter identical. GaN's native substrate is sapphire — the cover and the growth substrate are the same material family. No mass transfer of three chip types. Reflective cathode + tuned microcavity biases emission forward. |
| 6 | QD photodiode sensing layer | Sparse grid, one detector per 10-20 pixels. InAs-family absorbers, sensitive at ~1µm where silicon is blind. Deposited, not grown. |
| 7 | Compliant OCA bond | Soft silicone/acrylic, 50-150µm. This is the CTE decoupler (sapphire ~6 ppm/K vs backing). Also the acoustic suspension. Do NOT insert-mold plastic onto sapphire — locked-in residual stress fails on thermal cycling. Bond at room temp. |
| 8 | Glass-filled LCP structural back | 30-40% fill, ~10-20 ppm/K, RF-transparent (antennas live behind it). Molded separately. Supports the thin panel against flex. |

### Display specs

- ~6" class, ~460ppi
- MicroLED, not OLED. Inorganic: no moisture death, no fragile TFE, decades of lifetime, fits the 12-year warranty and the sealed body. OLED was the 2026 answer; this ships in 2032+.
- Polarizer-free: ~2x optical efficiency vs conventional stacks
- Brightness headroom: 2000+ nits sustained realistic
- Fill factor note: microLED chips are 3-5µm on a ~55µm pitch. The 4th subpixel and the photodiodes fit in existing dead space. Pixel pitch does not grow. Ambient contrast improves (more absorbing area).
- Interlayer yellow tint acceptable unless losses exceed ~10% below 480nm; correct with brighter blue. CF tuning is the second knob.
- If the interlayer ever needs to carry circuits: low-CTE polyimide (BPDA-PDA class, 3-6 ppm/K) replaces the OCA at that spot. Compliance and CTE-matching are redundant solutions; pick one per interface, never pay for both.

### IR wavelength: 1µm. Locked.

- Skin stays reflective (skin reflectance is water reflectance; dies past ~1400nm, ceiling 1300nm)
- Water absorbs enough that the environment is its own stray-light baffle. Contact sensor, not comms link: everything beyond a few mm is noise, and water eats it round-trip.
- Silicon QE at 1000nm is under 10% and falling. The display's IR emissions, including the optical data channel, are invisible to essentially every commodity camera on earth. 1050nm shaves the last few percent if wanted.
- QDs emit it and QDs detect it. Same material family, both deposited. No epitaxy anywhere on the red/IR problem.

---

## Sensing (what the display does besides display)

- **Touch:** optical, IR reflection at 1µm. Works wet, gloved, fully submerged. No capacitance anywhere. (Capacitive dies underwater; this doesn't.)
- **Force/press:** via exciter-interferometer vibration signature and OCA compression. Wet-mode redundant touch path.
- **SpO2 / heart rate:** red + IR reflectance = pulse oximetry. Finger on glass, anywhere.
- **Cardiac identity:** PPG waveform morphology as a passive auth signal.
- **Optical data:** display as bidirectional transceiver. Provisioning, debug, device-to-device — face the screens together. This is the data port. Invisible to silicon sensors.
- **No fingerprint. At all.** Not needed (see Auth), saves the dense detector zones, collimation optics, $30-50 of display BOM, and one pilot-line integration risk.

---

## Audio

Zero acoustic openings. The device talks by shaking its face and listens by watching its own face shake.

### Output

- **2x electromagnetic panel exciters:** audible band, ~300Hz up. Panel is the cone. 0.1mm sapphire is a nearly ideal diaphragm (high stiffness-to-mass pushes bending modes up where DSP eats them).
- **Calls:** route to the exciter nearest the ear zone, driven as a LOCAL bender, not whole-chassis. (Whole-chassis leaks your call to the bus seat next to you — Mi Mix lesson. Huawei P30 Pro localized it; copy that.) Pressed to the face = bone conduction; occlusion effect adds 20dB+ of bass below 500Hz. Harder smish, better sound.
- **1x LRA:** haptics (Taptic-style rigid-body clicks), felt bass (DSP crossover routes <200Hz audio envelope to vibration, Sony Dynamic Vibration style), notification, ultrasonic data chirps.
- **DSP:** modal EQ (per-unit measured panel transfer function, inverted), crossover, calibration. One signal chain, four jobs.
- Works underwater: water couples to solids ~3500x better than air.

### Input

- **Interferometric optical mic.** VCSEL + photodiode inside the sealed volume, watching the sapphire cover. The structural window is the diaphragm. Picometer displacement sensitivity. No port, no membrane, nothing to clog or fail at year 8.
- Bonus path: phone against face, jaw conduction drives the cover directly. Voice isolation in wind/noise for free (same trick as earbud VPU sensors).
- Works underwater. Full call chain functions submerged, in rain, in the shower. No phone on earth prints that line.

### Per-unit acoustic calibration = acoustic PUF

Every panel's modal fingerprint is unique and physically unclonable. Factory calibration data doubles as device-bound entropy for TOKEN attestation. A PUF you can hear.

---

## Kill Switch

- **Optical.** Emitter + detector inside sealed volume, actuator blocks the light path thru the sapphire (excellent optical window, no dedicated penetration).
- **Fail-dark = dead.** Light present: run. Light absent: off. Every failure mode — blocked path, dead emitter, cut sensing power, cracked window, water in the cavity — collapses to off. The switch can never fail toward on.
- **No firmware in the loop.** Detector output gates the main power FET directly. If a processor can veto it, it's a request, not a kill switch.
- Kill = power cut + CSR key zeroing. 0ms.
- Optical sidesteps the magnetic-actuation landmine entirely (fridge magnets, car mounts, MRI, stranger-with-a-neodymium DoS thru your pocket). Also doesn't care that the LRA is a magnet on a spring.

---

## Body

- Hermetic. No ports, no membranes, no gaskets-around-connectors because no connectors.
- Dive-rated, not IP-rated. Actual number comes from pressure-testing the panel bond; nothing in the architecture caps it at splash-proof. TODO: pressure spec.
- Frame: TBD (titanium wants antenna windows; LCP back is RF-transparent, so antennas live there regardless).
- Thickness: ~9-10mm acceptable (battery-driven, see below). ~200g class.
- Pressure equalization for altitude/thermal: rigid-body design, verify hermetic ΔP tolerance. TODO.

---

## Power

- **Charging:** resonant wireless only. Slow-charge philosophy — sealed body has nowhere to vent coil losses, and slow charging is what the battery wants anyway.
- **Battery — THE open bet of the whole device:**
  - Portless + sealed + 12-year warranty makes the battery the contradiction in the sentence. Standard Li-ion: 500-1000 cycles, done in 2-3 years. Unacceptable.
  - Option A: LTO. 15,000+ cycles, genuinely outlives the warranty, ~half the energy density. Cost: the 9-10mm body. Honest, unfashionable, correct.
  - Option B: solid-state, the year-9-12 bet. If it lands at consumer scale on schedule, the contradiction dissolves.
  - Decision date: TBD, late as possible. Design the envelope for LTO; celebrate if solid-state rescues the millimeters.

---

## Data / Radio

- WiFi / BT / UWB. UWB or 60GHz contactless for high-bandwidth sync thru the case.
- Optical thru-display for provisioning, debug, recovery, device-to-device (see Sensing).
- Cellular via FGTW MVNO. No phone numbers, no SMS. Modem: certified module is the pragmatic path; baseband sovereignty (isolated/external modem posture) is an open architecture question. TODO.
- Ultrasonic acoustic channel via exciters as tertiary path (pairing, earbuds, chirp-to-Glyph).

---

## Compute

- Own RISC-V SoC (Chisel → tape-out, mature node, 12-22nm class is all it needs)
- PIPE security chip
- ferros (ring memory, no paging), VSF compositor, TOKEN, Photon
- Custom CSRs for key zeroing (closed until silicon ships, opens after Founders)

---

## Auth

**Passive, continuous, always.** No "look at me." No "touch this." Doesn't work like that.

- **Signals:** voice (interferometric mic, ambient, scored on-device, never leaves), gait (IMU), RF environment (the WiFi/cell/BLE signature of your life — unfakeable from another building), touch dynamics (cadence, swipe geometry, press force), cardiac (PPG morphology), location, network stats, acoustic PUF anchoring it all to this physical unit.
- **Fusion:** 6+ independent signals, confidence score, not binary. Any one drifts (sick voice, limp), the rest hold.
- **TOKEN speaks confidence tiers natively.** Low confidence: read-only works. High-value ops gate on higher score. Score climbs passively thru normal use. (Google built this in 2016 — Project Abacus — and killed it because third-party banking apps demanded binary yes/no. Ecosystem problem, not physics. We own the stack. Their blocker doesn't exist here.)
- **Theft:** continuous scoring beats binary unlock cold. Snatch = gait discontinuity + wrong hands + wrong RF = confidence collapse in seconds, no user action.
- **Travel / correlated drift:** device is itinerary-aware. Knows you're flying to Europe tomorrow; expected discontinuity doesn't trip it. Weight body-borne signals (cardiac, voice, touch) over environmental ones when environment legitimately changes.
- **Watch as continuity beacon:** the dumb watch on the wrist — skin contact + proximity + its own motion signature — bridges every environmental discontinuity. Watch and phone vouch for each other. Strongest passive signal in the system.
- **Anomaly = lock.** If shit seems weird, it locks. Period.

## Recovery

- Locked device: recovery is a call to wife / daughter / dad — social attestation per user-set preferences. No vendor, no account, no email. FGTW enrollment path, already proven.
- Post-kill (keys zeroed): resurrection via fleet re-enrollment + social attestation. This is the one ceremony in the system, and it's earned: the keys are GONE, that was the point.
- TODO: spec "time from kill to restored trust."

---

## Launch Tiers

| Tier | Units | Price | Timing |
|------|-------|-------|--------|
| Founders | 1,000 | $5,000 | Serial #0001-1000, 12-yr warranty, crypto only |
| Elite | 10,000 | $3,000 | +6 months |
| Pro | 100,000 | $1,500 | +12 months |
| Standard | 1M+ | $600 | +18-24 months |

Crypto only (BTC, SOL, XMR, TOKEN). Decentralized web only. No press, no review units.

---

## Economics (2032 dollars, rough, revisit yearly)

### NRE — display-dominated, ~$10-30M total

- Display stack pilot-line development: $2-10M (assumes QD-microLED ecosystem matures on curve; double it if not). The elephant. Every year, re-check this number first.
- SoC: shuttle protos $100-300k; production masks $1-5M
- Optical mic productization: $200k-1M
- Mechanical tooling: $200-500k
- Certs: $100k baseline; +few hundred k if cellular certs bite

**Founders math is honest math:** 1,000 × $5k = $5M gross. Founders does not pay for development and can't. It's a funding event and a proof; NRE amortizes across Pro/Standard volume.

### Per-unit BOM

| Component | 10-100k volume |
|-----------|---------------|
| Display/cover assembly (sapphire, QD-microLED, photodiodes, filters, bond) | $250-500 |
| RISC-V SoC (owned, mature node) | $15-40 |
| PIPE | $2-5 |
| Memory + storage (16GB/512GB class) | $30-60 |
| Battery (LTO or solid-state) | $20-50 |
| Radios + cellular module | $30-60 |
| Wireless charging | $5-10 |
| Exciters ×2, LRA, mic module, kill-switch optics | $15-30 |
| LCP back, frame, gaskets, assembly | $40-80 |
| PMIC, IMU, passives, misc | $20-40 |
| **Total** | **~$450-850** |

At 1M+: ~$250-450 as the display rides its curve. Display is 50-60% of BOM in every scenario. Every cost-down conversation for the next decade is a display conversation.

- $1,500 Pro: fat margin
- $600 Standard: works at real volume IF display lands near bottom of range

---

## Patent Candidates (float to Mouscardes)

1. **Optical mic on structural window:** device's structural cover as microphone diaphragm, sensed interferometrically from within a hermetic volume, no acoustic port. Laser vibrometry is old; this combination isn't. Pay for the search.
2. **Acoustic PUF attestation:** per-unit panel modal fingerprint as hardware-bound credential entropy. Weaker, real, fun to draft.
3. **RGBI QD-converted monolithic array with integrated large-area QD-photodiode sensing on structural sapphire.** Specific combination, not a component list.

NOT patentable, don't spend attorney hours: moving-mass haptic click (Taptic, 2015, patented to the ground), exciter panel audio (LG/Sony shipped), dual audio/haptic actuator use (Cirrus sells the chips), bass-to-vibration crossover (Sony Dynamic Vibration).

---

## Open Questions / Whittle List

- [ ] Display fab partner. "Deposit on customer-supplied sapphire" is a nonstandard ask; longest pole in the whole plan. Start conversations years early.
- [ ] QD lifetime under sustained blue flux at phone brightness
- [ ] QD photodiode dark current over temperature
- [ ] Optical crosstalk: IR emitters vs detectors sharing a transparent substrate
- [ ] Sapphire edge strengthening spec (fracture initiates at edges, not faces)
- [ ] Index-matching layer at sapphire-OCA interface (1.77 → 1.5 loss) — have coating vendor model both interfaces, not just front AR
- [ ] Pressure rating: test panel bond, print a real dive number
- [ ] Hermetic ΔP tolerance (altitude, thermal)
- [ ] Exciter placement vs panel nodal lines; localized-bender geometry for the ear zone
- [ ] LTO vs solid-state decision date
- [ ] Baseband sovereignty: certified module vs isolated modem architecture
- [ ] Frame material + antenna strategy
- [ ] "Time from kill to restored trust" spec
- [ ] 1000nm vs 1050nm final call (silicon-blindness margin vs QD efficiency)
- [ ] Ultrasonic channel: pairing protocol, data rate, range