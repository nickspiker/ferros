# Pixel 8 Pro calibration data

Spectral characterisation of the Pixel 8 Pro's four camera modules and its display, measured 2026-10-21/22. Treated as **model tier** for every Pixel 8 Pro (VSF tiers: `unit` | `model` | `assumed`): the primaries and spectral responses are assumed uniform across units. **Caveat: a single-unit batch (n = 1).** Everything here was measured on one phone; it stands for the model until more units are measured. A unit calibrated individually goes back to `unit` tier. The derived IDT files themselves still carry tier `unit`, as recorded; relabel to `model` when they are next re-encoded.

- `derived/` — per module (cam2 main, cam3 ultrawide, cam4 tele, cam7 front): a VSF IDT with an Absolute entry (counts per SI radiance) and a Relative entry (reflectance), and the per-channel spectral responses, 268–1084 nm in 4 nm steps.
- `stellarnet/` — StellarNet spectra of the reference puck (UV, VIS, both) at several integration times, with the spectrometer's own dark readings (`dark_*.csv`, the instrument's, not a camera's); `scan-order.png` is the measurement order.
- `sekonic/` — the Sekonic illuminance / CCT reading of the source.

Not here, on purpose: the raw averaged camera frames the IDTs were solved from (`*average.vsf`, ignored by `.gitignore`). They carry this unit's sensor pixel-response pattern, a per-device fingerprint, and capture-location fields. No phone serial is in any file here.

The display's spectra live with the display oracle: `oracle/display/husky-hk3/profile-20261022/spotread.log` (227 i1 Pro readings, each 380–730 nm at 10 nm, absolute radiance) and `calib-20261017-1018.csv`. See DISPLAY-CALIB.md.
