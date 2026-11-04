---
name: no-baked-apps
description: "Owner 2026-11-04: the base OS has ONE built-in circle (system settings); every app — clock, Photon, Mahere, Android launcher — is installed by the person, never preloaded; attest loads only attest"
metadata:
  type: feedback
---

The ferros base OS has exactly one built-in circle: system settings (dashboard front, settings back). Nothing else is baked in: no clock, no Ramps, no Photon/Mahere/Lumis/Opsin circles (Photon included: owner 2026-11-04, "Photon is just an app. on non ferros it just happens to have the FGTW bits in it too"), no Android launcher, not as a circle and not as a settings row. The owner said "You want a clock? INSTALL IT YOURSELF!!!" and "So now you're baking Android into everyones OS???". Before an attest, only the attest screen and device drivers run. Nothing that reads the person's space (their arrangement, apps or Photon) loads until they attest: "Attest should ONLY load attest. That's how we get bugs and security holes."

**Why:** ferros is the opposite of bloatware. A preloaded app list also means the OS "knows" a person's apps and layout before they unlock, which the owner rejected outright. Every extra thing loaded is an extra attack surface.

**How to apply:**
- Never add a circle, row or widget to the base OS for a specific app, Android included.
- A feature that needs a UI entry point waits for installing; it is not a base-OS shortcut. Say so plainly instead of finding another built-in spot (the 2026-11-04 mistake went circle, then "move it into settings", rejected both times).
- Rights that only an app would use (e.g. the kernel `android` switch) sit in no one's grants until an installed app holds them.
- Built 2026-11-04 in e982af4. Related: [[android-behind-attestation]], [[identity-not-on-disk-default]].
- The FGTW side (identity lookup, mint, fleet) is the OS's: process `fgtw` (payloads/fgtw, vault scope `fgtw.`), part of attest, renamed from `photon` in 256dc23. Never call it Photon or treat it as the app.
