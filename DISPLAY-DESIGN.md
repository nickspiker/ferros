# DISPLAY-DESIGN — the husky display driver: decisions, plan, profiling, HDR

Decided with the owner 2026-10-16: **display first, prim and proper — every mode, 8 and 10 bit, all profiled — then the display pipe (the DPU), then the camera.** No driver code until this plan is agreed. DISPLAY.md is today's state (a frame pusher on the surface ABL leaves running); DISPLAY-HK3.md is the panel inventory; `oracle/display/husky-hk3/` is how Android drives it, captured live.

## 0. The panel is google-hk3

The first inventory here came from `panel-samsung-s6e3hc3-c10.c`, a sibling device's panel, which is why its geometry didn't match. The phone says `panel_name = google-hk3`, model HK3-A0B-00, on **DSIM0** driven by **DECON0**. Native 1344×2992 at 60/120 Hz; a 1008×2244 pair the **panel itself scales** (`C3 0D` + `F2 81`); LP (AOD) at 30 Hz in both sizes; idle floors of 1/10/30 Hz by the panel's auto-rate engine; a 60 Hz "normal speed" op mode (`60 18`) beside the 120 Hz "high speed" one. DSC 8 bpc / 8 bpp, two slices of 672×187. Brightness 2–1000 nits normal (DBV 196–3307), 1000–1600 nits HBM (3308–4095), default 140 nits. 4 DSI lanes at 1346 Mbps as Android runs them (1368 as ABL leaves them; ferros does either), command mode, TE on gpp0-3. Every byte of every transition: DISPLAY-HK3.md §3–5 and the oracle.

## 1. Inventory and oracle

