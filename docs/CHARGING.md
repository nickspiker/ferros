# CHARGING: the husky's battery charger, and how Android drives it

A desk study, 2026-11-24.
Nothing here is built in ferros: ferros has no charge control and the charger runs on the bootloader's settings.
Owner, 2026-11-24: "monitor Android if needed to get our power driver specifics, then we can talk about battery settings. max charge, limit charging current, scheduled charging and shit like that."
Battery policy is his to decide; this is the knowledge and the capture plan it rests on.

**Sources.** Google's bms modules, shallow clone of `kernel/google-modules/bms` at `android-gs-shusky-6.1-android15-qpr2` (2a3ea226, the branch kdev-shusky is on) in `/mnt/Harbor/ferros-ref/bms`; the newer `android-gs-shusky-6.1-android16` copy is in `/mnt/Harbor/husky-kernel/private/google-modules/bms` (same algorithm, small drifts).
Device tree: `/mnt/Harbor/ferros-ref/kdev-shusky/dts/` (`zuma-shusky-battery.dtsi`, `zuma-husky-battery.dtsi`, `zuma-shusky-charging.dtsi`, `zuma-husky-charging.dtsi`).
USB side: `soc/gs/drivers/usb/typec/tcpm/google/usb_psy.c` in the husky kernel tree.
File and line references below are to those trees.
UNCONFIRMED marks what the source does not settle; the MAX77759 datasheet is not public, so field encodings that Google's code never spells out come from Maxim's sibling parts (MAX77705, MAX77729) and are marked so.

## The parts

| part | where | what |
|---|---|---|
| MAX77759 charger | hsi2c_11, G#69 | switching buck charger, Android's `main-charger` power supply; inputs CHGIN (USB) and WCIN (wireless); IRQ on IF_PMIC_IRQ_L gpa8-0 (XEINT_22), shared with the MAX77759's PMIC/TCPC side at G#66 |
| MAX77759 fuel gauge | hsi2c_11, G#36 | PMIC.md; FG_INT_L gpa7-7 (XEINT_21) |
| PCA9468 direct charger | hsi2c_11, G#57 | 2:1 switched capacitor from a PPS adapter or the wireless receiver (`dc-mains`); IRQ gpa7-4 (PLC_INT_L); enabled thru S2MPG14 GPIO 1 (CHARGE_PUMP_EN) |
| TCPC and BC1.2 | MAX77759, G#66 | USB.md; picks the input current by source |
| pack EEPROM | hsi2c_9, G#50 (m24c08) | battery identity and history (`google,eeprom-pairing`) |
| wireless receiver | `zuma-shusky-wcharger.dtsi` | Qi into WCIN, or into the PCA9468 for wireless direct charging |

