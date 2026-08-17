# Glyph — Hardware Specification

**Status:** Living draft, rev 2 (thick-slab revision). Whittle freely.
**Started:** August 2026 (year 3 of the plan)
**Target:** Year 9-12 (~2032-2035)
**Owner:** Nobody. Author: Nick.

---

## Design Principles

1. Hermetic. Zero ports, zero membranes, zero acoustic openings. The body is sealed, period.
2. Nothing is replaceable. No serviceable parts, no wear items, no doors. Built once, sealed once.
3. One physical control: the kill switch. Everything else is the glass.
4. Kill means dead. Hardware power cut, keys zeroed, no firmware in the loop, no software veto.
5. Passive auth always. The device knows you the way your dog does. No unlock ceremonies. Ever.
6. Design for the tech of 2032-2035, not 2026. Bet on maturing curves, not shipping parts.
7. 12-year design life. Every component choice answers to that number.
8. Fail toward off. Every ambiguous failure mode collapses to dead, never to open.
9. Rigid, heavy, thick is fine. This is a brick with a soul, not a fashion item.

---

## Cover / Display

The cover is not protecting the display. The cover IS the display substrate.

### Sapphire slab: 1mm (2mm optional overkill tier decision, default 1mm)

- Single-crystal, c-plane. GaN epi demands c-plane; c-plane is also sapphire's hardest, most fracture-resistant cut. The display constraint picks the right crystal face for drops. Free.
- **1mm = native epi wafer thickness (0.65-1.3mm standard).** The cover is the as-grown substrate. NO post-fab thinning step, ever. Deletes the scariest, lowest-yield process step in the whole display flow.
- Drop margin: bending stress scales 1/t². 1mm = 100x margin vs a 0.1mm film. 2mm = 400x for +42g and 2x crystal cost — spinal-tap option, not the plan.
- Weight cost: ~42g at 1mm for a 6" cover (3.98 g/cm³). Accepted per principle 9.
- Both faces epi-grade polished. Back face polish is STRUCTURAL, not cosmetic — fracture starts at flaws on the surface in tension, and under point impact that's the back face.
- Optional strengthening: ion implantation for a compressive surface skin (Apple/GTAT-era research, real bend-strength gains). AR stack deposited compressive rather than neutral as the cheap cousin.
- Corner radius: larger than the case radius (see Case) — 8-9mm class.
- Optics: normal-incidence birefringence is zero on c-plane. Display under 1-2mm sapphire is watch-industry daily bread.

### Stack, front to back

| # | Layer | Notes |
|---|-------|-------|
| 1 | Hard sputtered AR stack | Multi-layer ion-beam oxide (SiO2/TiO2 class). Watch-industry process. Sapphire's n=1.77 makes AR mandatory, not cosmetic. Spec compressive film stress. |
| 2 | Sapphire slab, 1mm | See above. Structure, window, mic diaphragm, epi substrate. |
| 3 | Black matrix + color filters + IR-pass filter | Patterned photolithographically on the backside, first process step, flat virgin substrate. IR subpixel filter passes ~1µm, blocks visible: looks like black matrix to the eye. No polarizer anywhere (CF-on-stack). Blacks = off-state + BM absorption. |
| 4 | QD conversion layers | Inkjet/litho per subpixel. Red QDs, green QDs, IR QDs (InAs family, RoHS-clean) at ~1µm. Blue passes unconverted on the blue subpixel. |
| 5 | Blue GaN microLED array | Monolithic, one epitaxy, every emitter identical, grown on the cover itself. No mass transfer. Reflective cathode + microcavity biases emission forward. |
| 6 | QD photodiode sensing layer | Sparse grid, one detector per 10-20 px. InAs-family absorbers at ~1µm where silicon is blind. Deposited, not grown. |
| 7 | Stiff, thin bond to case | With AlN behind (CTE 4.5 vs sapphire 6, ΔCTE ~1.5 ppm/K), the bond no longer needs to be compliant for thermal reasons. Stiff shallow support also spreads point loads instead of letting the panel dimple. Compliance lives deeper (foam core). |
| 8 | AlN case | See Case. |

### Display specs

- ~6" class, ~460ppi
- MicroLED, not OLED. Inorganic: no moisture death, no fragile TFE, decades of lifetime. OLED was the 2026 answer; this ships in 2032+.
- Polarizer-free: ~2x optical efficiency vs conventional stacks
- Brightness headroom: 2000+ nits sustained realistic
- Fill factor: microLED chips are 3-5µm on ~55µm pitch. 4th subpixel + photodiodes fit in dead space. Pitch does not grow. Ambient contrast improves (more absorbing area).
- Interlayer yellow tint acceptable unless losses exceed ~10% below 480nm; correct with brighter blue. CF tuning is the second knob.

