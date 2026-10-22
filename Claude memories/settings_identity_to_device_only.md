---
name: settings-identity-to-device-only
description: Settings flow one way — an attesting identity's preference is written to the device; a device setting is NEVER read back into an identity's preference. Device base (tatau) is the first such setting.
metadata:
  type: project
---

A setting lives on the device (`device.*` in the device vault) or with the identity (fleet-wide, e.g. Photon's `display.base`). When an identity attests on a device, its preference is written to the device; a device setting sticks until the holder changes it or the next identity attests. Nothing ever writes a device value into an identity preference (owner, 2026-10-22: "obviously we never read the device base and write to the identity preference based on the value, only from identity to device").

**Why:** a device can be configured by whoever holds it; a person's preferences are theirs alone.

**How to apply:** any settings or sync code — ferros, Photon, Mahere — copies identity → device on attestation only. First setting: the number base, `device.base` = one byte of `tatau` code (10 arabic, 12 DMS, 16 hex, G#8C DLS), loaded at boot. Glyphs only where the font is ours; words in terminals. Spec in ferros SETTINGS.md; formatter is `Code/tatau` (public, nickspiker/tatau), which spirix and vsf should depend on. Related: [[photon_terminology]], [[ira_network_owns_assignment]].