The cell: 4950 mAh (`google,chg-battery-capacity`, husky overrides shusky's 4087), charged to 4.48 V at most (`google,fv-max-uv`).
The 8a (akita) has the same MAX77759 charger and gauge, no direct charger, a 4404 mAh cell and five CV tiers (4.2, 4.25, 4.3, 4.35, 4.48 V).

## The Pro on 2026-11-24: why it sits at 88%

The Pro sat on the dev box's USB overnight at 88%.
Read-only reads on hsi2c_11 (`ferros-bridge display i2c 69 <reg>`, and `display power 1` without `battery`), A#11 reads in all, no writes; B0 and B1 were left alone because interrupt registers usually clear when read.

| reg | name | value | decoded |
|---|---|---|---|
| B3 | CHG_INT2_MASK | G#FF | every INT2 source masked (reset value; Android unmasks INSEL, STA_TO, STA_DONE) |
| B4 | CHG_INT_OK | G#C7 | AICL_OK, CHGIN_OK, INLIM_OK, THM2_OK, BYP_OK set; **CHG_OK 0, BAT_OK 0**; WCIN_OK 0 |
| B5 | CHG_DETAILS_00 | G#60 | CHGIN_DTLS 3 (USB input valid), WCIN_DTLS 0, TREG 0 (no thermal regulation) |
| B6 | CHG_DETAILS_01 | G#A6 | **CHG_DTLS 6 = timer fault** (`CHGR_DTLS_TIMER_FAULT_MODE`, max77759_charger.c:49); BAT_DTLS 2 (in Maxim's family encoding "taking longer than expected to charge", UNCONFIRMED); VDROOP2_OK 1 |
| B7 | CHG_DETAILS_02 | G#20 | CHGIN_STS 1, WCIN_STS 0, BYP_DTLS 0 |
| B8 | CHG_DETAILS_03 | G#42 | FSHIP_EXIT_DTLS 1, MD_DTLS 0, THM_DTLS 2 (normal band in the family encoding, UNCONFIRMED; the gauge reads 26 °C) |
| B9 | CHG_CNFG_00 | G#05 | MODE 5 = charger + buck on; WDTCLR 0, CP_EN 0 |
| BA | CHG_CNFG_01 | G#99 | reset value: **FCHGTIME 1 (fast-charge safety timer on; 3 h in the family encoding, UNCONFIRMED)**, RECYCLE_EN 1, CHG_RSTRT 1, LSEL 0, PQEN 1 |
| BB | CHG_CNFG_02 | G#1D | CHGCC 29 = 1.933 A fast-charge current (reset G#07 = 0.466 A: the bootloader raised it) |
| BC | CHG_CNFG_03 | G#C2 | TO_ITH 2 = 250 mA top-off threshold, TO_TIME 0 (shortest top-off timer), AUTO_FSHIP_MODE_EN 1, SYS_TRACK_DIS 1 (reset G#99: the bootloader set it) |
| BD | CHG_CNFG_04 | G#23 | CHG_CV_PRM 35 = **4.35 V** CV (reset G#14 = 4.20 V: the bootloader set it) |
| C1 | CHG_CNFG_08 | G#05 | reset value: JEITA_EN 1 (the charger's own hardware JEITA), FSW 1 |
| C2 | CHG_CNFG_09 | G#13 | reset value: CHGIN_ILIM 19 = **500 mA input limit**, NO_AUTOIBUS 0 (the chip may set ILIM itself) |
| C5 | CHG_CNFG_12 | G#6A | reset value: CHGINSEL 1, WCINSEL 1, VCHGIN_REG 1 (4.6 V input regulation), WCIN_REG 1, DISKIP 0; bit 7 (named CHG_EN in Google's map) 0, its meaning UNCONFIRMED |
| C6 | CHG_CNFG_13 | G#03 | reset value: USB_TEMP 3, THM2_HW_CTRL 0 |
| CA | CHG_CNFG_17 | G#40 | reset value: AICL 1 |
| CB | CHG_CNFG_18 | G#00 | **WDTEN 0 (charger watchdog off)**, MINVSYS 0, VDP1/VDP2_STP_BST 0 (reset G#C0: the bootloader cleared the two VDP bits) |
| gauge | | | SOC 88 %, 4126 mV, 26.15 °C, −7 mA (average −4 mA) |

**Why 88%:** the charger stopped on its fast-charge safety timer, not at a ceiling.
The bootloader leaves FCHGTIME at its reset value (timer on); Android's driver clears it at probe ("disable fast charge safety timer", max77759_charger.c:3254-3257) because google_battery owns termination; ferros never touches the charger, so the timer stayed armed.
On the dev box's USB the input is held to 500 mA at 5 V (2.5 W), and with ferros drawing 0.6 to 1.2 W the cell got roughly 0.3 to 0.45 A: far too slow to fill 4950 mAh before the timer ran out.
The timer fault latched (CHG_DTLS 6, CHG_OK and BAT_OK low): the buck still carries the system from USB (the −7 mA is the cell barely in use), but nothing charges the cell.
88% is simply where the timer expired; the CV of 4.35 V was never reached (4.126 V).

**Confirming with no write:** in the Maxim family a timer fault clears when the input goes away; unplugging and replugging the cable by hand, then reading B6 again, should show CHG_DTLS 1 (constant current) and a positive gauge current (UNCONFIRMED until read).
The same ceiling will come back on any slow source after the timer's length (3 h if the family encoding holds).
Note that `display power ... battery` and `diag power start` write MODE (0, then 5 again); whether that restarts the timer is UNCONFIRMED.

## Decided 2026-11-24: the safety timer off

Owner, 2026-11-24: "Turn the safety timer off, yes."
Only that: no other charging policy (ceiling, current, scheduling) until he decides those.

- **How:** CHG_CNFG_01 G#BA, FCHGTIME [2:0] to 0, a plain read-modify-write, as Android's driver does at probe (max77759_charger.c:3254-3257).
  Google's probe does not touch CHGPROT for it; the bootloader leaves CHG_CNFG_06 at G#0C (CHGPROT 3, the protected registers open), so the write takes as is, and ferros leaves CHGPROT as found too.
  Android's other protected writes (`max77759_external_reg_write`, the input select, the headroom) open CHGPROT and close it after; none of those is this one.
- **At every boot:** `hsi2c::charger::safety_timer_off` from `kernel_main`, after the flash log comes up; one try, read back, and a flash log line: "charger safety timer off: CHG_CNFG_01 G#99 -> G#98 (CHG_CNFG_06 G#0C)", or "NOT off" with the error or the read-back, and the boot goes on.
  It runs on the 8a too (the same MAX77759 charger).
- **On the live Pro (2026-11-24, RAM kernel, attested):** `display i2c 69 BA 98` read back G#98, CHG_CNFG_06 stayed G#0C.
  The fault stayed latched: CHG_DTLS 6 (B6 = G#A6), CHG_INT_OK G#C7, the cell at −5 to −6 mA, SOC 86 %, 4.109 V, several minutes after the write.
  Turning the timer off does not clear a fault already latched; a replug (or a MODE cycle) should, not yet tried.

## Decided 2026-11-24: the charge voltage cap

Owner, 2026-11-24, on the battery setting: a charge voltage cap only, set on Settings → battery; "for daily use, I'd set it to 4.2 v by default and allow the user to set it above".
Only the charger's CV: no SOC watching, no mode toggling, no "full charge" override.
The charger's own JEITA stays on, and the cap only ever lowers the limit; the full temperature tiers (Android's C-rate and CV table above) are a later step.

- **The range:** the charger's 10 mV steps from its 4.00 V floor up to the battery's own ceiling, the last step of Google's table for the cell: 4.48 V on husky (`google,chg-cv-limits` = 4.20, 4.30, 4.35, 4.48 V) and 4.48 V on the 8a too (akita: 4.20, 4.25, 4.30, 4.35, 4.48 V); never the chip's 4.50 V.
- **Where it lives:** a device setting (this battery's), never the identity's: the battery's control block, link 7 (ferros_sys::control `BATTERY`, slot `CHARGE_CV`, mV), kept in the device's settings document as `charge_cv` (SETTINGS.md) and written to the vault on every change, as every knob is (RADIO-CONTROLS.md).
  The preference block was full (48 of 48), so the cap is a control knob, not a preference.
- **Applied:** the kernel checks the cap (range and step, else refused) and the service loop writes CHG_CNFG_04 when the block's generation moves (service.rs `charge_poll`), reads it back and says so in the flash log ("charge cap A#4200 mV (kept), read back A#4200 mV").
  At every boot: 4.20 V right after the safety-timer write (main.rs), so the bootloader's 4.35 V never stands; then the kept cap on the service loop's first pass, once the settings are read.
  Nothing kept, or a kept value the battery does not take (a document stored from the bridge): 4.20 V, and the flash log says which.
- **The slider:** the battery page, under the gauge's facts: "charge to", the voltage with a rough share beside it, the slider, what it does, why 4.20 V is the default, and above 4.20 V the share of 4.20 V's charge cycles and why wear is not linear.
  The finger's place maps to the knob with no clamp and no kink (map charge.rs `of_finger`: softplus(k u) − softplus(k (u − 1)), over k = 2^7, C∞, within 2^-7 of linear along the track, and its ends still reach 4.00 and 4.48 V).
  The cap is set at the lift or the tap (one write to the vault), the value under the finger shown while it moves.
  Words in all sixteen languages (ferros_lang `ChargeCap`, `ChargeCapNote`, `ChargeCapWear`, `ChargeCapAt`, `ChargeCapDefault`, `ChargeCapCycles`).

**The numbers on the page, and where they come from:**
- **Wear:** Battery University, BU-808 "How to Prolong Lithium-based Batteries", Table 4 ("Discharge cycles and capacity as a function of charge voltage limit"): 4.30 V 150 to 250 cycles, 4.25 V 200 to 350, 4.20 V 300 to 500, 4.13 V 400 to 700, 4.06 V 600 to 1,000, 4.00 V 850 to 1,500, 3.92 V 1,200 to 2,000; "Every 0.10V drop below 4.20V/cell doubles the cycle but holds less capacity."
  Those cells were built for 4.20 V; this one is built for 4.48 V, so its own figures differ, and the page says so.
  The page's "about N of the charge cycles at 4.20 V" is that rule, 2^-(V − 4.20)/0.1: 4.30 V a half, 4.40 V a quarter, 4.48 V about a seventh.
- **The share beside the voltage:** an estimate for a 4.48 V lithium-cobalt cell, not a measurement of this one: 64 % at 4.00 V, 74 % at 4.10, 82 % at 4.20, 89 % at 4.30, 96 % at 4.40, 100 % at 4.48 (a smooth rising curve through those, map charge.rs `share_pct`).
  No cell-specific public curve was found.
  The Pro's own gauge cannot stand in yet: on 2026-11-24 it read 88 % at 4.126 V at rest and 100 % at 4.29 V while still taking 563 mA, so its "full" was learned under the bootloader's 4.35 V cap, not the cell's 4.48 V.
  To replace the curve: charge to each cap to the end, rest, read the gauge's VFOCV and RepCap against a full 4.48 V charge's.

**Measured on the Pro, 2026-11-24 (by register writes on the RAM kernel; the kernel part not yet loaded):**

| CV written | read back | the cell | CHG_DTLS / BAT_DTLS | current |
|---|---|---|---|---|
| 4.35 V (as found) | G#23 | 4.284 V | 1 (CC) / 3 OK | +425 to +486 mA, input at its 500 mA (AICL_OK 0) |
| 4.00 V | G#00 | 4.291 → 4.185 V | 2 (CV), then 8 (off) / 5 (over the CV) | +475 mA falling to −249 and −448 mA: the device runs from the cell |
| 4.20 V | G#14 | 4.209 → 4.219 V | 8 (off), then 4 (done) / 3 OK | +2, then −3 mA: held on the input |

- A cap below the cell's voltage stops charging and the device runs from the cell until the cell comes down to the cap; the input does not carry it meanwhile (CHG_OK and BAT_OK low).
  So setting 4.00 V on a full cell drains it to about 4.00 V first, at the device's own draw.
- At the cap, the charger rests off and the device runs on the input, the cell at about 0 mA.

## The charger registers a driver needs

Encodings from max77759_charger.c (Google's own conversions; the register map is `max77759_A1_regs.h`).

| what | register | encoding |
|---|---|---|
| mode | CHG_CNFG_00 G#B9 [3:0] | 0 all off, 4 buck only, **5 charger + buck**, 8/9/A boost and OTG, C to F buck-boost combinations (max77759_regs.h `max77759_charger_modes`) |
| watchdog clear | CHG_CNFG_00 [7:6] WDTCLR | write 1 to ping (`max77759_wd_tickle`, :2325) |
| fast-charge timer | CHG_CNFG_01 G#BA [2:0] FCHGTIME | 0 = off (Android); reset 1 |
| recharge threshold | CHG_CNFG_01 [5:4] CHG_RSTRT | not set by Android; reset 1 |
| fast-charge current | CHG_CNFG_02 G#BB [5:0] CHGCC | set: 0 = off (and mode drops to standby), ≤ 200 mA → 3, ≥ 4 A → G#3C, else 3 + (I − 200 mA) / 66.67 mA; read: 133 mA + (code − 2) × 66.67 mA (:1403-1480) |
| top-off current | CHG_CNFG_03 G#BC [2:0] TO_ITH | 150 mA + 50 mA × code, 150 to 500 mA (:1538) |
| top-off time | CHG_CNFG_03 [5:3] TO_TIME | not set by Android |
| CV | CHG_CNFG_04 G#BD [5:0] CHG_CV_PRM | 4.000 V + 10 mV × code up to G#32 (4.50 V); G#38 = 3.8 V, G#39 = 3.9 V (:1362-1400) |
| register protection | CHG_CNFG_06 G#BF [3:2] CHGPROT | 3 unlocks the protected registers, 0 locks (`max77759_chg_prot`, :301) |
| hardware JEITA | CHG_CNFG_08 G#C1 [2] JEITA_EN | left on |
| USB input limit | CHG_CNFG_09 G#C2 [6:0] CHGIN_ILIM, [7] NO_AUTOIBUS | set: 0 = input suspended, ≤ 3.2 A: 4 + (I − 125 mA) / 25 mA, above: G#7F; read: 100 mA + (code − 3) × 25 mA; Android always sets NO_AUTOIBUS 1 (:1482-1535) |
| wireless input limit | CHG_CNFG_10 G#C3 [5:0] WCIN_ILIM | `max77759_wcin_set_ilim_max_ua` (:1562) |
| input selection | CHG_CNFG_12 G#C5 | CHGINSEL, WCINSEL, VCHGIN_REG (1 = 4.6 V, 2 = 4.7 V) |
| watchdog enable | CHG_CNFG_18 G#CB [0] WDTEN | protected; tWD = 80 s (:432) |
| state | CHG_DETAILS_01 G#B6 [3:0] CHG_DTLS | 0 dead battery (prequal), 1 CC, 2 CV, 3 top-off, 4 done, 6 timer fault, 7 detbat high suspend, 8 off, A off high temp, B off watchdog, C off JEITA, D off temp (:43-55) |
| input currents | the gauge's side | CHGIN current = raw × 125 µA, WCIN = raw × 156 µA (:1864, :2306) |

Interrupts Android unmasks (`max77759_int_mask`, :101): CHG_INT AICL, CHGIN, WCIN, BAT, INLIM, THM2; CHG_INT2 INSEL, STA_TO, STA_DONE.
It masks CHG_STA_CC, CHG_STA_CV and CHG_I because the CV and input loops chatter at the end of charge.
These come in on gpa8-0, which a ferros driver can wait on instead of polling.

## Android's algorithm on husky

### The layers

- **google_battery** (`google_battery.c`, `google_bms.c`): multi-step charging ("MSC"): picks the CC limit and the CV (float, `fv_uv`) from the temperature and voltage tables, compensates IR drop, switches tiers, runs adaptive charging, and holds the displayed SOC at 100% after a full charge (recharge logic, `rl_soc_threshold` 95).
  It runs every 30 s, every 2 s near a tier voltage (`cv-update-interval` default 2000 ms).
- **google_charger** (`google_charger.c`): takes google_battery's votes and applies them to the charger, negotiates the USB PD contract, runs the defenders and the charge limits, and stops charging when a limit says so.
- **google_cpm** (`google_cpm.c`, the `gcpm` supply): chooses between the main charger and the PCA9468.
- **max77759_charger**: the registers, and a vote table that turns the callers' wishes into one MODE (`max77759_usecase.c`).
- **usb_psy** (TCPC side): turns BC1.2, Type-C and PD into one input current limit.

### CC and CV steps (husky's table)

Rows are battery temperature bands (gauge temperature, `google,chg-temp-limits` = 0, 10, 20, 42, 46, 48, 55 °C); columns are voltage tiers (`google,chg-cv-limits` = 4.20, 4.30, 4.35, 4.48 V); cells are the CC limit in hundredths of C (`google,chg-cc-limits`, husky), multiplied by 4950 mAh (`google_bms.c:177-188`).

| °C | to 4.20 V | to 4.30 V | to 4.35 V | to 4.48 V |
|---|---|---|---|---|
| 0 to 10 | 0.5 C = 2.48 A | 0.3 C = 1.49 A | stop | stop |
| 10 to 20 | 0.7 C = 3.47 A | 0.3 C | 0.3 C | 0.3 C |
| 20 to 42 | 1.2 C = 5.94 A | 1.2 C | 1.2 C | 0.5 C = 2.48 A |
| 42 to 46 | 1.0 C = 4.95 A | 1.0 C | 1.0 C | 0.5 C |
| 46 to 48 | 0.5 C | 0.5 C | stop | stop |
| 48 to 55 | 0.3 C | stop | stop | stop |

- Below 0 °C or at 55 °C and above charging stops entirely (`msc_logic_soft_jeita`, google_battery.c:3328).
- A zero means the last tier with current is the end: at 0 to 10 °C the cell is charged to 4.30 V only, at 48 to 55 °C to 4.20 V.
- Neighbouring tiers with the same current are merged (`gbms_msc_voltage_idx_merge_tiers`): at room temperature that is one CC stage at up to 5.94 A to 4.35 V, then 2.48 A to 4.48 V, then CV at 4.48 V.
- 5.94 A is only reachable thru the direct charger; the main charger's CHGCC tops out at 4 A, and the input limit and the thermal budget (`google,mdis-*`) usually bind first.
- **Cycle aging (AACT, AAFV):** past 300 and 1000 cycles (`google,chg-aact-ecc`) the 20 to 42 °C row drops to 1.1 C and then 1.0 C; the last tier's 4.48 V drops by 0 to 90 mV as cycles go from 200 to 950 (`google,aafv-ref-offset`, mV, per battery vendor).
- **A tier step:** while the cell is under the tier voltage, the charger's float is set above it by the IR drop seen between the charger's and the gauge's battery voltage (`msc_logic_irdrop`, :3352: fv = vtier + (vchrg − vbatt)), rounded to 10 mV (`fv-uv-resolution`) and capped at vtier × 1.025 (`fv-uv-margin-dpct` 1025).
  Near the tier it raises the float a click at a time (RAISE) or pulls it back if the gauge goes over vtier + 12 mV (`cv-otv-margin`), with a penalty of 10 updates (`cv-tier-ov-cnt`).
  It moves to the next tier when the current has stayed under the next tier's limit for 3 updates (`cv-tier-switch-cnt`), after a debounce of 3 (`cv-debounce-cnt`), with 20 mV counting as "at the tier" (`cv-range-accuracy`).
- In the last tier it stops adjusting and lets the charger's own CV, top-off and done run.

### Input current limits by source

The TCPC side votes one input limit (usb_psy.c; highest priority wins among the protocol votes, then the lower of that and the thermal vote):

| source | input limit |
|---|---|
| USB SDP, before enumeration | 100 mA (USB 2) / 150 mA (USB 3) |
| USB SDP, configured | **500 mA (USB 2)** / 900 mA (USB 3) |
| USB suspended | 2.5 mA |
| SDP never enumerating | after 5 s (600 s on the first connect after boot) treated as a DCP |
| CDP or DCP (BC1.2) | 500 mA at once, 1.5 A after PD negotiation has had its chance |
| dead battery | 500 mA |
| Type-C Rp 1.5 A / 3 A | that current |
| USB PD fixed | sink caps 5 V 3 A and 9 V 2.2 A (19.8 W), operating power 7.6 W (google_charger.c:590-603) |
| USB PD PPS | direct charging thru the PCA9468, below |
| wireless | WCIN, with the thermal steps 1.1, 0.5, 0.25, 0.11 A (`google,wlc-thermal-mitigation`); 23 W stands go thru the PCA9468 |

The dev box's port is USB 2 SDP, so Android itself would charge at 500 mA there.

### Termination and top-off

- The charger ends on its own: CV, then top-off below TO_ITH for TO_TIME, then done (CHG_DTLS 3, then 4).
  Android sets TO_ITH from the battery's CHARGE_TERM_CURRENT (google_charger.c:4398; husky has no `chg-topoff-limits`) and leaves TO_TIME as found.
- "Full" for Android is CHG_DTLS done and the cell at or over 4.40 V (`max77759,chg-term-voltage` 4400 mV, 200 mV debounce once done, max77759_charger.c `max77759_is_full`, :2164).
- After done, google_battery restarts only if the cell falls 200 mV under the tier (`MSC_DONE restart`, :5028); the displayed SOC holds 100% down to 95% (recharge logic).
- `google,chg-termination-5v` (husky sets it): on full, the PD contract drops back to 5 V.
- An overcharge guard (`chg-termination-enable`, not set on husky) would suspend input to keep the coulomb count within 0.5 to 0.75% of the count at termination.

### When the direct charger takes over

google_cpm moves charging to the PCA9468 (husky `zuma-shusky-charging.dtsi`, `zuma-husky-charging.dtsi`) when:
- the adapter offers PPS (or the wireless receiver can), and
- the battery's demand cc_max × fv is over 560 mW (`google,dc_limit-demand` 560000), and
- cc_max is over the CP minimum (1 A default for USB PPS, 2.75 A for wireless, `dc_limit-cc_min_wlc`), and
- the cell is between 3.4 V (`dc_limit-vbatt_min`, with 200 mV hysteresis below) and 4.45 V (the driver's default `dc_limit-vbatt_max`; husky's `google,dc_limit-vbatt` 4.35 V is a property this driver version does not read, UNCONFIRMED which one rules).

It drops back to the main charger under those, in the last tiers, and on thermal limits.
The PCA9468 runs at most 3 A in (6 A out, `pca9468,input-current-limit` 3000000), the adapter up to 10.5 V, its float 4.3 V, its own top-off at 500 mA in.
Thermal budgets: `google,mdis-out0-limits` (main charger, 4.0 A falling to 0.5 A in 26 steps) and `mdis-out1-limits` (direct, 6.0 A to 1.0 A).
Typical fast charge on a 3 A PPS adapter, from Google's time-to-full table: 64% at 30 min, 83% at 65 min, 100% at 141 min (`google,ttf-*`, husky).

### Charge limits: the owner's "max charge" and "scheduled charging" in Android

| feature | knob | husky's setting | what it does |
|---|---|---|---|
| charge limit (DWELL-DEFEND, retail) | `charge_stop_level`, `charge_start_level` (google_charger sysfs, SOC %) | 100 / 0 (off) | stop charging at the stop level, start again at or under the start level; above the stop level the input is cut too, so the cell feeds the system (`chg_run_defender`, :2181) |
| "Limit to 80%" (charging policy LONGLIFE) | charging policy vote | off unless chosen | stop 80, start 79 (google_bms.h:813-814) |
| adaptive charging | `charge_deadline` (seconds from now, from userspace's alarm or learned plug-in pattern), `charge_limit` (an always-on SOC), `health_safety_margin` | rest at 80% (`google,chg-rest-soc`), rest rate 0.15 C = 0.74 A (`google,chg-rest-rate` 15), before the trigger 0.8 C | under 80% charge at up to 0.8 C; from 80% charge at 0.15 C, and pause entirely while deadline > now + time-to-full + 30 min, so 100% lands at the deadline (`msc_logic_health`, :3718); it gives up if the deadline cannot be met, and stops for custom levels or TEMP-DEFEND |
| TEMP-DEFEND | `bd_*` | **dry run** on husky (`google,bd-temp-dry-run`): it counts but never stops charging | with the cell at or over 4.27 V for 6 h of time above 35 °C (`bd-trigger-*`), hold between 79% and 80% (`bd-recharge-soc`, `bd-drainto-soc`) and report overheat; resume at 28 °C, or 4 h, or SOC 50% |
| DOCK-DEFEND | `dd_*` | 100 / 0 (off) | the same limits while on a dock |
| TRICKLE-DEFEND | `bd-trickle-*` | disabled (b/259428308) | |

### The watchdog, and what needs pinging

- Android leaves the MAX77759's charger watchdog off (`data->wden = false; /* TODO: read from DT */`, :3207, then `max77759_wdt_enable(data, false)`).
  Nothing pings it, and the Pro's WDTEN reads 0 too.
  If enabled (protected register, CHGPROT first), it expires after 80 s without a WDTCLR write and the charger falls to CHG_DTLS B (off, watchdog) with safe defaults.
- The fast-charge safety timer is the only hardware timer that matters, and Android turns it off.
- The PCA9468 has its own watchdog bits (`PCA9468_BIT_WATCHDOG_EN`, `_CFG`); the driver only reports expiry; whether it arms it is UNCONFIRMED.
- Software timing: google_battery every 30 s (2 s near a tier), google_charger every 30 s in CC (`DRV_DEFAULTCC_UPDATE_INTERVAL`) and 2 s otherwise, the battery's own update every 47 s; all wake on charger and gauge interrupts too.

## Capture plan: watching Android charge (plan only, needs the owner)

### Which device, and how it gets to Android

The Pro: it has the PCA9468 and the husky cell, which the 8a does not.
Its memory note says it is the owner's wife's device (`pixel8_dedicated_dev_device.md`) and the 8a "could be anyone's" (`akita_pixel8a.md`); the owner should say which device and whose consent this takes before anything boots Android.
Android starts only by ferros's hand (BOOT.md "Android only by the identity's choice"): on a development build, volume down at the gate's Ready? screen (hands on the device), or `bridge diag android` once an identity is unlocked on the Pro.
Any reboot of Android lands in ferros again.
Before the first capture, check by hand that slot a's Android still boots to a home screen with USB debugging on (it was last used that way on 2026-10-25, unrooted); if it lands on "Cannot load Android system", stop after the first failure and never choose factory reset (`android_boot_experiments_rescue_party.md`).

### What to read, how often, from where

Three tiers; each later tier needs more from the owner.

0. **Unrooted, over adb (no change to Android):**
   - every second: `/sys/class/power_supply/battery/{capacity,current_now,current_avg,voltage_now,temp,status,charge_type,charge_counter}`; `/sys/class/power_supply/usb/{online,usb_type,current_max,voltage_max,input_current_limit}`; the `main-charger`, `dc-mains`, `gcpm` and `wireless` supplies' `status`, `constant_charge_current_max`, `constant_charge_voltage_max`, `input_current_limit`.
   - every minute: `dumpsys battery`; the google_charger and google_battery sysfs (`charge_stop_level`, `charge_start_level`, `charge_deadline`, `charge_limit`, `bd_state`, `dd_state`, `charge_stats`, `charging_status`, `charging_type`).
   - which of these SELinux lets the shell read is UNCONFIRMED: the first 15 minutes of the first session find out.
   - This gives what Android chose (float, current limit, input limit, source) but not why.
1. **Rooted (Magisk on slot a's init_boot, re-signed with keys/avb.vsf; one slot only):** add the kernel log (`dmesg -w`, every `MSC_*` line is google_battery's and google_charger's decision with its inputs: tier, temperature index, float, cc_max, IR drop state), the logbuffers (`/dev/logbuffer_ttf`, `_bd`, `_ssoc`, the TCPM's and usbpd's), and the charger's `registers_dump` sysfs (B0 to CC, the driver's own read) every 10 s.
   The dump reads B0 and B1 as well, which may take an interrupt from under the driver: every 10 s rather than every second for that reason.
2. **hv register oracle (no change to Android at all):** the hv traps hsi2c_11's page (G#10CB_0000) in stage 2 and logs every transfer Android makes to G#69, G#36 and G#57 with its time: the exact register writes and their order, which the other tiers only infer.
   The oracle builds stopped booting Android on 2026-10-29 for a reason never found (`android_boot_experiments_rescue_party.md`), so this waits until that is fixed.

Recommendation: tier 0 first, tier 1 if the owner allows root on the Pro's slot a, tier 2 only if a question remains about Android's exact writes.
Captures go to `/mnt/Harbor/Code/keys/husky/charging-<date>/` (local, never committed: they carry the device's serials).

### Conditions

| run | how | what it shows | time |
|---|---|---|---|
| probe | Android up, cable in | what the shell can read | 15 min |
| source sweep, at 40 to 60% | 2 min each: the dev box's port (SDP), a plain brick (DCP or Rp), a PD charger, a PPS charger (a Pixel 30 W), a wireless pad if there is one | input limits, PD contract, when the PCA9468 takes over | 1 h with the swaps |
| full cycle, PPS charger | from 10 to 20% to done, then 30 min more | tiers, IR drop, the direct charger's handover and its way back, termination, the 5 V drop on full | about 2.5 h, plus the drain |
| full cycle, dev box USB | from 10 to 20% to done | the main charger alone at 500 mA, top-off and done | 10 to 13 h, overnight |
| 80% limit | turn on "limit to 80%" in Android's battery settings, charge from under 79% | stop and restart, input cut above the stop level | 2 to 3 h |
| adaptive charging | Android's adaptive charging on, an alarm set, plugged in at night | the 80% rest, pause, finish at the deadline | one night |
| cool | device chilled in a sealed bag (a refrigerator, about 5 to 8 °C), charged on PPS while it warms | the 0 to 10 and 10 to 20 °C rows (4.30 V stop when cold), JEITA | 1 to 2 h |
| warm | the owner's call: the PPS cycle may reach the 42 to 46 °C row on its own; deliberate heating is not proposed | | |

Draining to the start point: unplugged with the screen on in Android or ferros (ferros's focused scope at about 4 W drains in about 5 h); no capture needed for the drain, and no run starts under 10% (prequalification below about 3 V is not worth the cell).
Hands needed: starting Android, cable and charger swaps, the settings toggles, the refrigerator; the rest runs unattended.
Elapsed: about two to three days, mostly overnight runs.

## What a ferros driver must get right first (not built)

- The safety timer: clear FCHGTIME as Android does once ferros owns termination, or keep it as a net at its longest setting; today it is what stops the Pro.
- The input limit from the source: CHGIN_ILIM from BC1.2, Rp and PD (USB.md's TCPC side), with NO_AUTOIBUS set as Android does.
- The CV and CC from a table like the one above, and the temperature cut-offs, from the gauge.
- Interrupts on gpa8-0 rather than polling (CHGIN, INSEL, STA_DONE, BAT, THM2, AICL, INLIM).
- MODE writes are shared with `diag power start` (the cut for measurement), so one owner of CHG_CNFG_00.

## Decisions for the owner (later, with Android's behaviour as the reference)

0. **The safety timer.** Android: off, software owns termination. Options: the same; or on at its longest as a backstop.
1. **Charge ceiling ("max charge").** Android: 4.48 V (100%) by default, less with cycle age (AAFV); optional "limit to 80%" (stop 80, start 79 by SOC); arbitrary stop and start levels. ferros can cap by SOC like Android or by CV voltage directly (4.00 to 4.50 V in 10 mV steps), which is what the cell's aging follows.
2. **Charging current.** Android: the C-rate table by temperature and voltage (up to 1.2 C = 5.94 A at room temperature thru the direct charger), the input limit by source, and thermal steps. Options: a cap on CHGCC (main charger, up to 4 A), whether to do PD and PPS and the direct charger at all, or a fixed gentle rate.
3. **Scheduled charging.** Android: adaptive charging rests at 80% (0.15 C, or paused) and finishes at a deadline from an alarm or a learned habit. ferros: a time to be full by, set in the battery page, or a schedule by time of day.
4. **Heat.** Android: TEMP-DEFEND exists but runs dry on husky; the C-rate table and the charger's JEITA do the work.
5. **The charger's watchdog.** Android: off. Turning it on means a dead ferros puts the charger back to safe defaults within 80 s, at the price of a ping every 80 s.
6. **The capture:** which tiers (root on the Pro's slot a or not), which device and whose consent, and the runs above.