### IR wavelength: 1µm. Locked.

- Skin stays reflective (skin reflectance is water reflectance; dies past ~1400nm, ceiling 1300nm)
- Water absorbs enough that the environment is its own stray-light baffle. Contact sensor, not comms link.
- Silicon QE at 1000nm is under 10% and falling: display IR emissions, including the optical data channel, are invisible to commodity cameras. 1050nm shaves the last few percent if wanted.
- QDs emit it and QDs detect it. Same material family, both deposited. No epitaxy anywhere on the red/IR problem.

---

## Sensing

- **Touch:** optical, IR reflection at 1µm. Works wet, gloved, fully submerged. No capacitance anywhere.
- **Force/press:** vibration signature + bond-layer strain via interferometer. Wet-mode redundant touch path.
- **SpO2 / heart rate:** red + IR reflectance = pulse oximetry, finger anywhere on glass.
- **Cardiac identity:** PPG waveform morphology as a passive auth signal.
- **Optical data:** display as bidirectional transceiver. Provisioning, debug, device-to-device. This is the data port. Invisible to silicon sensors.
- **No fingerprint. At all.** Not needed (see Auth). Saves dense detector zones, collimation optics, and one pilot-line integration risk.

---

## Audio — rigid-body architecture

Zero acoustic openings, zero bending panels. Thick slab killed panel-flex audio (stiffness scales t³; 1mm sapphire is ~14x stiffer than the glass Huawei bent, 2mm is ~115x). All audio is contact conduction, rigid motion, or high-frequency piezo. Exciters are DELETED from the BOM.

### Output

- **1x LRA (or two-axis):** haptics (Taptic-class rigid-body clicks — improved by the rigid heavy body), felt bass (crossover routes <200Hz audio envelope to vibration), notification buzz, and the bone-conduction earpiece drive.
- **Earpiece = the whole device pressed to your face.** Rigid conduction thru tissue; occlusion effect adds 20dB+ below 500Hz. A stiff heavy slab conducts BETTER than a flexing membrane. Calls improved vs the thin-panel design. Works underwater, in rain, in the shower (water couples to solids ~3500x better than air).
- **1x thickness-mode piezo on the slab:** ultrasonic data channel + high-frequency alert content (rings, chirps, >1kHz). Displacement demand falls 1/f², so thick sapphire is fine up high.
- **Deleted: free-air speech and music.** Speakerphone and out-loud media are the casualty of the thick slab. Earbuds carry media. This was already the philosophy; now it's load-bearing.

### Input

- **Interferometric optical mic.** VCSEL + photodiode inside the sealed volume, watching the sapphire. The structural window is the diaphragm. Sensitivity drops with t³ stiffness but interferometry has the headroom — 2mm sapphire ≈ bending stiffness of 3-4mm window glass, and laser mics read those routinely (empirically verified by the author at age 15).
- Jaw conduction bonus path when pressed to face: voice isolation in wind/noise, same trick as earbud VPU sensors.

### Per-unit acoustic calibration = acoustic PUF

Every slab+case assembly's modal fingerprint is unique and physically unclonable. Factory calibration doubles as device-bound entropy for TOKEN attestation. A PUF you can hear.

---

## Case — AlN, foam-cored, rubber-rimmed

### Material: aluminum nitride

The only case material with metal-class heat spreading AND zero electromagnetic penalty:

