---
name: husky-touch-bringup
description: ferros brings husky's FTM5 touch controller up from a cold boot (2026-10-19) — diag touch state/up/watch/paint/off; capture method that works (boot probe at loglevel=4, NOT fts unbind which panics); finger test of axes still pending
metadata:
  type: project
---

Working since 2026-10-19 on slot b, no Linux: `bridge diag "touch up"` gives controller ready in 12 ms, chip ID G#3640 (SALIXP), scan mode on; `"touch paint"` draws strokes on the panel for 16 s; `"touch watch"` prints events. Code ferros_kernel/src/touch.rs; recipe, protocol and pins/clocks/rails in ferros/docs/oracle/TOUCH.md.

**Facts that cost time (don't re-derive):**
- Driver source is public: AOSP kernel/google-modules/touch/fts_touch, cloned at /mnt/Harbor/ferros-ref/touch-fts (branch android-gs-shusky-6.1-android15-qpr2, dir ftm5/).
- Capture the boot probe, not a rebind: `echo spi20.0 > .../fts/unbind` PANICS Android (of_drm_find_bridge). Boot capture with HV_CMDLINE=loglevel=4 (tools/husky-hv.sh) keeps pstore clean; decode with tools/oracle-decode-spi.py.
- Leaving the touch-SPI oracle build running idle in Android ended in a watchdog reset; reflash the clean hv right after a capture.
- Cold: USI2 clock gates already open (auto), USI_CON=1 must be cleared, rails LDO25M/LDO4M programmed but off (bit 7 enables, via ACPM main PMIC), reset gpp21-2 is an input; SPI pads are fixed-function (no pinmux).
- System-info block holds a per-die ID at offsets 32-47: identity data, don't read or log it (see [[ira-entropy-sources-husky]]).

**Pending:** an owner finger test of the axes (expect x 0-1343 across the short side, y 0-2991) via `touch paint`.

Related: [[husky-camera-bringup]], [[husky-cold-slot-b]].
