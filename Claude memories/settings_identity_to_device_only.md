---
name: settings-identity-to-device-only
description: Settings flow one way — an attesting identity's preference is written to the device; a device setting is NEVER read back into an identity's preference. Device base (tatau) is the first such setting.
metadata:
  type: project
---

A setting lives on the device (`device.*` in the device vault) or with the identity (fleet-wide, e.g. Photon's `display.base`). When an identity attests on a device, its preference is written to the device; a device setting sticks until the holder changes it or the next identity attests. Nothing ever writes a device value into an identity preference (owner, 2026-10-22: "obviously we never read the device base and write to the identity preference based on the value, only from identity to device").

**Why:** a device can be configured by whoever holds it; a person's preferences are theirs alone.

**How to apply:** any settings or sync code — ferros, Photon, Mahere — copies identity → device on attestation only. First setting: the number base, `device.base` = one byte of `tatau` code (10 arabic, 12 DMS, 16 hex, G#8C DLS), loaded at boot. Glyphs only where the font is ours; words in terminals. Spec in ferros SETTINGS.md; formatter is `Code/tatau` (public, nickspiker/tatau), which spirix and vsf should depend on. Related: [[photon_terminology]], [[ira_network_owns_assignment]].

**Kept as spec VSF since 2026-10-27** (owner: "All device settings should be proper spec VSF for aid in vault storage, all the things"; "language type inherits last attested identity"): one VSF document under vault key `device`, section `device`, a typed field per setting (base, labels, brightness, dither, theme, prox_wake, scope, layout, tz), loaded at boot by ferros_kernel `settings.rs`, written whole on every change; the raw one-byte `device.*` keys are gone. `layout`: 0 Dvorak (default), 1 QWERTY. The language waits on a VSF language type requested from the VSF session (ISO 639-3 packed base-26 + optional ISO 15924 / ISO 3166-1 numerics); defaults English; first list English, Spanish, French, Portuguese, Indonesian, German, Turkish (Oxanium covers them; Vietnamese not). Other vault values (map placement, photon roots, firmware) are not VSF yet: "all the things" is the target. Set from the host with `ferros-bridge diag "pref <name> <value>"`.