- **Thermal ~170-200 W/mK dense** (vs LCP's 0.5). Sealed body has no vents; the case IS the heatsink. Full-surface spreading for SoC + wireless charging heat. (Sapphire itself adds ~35-40 W/mK front-face spreading.)
- **Electrically insulating, RF-quiet.** Standard RF package ceramic. Antennas work thru it (design for εr ~8.6). No eddy currents: wireless charging unimpeded.
- **CTE 4.5 vs sapphire 6.** Closest structural match available. Stiff thin bonds allowed.
- **Hydrolysis caveat:** AlN reacts with water over time. Dense sintered + conformal seal (ALD alumina or thin glaze) closes it. The surface treatment is PART OF the material spec, not an afterthought.

### Structure: graded density

- **Dense AlN skins** inside and out: hermetic surfaces, hard exterior, full thermal contact at SoC and coil mounts.
- **Porous AlN core, polymer-infiltrated:** CIM feedstock spiked with sacrificial pore formers (PMMA microbeads/starch, burn out with binder) → controlled-porosity core → vacuum-infiltrate with castable resin, cure in place. Urethane elastomer preferred (every pore becomes a micro shock mount); PMMA-via-MMA-monomer is the glassy alternative. NOT thermoplastic melt — nothing viscous wicks micron pores. Result: interpenetrating composite. Ceramic network carries stiffness + heat (30-80 W/mK core, still 60-160x LCP), polymer phase arrests every crack and eats drop energy. Precedent: dental PICN (VITA Enamic), metal-infiltrated ceramic foams, Ritchie's nacre-like Al2O3-PMMA (~30 MPa·m½, 10x base ceramic, aluminum-alloy class).
- Layered feedstocks co-mold and co-sinter as one part. Graded-density CIM is established practice.

### Manufacturing

- Ceramic injection molding: powder + thermoplastic binder → mold → debind → sinter (~1700-1800°C, N2, yttria aid), 15-20% linear shrink, ±0.3-0.5% as-sintered, diamond-grind critical fits. How every zirconia phone back was made.
- AlN CIM is the fussy end (anhydrous feedstock chain — the powder hydrolyzes). Fewer shops than zirconia; they exist because RF packaging demands them.
- Tooling turns on at Elite/Pro volume. Founders' 1,000 cases: machined from sintered blanks. No tooling bet before design freeze.

### Rubber — flush bezel, fat corners, vulcanized forever

- **Natural rubber**, not TPU. Strain-induced crystallization: self-reinforces at the point of incipient tear, and NR beats TPU on fatigue and tear resistance, which is bumper duty. A century of bridge-bearing service data says thick-section NR self-passivates (oxidized skin shields the bulk) and outlives the warranty with ~90 years of margin.
- **Compound:** carbon black (UV) + antiozonant + wax bloom. **Non-6PPD antiozonant chemistry mandatory** — 6PPD-quinone in road runoff is acutely lethal to coho salmon (active regulatory topic in WA and NZ). A sovereignty device shedding salmon poison is a bad footnote.
- **RF:** reinforcing-grade carbon black is past percolation = mmWave absorber (it's literally what anechoic foam is). Rule: NO RF thru the rubber above sub-6. All mmWave (60GHz docking/data) routes thru AlN or sapphire windows — both are excellent mmWave dielectrics (loss tangent ~1e-3 and ~1e-4). Thin rubber cross-section at 900MHz-2.4GHz is acceptable loss.
- **Geometry:** flush 1-2mm bezel surrounding the sapphire edge, zero protrusion. The bezel's job is glass-edge isolation (no hard object can ever touch the sapphire edge in any orientation), NOT energy absorption — 1-2mm of rubber bottoms out instantly against ~3J drops; energy absorption belongs to the foam core.
- **Corners:** case radius ~6mm, sapphire radius 8-9mm. Differential radii auto-widen the rubber annulus from 1-2mm on straights to 3-4mm exactly at corners, where drop statistics concentrate strikes. Zero protrusion, zero molded lumps, two numbers on two drawings. Generous sapphire radii also grind/polish to lower flaw density than tight ones.
- **Bond:** vulcanized on, not glued, not replaceable. Silane coupling agents (silanol end condenses with plasma-cleaned AlN surface hydroxyls, sulfur-functional end co-vulcanizes into the NR network — same chemistry as green-tire silica coupling). Insert-mold the finished case, compression/transfer-mold uncured NR, ~150°C cure forms rubber and bond in one shot. Spec 100% R-type (rubber-body) failure per ASTM D429; any interface failure is a rejected process.

### Drop strategy (division of labor)

1. Thick slab: 100x bending margin. The mid-face point-strike problem is solved by thickness.
2. Stiff shallow bond: spreads point loads into the stack instead of letting the panel dimple.
3. Rubber bezel: total glass-edge isolation.
4. Corner radii + widened corner rubber: spreads corner strikes into distributed loads.
5. Infiltrated foam core: crushes progressively, eats the drop energy.
6. Face-down flat drop: real ground is rough, first contact is a random asperity, and mid-face point load is the thick slab's best case. Flush is defensible. (0.15mm proudness available as invisible insurance; not currently specced.)

---

## Kill Switch

- **Optical.** Emitter + detector inside sealed volume, actuator blocks the light path thru the sapphire.
- **Fail-dark = dead.** Light present: run. Light absent: off. Every failure mode — blocked path, dead emitter, cut sensing power, cracked window, water in the cavity — collapses to off. The switch can never fail toward on.
- **No firmware in the loop.** Detector output gates the main power FET directly. If a processor can veto it, it's a request, not a kill switch.
- Kill = power cut + CSR key zeroing. 0ms.
- Optical sidesteps magnetic actuation entirely (fridge magnets, car mounts, MRI, stranger-with-a-neodymium DoS) and doesn't care that the LRA is a magnet on a spring.

---

## Body

- Hermetic. No ports, no membranes, no doors, no serviceable anything.
- Dive-rated, not IP-rated. Number comes from pressure-testing the slab bond; a 1mm slab moves this from "test the panel" to "test the seams." TODO: pressure spec.
- Full call chain functions submerged (contact-conduction earpiece + interferometric mic + optical touch). No phone on earth prints that line.
- Thickness ~9-11mm, weight ~230-260g class (thick slab + ceramic + LTO). A brick with a soul. Accepted.
- Pressure equalization for altitude/thermal: rigid body, verify hermetic ΔP tolerance. TODO.

---

## Power

- **Charging:** resonant wireless only. Slow-charge philosophy — sealed body vents nothing, and slow is what the battery wants. AlN case = no eddy loss, whole-back spreading of coil heat.
- **Battery — THE open bet:**
  - Portless + sealed + 12-year warranty makes the battery the contradiction in the sentence. Standard Li-ion: 500-1000 cycles. Unacceptable.
  - Option A: LTO. 15,000+ cycles, outlives the warranty, ~half the energy density. Cost: the thickness. Honest, unfashionable, correct.
  - Option B: solid-state, the year-9-12 bet. If it lands at consumer scale, the contradiction dissolves.
  - Decision date: TBD, late as possible. Design the envelope for LTO; celebrate if solid-state rescues the millimeters.

---

## Data / Radio

- WiFi / BT / UWB. UWB or 60GHz contactless for high-bandwidth sync — mmWave thru AlN or sapphire windows ONLY (never thru carbon-black rubber).
- Sub-6 antennas behind the AlN back (design for εr ~8.6; zirconia phones shipped at εr ~30, this is easier).
- Optical thru-display for provisioning, debug, recovery, device-to-device.
- Cellular via FGTW MVNO. No phone numbers, no SMS. Modem: certified module pragmatic; baseband sovereignty (isolated/external modem posture) open. TODO.
- Ultrasonic acoustic channel via the slab piezo (pairing, earbuds, chirp-to-Glyph).

---

## Compute

- Own RISC-V SoC (Chisel → tape-out, mature node, 12-22nm class)
- PIPE security chip
- ferros (ring memory, no paging), VSF compositor, TOKEN, Photon
- Custom CSRs for key zeroing (closed until silicon ships, opens after Founders)

---

## Auth

**Passive, continuous, always.** No "look at me." No "touch this." Doesn't work like that.

- **Signals:** voice (interferometric mic, ambient, scored on-device, never leaves), gait (IMU), RF environment (the WiFi/cell/BLE signature of your life), touch dynamics (cadence, swipe geometry, press force), cardiac (PPG morphology), location, network stats, acoustic PUF anchoring scores to this physical unit.
- **Fusion:** 6+ independent signals, confidence score, not binary. Any one drifts, the rest hold.
- **TOKEN speaks confidence tiers natively.** Low confidence: read-only works. High-value ops gate higher. Score climbs passively thru normal use. (Google's Project Abacus proved the tech and died on binary-API ecosystem demands. We own the stack. Their blocker doesn't exist here.)
- **Theft:** snatch = gait discontinuity + wrong hands + wrong RF = confidence collapse in seconds, no user action.
- **Travel / correlated drift:** device is itinerary-aware; expected discontinuity doesn't trip it. Weight body-borne signals (cardiac, voice, touch) over environmental ones when the environment legitimately changes.
- **Watch as continuity beacon:** the dumb watch on the wrist bridges every environmental discontinuity. Watch and phone vouch for each other. Strongest passive signal in the system.
- **Anomaly = lock.** If shit seems weird, it locks. Period.

## Recovery

- Locked device: call wife / daughter / dad — social attestation per user-set preferences. No vendor, no account, no email. FGTW enrollment path, already proven.
- Post-kill (keys zeroed): resurrection via fleet re-enrollment + social attestation. The one ceremony in the system, and it's earned: the keys are GONE, that was the point.
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

- Display stack pilot-line development: $2-10M assuming QD-microLED matures on curve; double if not. The elephant; re-check annually first. NOTE: 1mm slab REDUCES this risk — standard-thickness substrates, no thinning step, normal tool handling.
- SoC: shuttle protos $100-300k; production masks $1-5M
- Optical mic productization: $200k-1M
- Mechanical tooling (CIM molds, NR molds, grind fixtures): $300-700k
- Certs: $100k baseline; +few hundred k if cellular certs bite

**Founders math is honest math:** 1,000 × $5k = $5M gross. Founders does not pay for development and can't. It's a funding event and a proof; NRE amortizes across Pro/Standard.

### Per-unit BOM

| Component | 10-100k volume |
|-----------|---------------|
| Display/cover assembly (1mm sapphire slab, QD-microLED, photodiodes, filters) | $280-560 |
| AlN case (CIM graded, infiltrated, sealed) + NR overmold | $40-90 |
| RISC-V SoC (owned, mature node) | $15-40 |
| PIPE | $2-5 |
| Memory + storage (16GB/512GB class) | $30-60 |
| Battery (LTO or solid-state) | $20-50 |
| Radios + cellular module | $30-60 |
| Wireless charging | $5-10 |
| LRA, slab piezo, mic module, kill-switch optics | $10-25 |
| PMIC, IMU, passives, misc, assembly | $40-70 |
| **Total** | **~$470-970** |

At 1M+: ~$270-500. Display is 50-60% of BOM in every scenario; every cost-down conversation for a decade is a display conversation. Exciters deleted; slab cost up vs thin film; case up vs LCP, but it replaced a heatsink that no longer needs to exist.

- $1,500 Pro: healthy margin
- $600 Standard: works at real volume IF display lands near bottom of range

---

## Patent Candidates (float to Mouscardes)

1. **Optical mic on structural window:** device's structural cover as microphone diaphragm, sensed interferometrically from within a hermetic volume, no acoustic port. Pay for the search.
2. **Acoustic PUF attestation:** per-unit modal fingerprint as hardware-bound credential entropy.
3. **RGBI QD-converted monolithic array with integrated large-area QD-photodiode sensing on structural sapphire.** Specific combination, not a component list.

NOT patentable, don't spend attorney hours: moving-mass haptic click (Taptic, 2015), exciter panel audio (LG/Sony shipped), dual audio/haptic actuator (Cirrus sells the chips), bass-to-vibration crossover (Sony Dynamic Vibration), porous ceramics / infiltrated composites (dental PICN, decades of prior art), rubber-to-substrate vulcanization bonding (a century old).

---

## Open Questions / Whittle List

- [ ] Display fab partner. "Grow on customer's 1mm c-plane slabs" is nonstandard but FRIENDLIER than the old 0.1mm plan. Longest pole; start conversations years early.
- [ ] 1mm vs 2mm final call (default 1mm; 2mm is +42g for margin nothing else needs)
- [ ] QD lifetime under sustained blue flux at phone brightness
- [ ] QD photodiode dark current over temperature
- [ ] Optical crosstalk: IR emitters vs detectors sharing a transparent substrate
- [ ] Ion implantation strengthening: spec or skip
- [ ] Sapphire edge/corner grind + polish spec (fracture lives at edges; back-face polish is structural)
- [ ] AlN conformal seal selection (ALD alumina vs glaze) + dunk-life validation
- [ ] Infiltration resin: urethane vs PMMA-via-monomer; thermal vs damping trade
- [ ] Non-6PPD antiozonant system selection
- [ ] CIM-vs-machined crossover volume for the case
- [ ] Interferometric mic SNR budget on 1mm slab; VCSEL wavelength + cavity design
- [ ] Piezo coverage of the 1-3kHz alert band on thick slab (how much mid survives)
- [ ] Pressure rating: test slab bond + seams, print a real dive number
- [ ] Hermetic ΔP tolerance (altitude, thermal)
- [ ] LTO vs solid-state decision date
- [ ] Baseband sovereignty: certified module vs isolated modem architecture
- [ ] Antenna detail design behind AlN (εr ~8.6)
- [ ] "Time from kill to restored trust" spec
- [ ] 1000nm vs 1050nm final call (silicon-blindness margin vs QD efficiency)
- [ ] Ultrasonic channel: pairing protocol, data rate, range
- [ ] Weight budget: confirm ~230-260g target holds with LTO