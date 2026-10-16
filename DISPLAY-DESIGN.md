# DISPLAY-DESIGN — knobs inventory + proposed design (for review)

Written for the owner to review *before* any display-driver code (per the standing request). Nothing here is built yet. DISPLAY.md has the current "frame pusher on ABL's pipeline" status; this is the plan for a real driver. Sources: the husky vendor kernel (panel `s6e3hc3-c10`, DPU `cal_9865`, DTs).

## 0. The one thing to decide first: resolution

The panel is **`samsung,s6e3hc3-c10` on DSIM1**. The device tree says **1344×2992** (native husky). But the shipped panel *driver* source (mode tables, PPS, CASET/PASET) describes **1440×3120 / 1080×2340** — Pixel 7 Pro geometry — so the driver is a close cousin, not an exact match, and the true husky timing lives only in the DT. **Takeaway: the timing/DSC generator must be resolution-parametric, driven from measured values, not copied from the driver's constants.** We already run on ABL's adopted surface at the native **1344×2992**, so we have the real geometry in hand.

## 1. What the hardware gives us (condensed inventory)

Full register/command detail with citations is in the research notes; this is the designer's summary.

**Panel (DCS over DSIM1 command mode):**
- Lifecycle: sleep-out `0x11` (+120 ms), display-on `0x29`, display-off `0x28`, sleep-in `0x10`. Init set: TE-on `0x35`, TSP-sync, frame-insert `0xBD`, CASET/PASET, DSC-on `0x9D 01` + PPS, panel-mode `0xC3`.
- Reset: gpp10, HIGH→5 ms→LOW→5 ms→HIGH→10 ms. Rails: vci = `s_ldo8`, vddi = `s_ldo29` (enable vddi→vci).
- Brightness: `0x51` WRDISBV, **12-bit**, nits 2–500 (level 4–2047); HBM to 1000 nits (level 2238–3949) via `0x53` (BCTRL `0x20`, HBM `0xC0`, dimming `0x08`, local-HBM `0x10`) + IRC `0x60`.
- Refresh: 60/120 Hz normal, 10/30 Hz LP/AOD; select via `0x60` (120=`00`,60=`01`,30=`02`,10=`03`), **always latch with `0xF7 0F`**, early-exit/porch via `0xBD`. TE mode `0xB9` (changeable/fixed).
- Color: DSC 8 bpc / 8 bpp, 2 slices. Gamma switch `0x49`. HDR10/HLG advertised. DECON has a 10-bit path + dither/DQE (unused at 8 bpc).

**DPU (DECON v7.6 + DPP + DSIM):**
- 14 windows, 14 DPP layers; formats RGBA/BGRA 8888, 565, **1010102 (10-bit)**, FP16, NV12/P010; AFBC + SBWC; per-DPP scaling ¼×–4×.
- Command mode: write pixels → **shadow-update** (`SHD_REG_UP_REQ` 0x50: GLOBAL bit31, per-win, CMP bit20) → **trigger** (`TRIG_CON` 0x30, HW trigger from DDI/TE) → wait **FRAME_DONE** (INT bit13). This is the per-frame loop we already do for adoption.
- DSC: 2 encoders, slice-count 2, slice-height 52 (WQHD-class). TE from **gpp0-4** (EINT); DECON hardware-triggers off it.
- DSIM: 4 data lanes, hs-clk 1368 MHz, DCS via header/payload FIFOs (`0x58`/`0x5c`), FRAME_DONE/RX-TE interrupts.
- Power: DPU domains `pd_dpuf0`/`pd_dpuf1` (+ dpub), DSIM PHY `mipi_phy_dsim0_m4m4`, DECON enable bits, panel LP/idle states.

## 2. Proposed architecture

A **display server process** (EL0, masked/sealed like every other), not kernel code. It holds:
- hardware grants for DECON0, the DPP layer(s) it uses, DSIM + DPHY, and the DPU power/clock registers it needs (via the existing `--hw` registry + EL3 for power-domain reads);
- a **DMA region inside its arc** for its framebuffer (non-cacheable, the same model the WiFi driver will use), so masked accesses reach it and DECON DMAs from its physical base;
- a `display` endpoint other processes call to put pixels / text / status on screen.

The **kernel keeps only a tiny pre-process painter** — the "Ready?" gate and the boot checklist — because those run before any process exists. Everything after first-process is the server's job. This matches the blueprint ("drivers are processes") and keeps the kernel's display surface minimal.

Why a process, not kernel: the panel bring-up is long and full of delays and DCS round-trips; it wants its own time slices, its own fault isolation (a bad DCS write ends the server, not the kernel), and the capability model already fits (grants + a DMA arc + an endpoint).

## 3. Implementation tiers (smallest useful first)

1. **DSI command path.** Send DCS to the panel over DSIM's FIFOs: brightness (`0x51`), display on/off (`0x28`/`0x29`), sleep in/out. *Unblocks the proximity-gated wake policy and power saving without any pixel work.* Highest value per line.
2. **Own framebuffer.** Allocate the framebuffer in the server's DMA arc, point a DPP layer at it (linear, BGRX8888 as today), drive the shadow-update + trigger loop off TE/FRAME_DONE. Replaces drawing into ABL's surface.
3. **The `display` endpoint + the kernel painter handoff.** The server adopts the panel the kernel was painting, and other processes draw through it. Move the Ready/checklist painter's role to the server once a process exists.
4. **Cold power-up from off.** Rails → reset → the full init sequence (`0x11`/init-cmds/DSC+PPS/`0xC3`/`0x29`) + DSIM PLL + DPU power domains. This is the multi-session piece; only needed once we ever power the panel fully down (e.g. deep idle). ABL leaves it running, so tiers 1–3 don't need it.

## 4. Knobs to expose vs defer

- **Expose early:** brightness (12-bit), display on/off, one refresh mode (start at whatever ABL left, likely 120 Hz), the framebuffer.
- **Expose for the wake policy:** display on/off + brightness ramp + AOD/LP entry (10/30 Hz, `0x53` low-nit), since the prox-gated wake policy is a named goal.
- **Defer:** HBM (sun-readable), HDR, 10-bit path, multi-window compositing, SBWC/AFBC, per-DPP scaling, local-HBM. All real, none needed for a first UI.
- **Local-HBM is the under-display fingerprint spot** (fixed centre x=G#2D0 y=G#939, 6 mm radius), not a general per-region highlight tool. Highlight "pop" goes through global brightness (`0x51`, HBM) with the SDR part of the image scaled down in pixel values. On OLED that only brightens the highlight pixels, since there's no backlight.

## 5. Decisions for the owner

1. **Refresh strategy:** fixed 120 Hz for simplicity first, or wire variable-refresh (10–120 Hz) early because it's the big battery lever? LP/AOD ties into the wake policy.
2. **Wake policy coupling:** should the display server own the proximity-gated power-button policy (screen lights only if power pressed AND prox far), or is that a separate policy process that calls the display server? (The wake-policy memory says it's native ferros display-policy, not kernel.)
3. **Bit depth:** 8 bpc is what the panel's DSC is configured for; the 10-bit path exists. Start 8, revisit for Lumis/photo?
4. **Cold power-up priority:** worth building now, or lean on ABL leaving the panel up and only build full power-up when deep-idle/off actually matters?
5. **Kernel painter vs server:** keep the Ready/checklist painter in the kernel permanently (it must run pre-process), with the server taking over only after first process — agree?

No code until you've weighed these.
