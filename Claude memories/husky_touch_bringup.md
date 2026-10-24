---
name: husky-touch-bringup
description: ferros brings husky's FTM5 touch controller up from a cold boot and reports contacts (finger-verified 2026-10-20) — diag touch state/up/watch/paint/map/raw/decay/off; capture method that works (boot probe at loglevel=4, NOT fts unbind which panics); the three bugs that silently killed contact reporting
metadata:
  type: project
---

Contact reporting finger-verified 2026-10-20 (2035 readings in 30 s, pressure to A#123, largest contact A#352 by A#224). Working since 2026-10-19 on slot b, no Linux: `bridge diag "touch up"` gives controller ready in 12 ms, chip ID G#3640 (SALIXP), scan mode on; `"touch paint"` draws strokes on the panel for 16 s; `"touch watch"` prints events. Code ferros_kernel/src/touch.rs; recipe, protocol and pins/clocks/rails in ferros/docs/oracle/TOUCH.md.

**Facts that cost time (don't re-derive):**
- Driver source is public: AOSP kernel/google-modules/touch/fts_touch, cloned at /mnt/Harbor/ferros-ref/touch-fts (branch android-gs-shusky-6.1-android15-qpr2, dir ftm5/).
- Capture the boot probe, not a rebind: `echo spi20.0 > .../fts/unbind` PANICS Android (of_drm_find_bridge). Boot capture with HV_CMDLINE=loglevel=4 (tools/husky-hv.sh) keeps pstore clean; decode with tools/oracle-decode-spi.py.
- Leaving the touch-SPI oracle build running idle in Android ended in a watchdog reset; reflash the clean hv right after a capture.
- Cold: USI2 clock gates already open (auto), USI_CON=1 must be cleared, rails LDO25M/LDO4M programmed but off (bit 7 enables, via ACPM main PMIC), reset gpp21-2 is an input; SPI pads are fixed-function (no pinmux).
- System-info block holds a per-die ID at offsets 32-47: identity data, don't read or log it (see [[ira-entropy-sources-husky]]).

**Three bugs that each made the chip sense but report nothing — all fixed 2026-10-20 (commit d0e872b), don't reintroduce:**
- `setScanMode` (ftsCore.c) is TWO writes: a two-byte zero write to hw reg G#2000_0000 **then** `A0 <mode> <settings>`. Without the first the mode command is accepted silently and no contact is ever reported, while capacitance frames still show the finger. G#2000_0000 reads back as the chip ID, which is why the write looks like a capture artefact and is easy to drop.
- An event's identifier is the HIGH NIBBLE of byte zero only (`eventId = evt_data[0] >> 4`); the low nibble carries payload, including the top two bits of the major axis. Comparing the whole byte drops every contact wide enough to set them. Beware: EVT_CONTROLLER_READY is nibble 0 and an empty queue also reads 0x00, so rule out nothing-pending before matching identifiers.
- Drain the queue in bursts like `fts_interrupt_handler`: byte 7 of an event holds in its low five bits how many remain (its top two bits are the minor axis). One-at-a-time reads race the queue cursor and return the same event with its tail zeroed, which decodes as a contact jumping to the top of the screen at zero pressure. Seven events plus the dummy byte fill one 64-byte SPI segment.

Contact fields: x = ((e[3] & 0xF) << 8) | e[2], y = (e[4] << 4) | (e[3] >> 4), pressure = e[5], major = (((e[0] & 0x0C) << 2) | (e[6] >> 4)) * 16, minor = (((e[7] & 0xC0) >> 2) | (e[6] & 0x0F)) * 16. Axes confirmed: x 0-1343 across the short side, y 0-2991.

`diag touch decay` is the scope for this: a 1:1 10-bit decay screen (586 frames in 30 s, 29 ms a decay pass) stamping each contact's ellipse split across channels — major in red, minor in blue, pressure in green. Keep its canvas in the cacheable DISPLAY_HEAP and blit write-only; a read-modify-write straight on the framebuffer costs 1079 ms a frame.

Related: [[husky-camera-bringup]], [[husky-cold-slot-b]].