- **DISPLAY-HK3.md**: modes, DSC/PPS fields, every command set and transition, brightness/HBM/ACL/dimming, TE/TE2, rails and reset, DSIM0/DPHY/DECON0 addresses, the stock driver's control surface — with line citations.
- **oracle/display/husky-hk3/**: for 26 states (each mode with idle off and on, idle floors, NS/HS op mode, brightness levels, HBM, dimming, AOD in/out, full OFF and ON): the DSI packets Android sent to get there (bytes, types, waits, timings), the DPU register dump afterwards (DECON0, windows, DPP, DMA, HDR, SRAMC, DSIM, DPHY, link), and the panel's sysfs. The `ON` capture is the complete power-on: PPS, `11`, the init set with its 10 ms and 110 ms waits, C3/F2, features, `29`. The DPU register dumps are the DECON/DSIM configuration for each mode, which no driver source gives directly.

## 2. Decisions (the owner's)

1. **All modes.** WQHD and FHD at 60 and 120, the panel's idle/auto floors (1/10/30), NS 60 Hz, AOD 30 Hz, HBM, dimming — each brought up and checked against the oracle's register state.
2. **8 and 10 bit, both, profiled.** Everything Android runs is 8 bpc DSC. The 10-bit path exists in DECON (`TEN_BPC_MODE`, 1010102 formats, dither/DQE) and the driver's F2 register is commented "8/10bit config"; whether the panel takes a 10 bpc PPS is UNCONFIRMED and is an experiment, not an assumption.
3. **"GPU" means the DPU** (DECON/DPP: scaling, blending, DQE, the 2D pipe to the panel). fluor is a CPU compositor; the shell never needs Mali, and a bare-metal Mali is a separate project.
4. **Cold power-up from off**: yes, prim and proper — rails, reset, the full init — not a dependence on ABL leaving the panel lit.
5. **The kernel keeps its pre-process painter** (Ready? gate, checklist); the display server takes over at first process.
6. **Camera after the display.** AoC audio after that.

## 3. Architecture

A **display server process** (EL0, masked/sealed) holding grants for DECON0, the DPP layers it uses, DSIM0 + DPHY, the DPU power/clock registers and the PMIC path for the panel rails; a DMA framebuffer inside its arc (non-cacheable, the S2MPU fence over it); and a `display` endpoint other processes draw through. The shell (SHELL.md) is a client of it. Why a process: the panel bring-up is long and full of waits and DCS round trips, a bad DCS write ends the server rather than the kernel, and the capability model already fits.

## 4. The plan, in order

1. **DSI command path** — DONE 2026-10-16 (DISPLAY.md): DCS writes and reads, brightness proven by readback, refresh 60 ⇄ 120 by the panel's own command sets (`hk3.rs`, `bridge display mode`), measured on the TE pin; DSIM's command-TE timers set per rate. Originally: Send DCS over DSIM0's FIFOs; prove it with the smallest visible commands and their oracle bytes: brightness `51 hh ll`, dimming `53 28/20`, display off/on `28` / `29` with the F4 Vreg/AMP bracket, HBM `53 E0` + `51 0F FF`. Then the mode switches: `60 01` / `60 00` with the `BD` early-exit table and the TE2 update, exactly as `W60_noidle` / `W120_noidle` show. This alone unblocks the wake policy and power saving.
2. **Our own framebuffer, and DECON configured like Android's.** Measured 2026-10-16 (`oracle/display/husky-hk3/tools`-style diff of the live registers against the W120 dump): ABL leaves DECON within 31 of 708 words of Android's 120 Hz state. What Android sets that ABL doesn't, which this step must set: `GLOBAL_CON.TEN_BPC_MODE` (DECON composes in **10-bit**), `DATA_PATH_CON_0` enhance path = dither + DQE (ABL: none), SRAM allocation for every window it uses (`SRAM_EN_OF_PRI_0/1`), out-FIFO QoS (`OF_URGENT_EN`, `OF_RD_URGENT_0/1`, `OF_DTA_CONTROL/THRESHOLD`), the interrupt enables, and per layer the HDR block (`HDR_LSI`: EOTF LUT, gain, OETF LUT — Android degammas each layer to linear, blends at 10 bpc, re-encodes) plus `DPP_COM` and `SRAMC` mode bits. So Android's "10-bit" is internal precision for colour management in front of an 8 bpc DSC link — exactly the role §6 gives it. Tried 2026-10-16: setting `TEN_BPC_MODE` with the dither bit but without the DQE's dither configuration and Android's DSC_CONTROL1 (G#55) stalls DECON on a frame timeout and nothing short of a reboot recovers it; the SRAM/QoS/interrupt part is in (`display tune`), the 10 bpc part turned out to need only the DQE in the enhance path and sized (its dither is one enable bit; the dump had the sizes) — done 2026-10-16 as a modeset option (`dpu.rs`, DISPLAY.md). The framebuffer itself — DONE 2026-10-16 as a region (`cachemap::DISPLAY_FB`, 32 MiB at G#8400_0000, non-cacheable, EL0 RW, DMA grant `display-fb`; `bridge display fb own`): the base layer reads it with no DMA error, so the DPU reaches ferros's DRAM unfenced. Left for the server: owning the layer registers itself (DECON0 window 5 / DPP0 / DMA0 from the dumps) instead of inheriting ABL's, and the shadow-update → trigger → frame-done loop off TE.
3. **Every mode** — DONE 2026-10-16, 10 bpc compose included (DISPLAY.md): FHD ⇄ WQHD by the full modeset (DISPLAY-MODESET.md), the auto-rate idle floors (30/10/1 Hz, measured with changeable TE), NS 60 (TE 1201 µs), AOD (30 Hz, 687 µs, DBV 853) and back, HBM and dimming. Each state's registers diffed against the oracle's dump: with `modeset … 10` the 10 bpc + dither + DQE words match too; what still differs is the window/buffer choices (ours), Android's AFBC bit and its DPP IRQ enable. The DSIM link matches word for word once the modeset takes Android's 1346 Mbps link rate (DISPLAY-HK3.md §7); at ABL's 1368 the PLL, lane timing and TE-timer words are the rate's own.
4. **Cold power-up.** VDDI → VCI → reset {1,1,5} ms → extinfo/ID reads → PPS → init (10 ms, 110 ms) → VDDD, per DISPLAY-HK3.md §6 — needs the S2MPG14/15 regulator path (LDO14, LDO22, BUCKA) from EL2; then the full `OFF` sequence (`28`, F4 writes, `10` + 100 ms, rails down). Verified by cutting the panel off and bringing it back without ABL.
5. **10-bit experiment.** DECON `TEN_BPC_MODE` + a 10 bpc PPS, with the F2 register's bits explored; the result is either a working 10-bit mode or a documented refusal. Dither through DECON either way (it is what makes 8-bit-under-HDR-headroom clean). **Prepared 2026-10-16** (`modeset … link10 [f2=XX]`, DISPLAY.md): the pipe runs it; the verdict needs eyes on the panel. The 10 bpc blend + DQE dither on the 8 bpc link is done and the default.
6. **The display server and `display` endpoint**, fluor's no_std path rendering the shell on the phone.

## 5. Profiling: what "profiled" means here

- **Timing**: per mode, TE period and width (the panel's TE constants: 273 µs at 120, 8500 µs at 60 HS, 1223 at 60 NS, 693 in AOD), frame transfer time against the underrun budget, mode-switch latency (the oracle has the transition timings: a W120→W60 switch is 23 packets in ~6 ms plus the vsync waits).
- **Power**: per mode and brightness, measured — not estimated. **Done under ferros 2026-10-16: DISPLAY-POWER.md** (the gauge read over hsi2c_11 with the charger's buck off so the battery carries the system; solid-fill scenes; every mode, 8/10 bpc, both link rates, three brightnesses, HBM). Headline: content (+2.4 W white over black at 140 nits) and brightness (black alone moves 0.8 W between 2 and 140 nits) dwarf refresh rate (100–200 mW) and everything in the pipe (10 bpc, link rate, FHD: nothing measurable). Still to do: AOD, the Android baseline for the same scenes, 20 s averages.
- **Colour**: the panel's code-to-nits curve at several brightness levels, measured with a photometer — the one input the HDR mapping below cannot get from any register. Samsung DDICs calibrate gamma per brightness level, and HBM differs again.
- **10 bit**: whether it exists on the link (plan step 5), and if so, measured banding versus 8 bit + dither.

## 6. HDR rendering: gamma2tanh to the panel

Content arrives as VSF `gamma2tanh` (vsf colour.md "HDR: Gamma 2, Then tanh": `tanh(√L)` per channel, diffuse white at code 194/256, 3.3 stops of headroom at 8 bits, 5.1 at 16). The display server renders it as follows.

1. **Pick diffuse white in nits**, W: from ambient light when a sensor is read, else a setting (140 nits is the panel's default level; a dim room wants less).
2. **Headroom** H = P / W, where P is the peak the panel can give right now: 1000 nits normal, 1600 in HBM (HBM is a budget — Android caps it to 5 minutes per 30 — so H is a function of the HBM timer, not a constant).
3. **Drive the panel at P**: `51` to the DBV for P (and `53 E0` for HBM), so the brightest code is P nits.
4. **Map codes**: decode L = atanh(v)². SDR content (L ≤ 1) is scaled by W/P in linear light so it sits at W nits; the tail (L > 1) is placed between W and P. If the content's headroom exceeds H, squash the tail with the same tanh at the display's scale: `v' = tanh(√(L / L_max) · k)` with k chosen so L = 1 lands at W/P — the display's tone map is the decoder with a different scale, never a clip.
5. **Precision**: scaling SDR down by W/P costs √(W/P) of the panel's codes (at W/P = ¼, half of them); dither in DECON — or 10 bit on the link if step 5 of the plan delivers it — keeps gradients clean. This is where 10 bit earns its place: precision under headroom, not range.
6. **OLED makes this cheap**: only the highlight pixels emit more; the SDR part costs what it did. Sustained bright area is bounded by the panel's power and the HBM timer, which is physics, not the encoding.
7. **Calibration input**: the measured code-to-nits curve per brightness level (§5), applied as a per-level LUT before DSC. Until it is measured, the mapping is approximate.

Untagged content is regular VSF RGB and renders at W with no tail.

## 7. Open

- The regulator path from EL2 to S2MPG14/15 (ACPM/SPMI), needed for cold power-up and for reading the panel's rails.
- Whether the panel accepts a 10 bpc PPS (plan step 5).
- The photometer measurement (needs the owner and an instrument).
- Touch input (SPI, `spitouch`), which the shell needs and nothing covers yet.
