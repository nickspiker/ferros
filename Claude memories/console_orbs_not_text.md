---
name: console-orbs-not-text
description: "console circles: since 2026-11-09 radios show numbers (orbs off behind ORBS), GPS shows satellite rings meeting over the person; the helm = bottom two buttons, wedges 1/8 black, white when pressed"
metadata:
  node_type: memory
  type: feedback
  originSessionId: a83238a3-4495-4510-b376-d5e6ba884c36
  modified: 2026-11-07T22:56:21.253Z
---

**Changed 2026-11-09 (owner: "I haven't thought thru what I want it to look like and get used to the numbers so switch to the most useful stats"):** the radios on show their name and up to four measures as text, like the battery (display `radio_lines`); GPS on draws each satellite as a bubble-limb ring through the centre so the point over the person brightens with more satellites (`gps_rings`); the orbs are kept behind `ORBS = false` until the look is decided. The helm (the bottom two buttons): only the home ring is a circle; each wedge is an eighth-opacity layer, black at rest, white by the press level (display op 54). The rest below is the earlier direction.

Owner, 2026-11-07: "We won't be having words in these circles." The console circles are organic orbs.
- They draw each radio's state and measures graphically.
- The orb carries text only when that radio is off ("LTE", "WiFi", etc.). An illustrative variant may come later; that is undecided.
- Slots, called "tracks": add as many per radio as needed to show everything. The 8 per radio is not a design limit.
- Gestures (owner, 2026-11-07):
  - tap a radio's circle: that radio on or off (built: `radios_off`, SETTINGS.md);
  - tap, slide toward the screen's centre and release: open that radio's detail page, the only place with numbers and config.
- The visual reference is Lumis's camera picker buttons (`lumis/rust/src/ui_main_menu.rs` `draw_camera_button`): the rim profile and the R/G/B channel separation.

**Why:** the owner kept seeing me design text lines and 9-line layouts for the circles, which misses the orb concept.

**How to apply:**
- Never plan console content as lines of text.
- Size the measure tracks by what the orbs render, not by what fits as text.
- Port looks front to back with `under`, in darkness space, C¹ (see [[feedback-darkness-space]]).
