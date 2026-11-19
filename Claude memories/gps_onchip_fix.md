---
name: gps-onchip-fix
description: husky GPS fix under ferros (2026-10-25) = the BCM4776's own engine via a replay of gpsd's on-chip session; now the `gps` driver process (payloads/gps, docs/GPS.md); how it was found, the ~150 m disagreement with Android's host engine
metadata:
  type: project
---

ferros gets GPS fixes (first 2026-10-25 night) from the BCM4776's on-chip engine: `diag gps boot [s]` (now the `gps` driver process, payloads/gps) replays the vault's `gps.replay` (gpsd's on-chip session, cut from /mnt/Harbor/tmp/gnss-capture-2026-10-25/lhd4.trace) with the time messages rewritten from the RTC; first fix 11 s, ±8 m; `diag gps fix`, `diag gps off`. Details in docs/GPS.md "The on-chip engine". The capture's position messages are zeroed in the vault copy (AUTH.md); GPS never runs before a person attests ([[radios_at_attest_gps_locked]]).

Found by: gps.xml `IgnoreOnChipRequest="false"` (bind-mounted copy) + `$pglirm,req_breadcrumb,pvtOnChip,periodSec,1,batchOption,2` into glgpsctrl with no other location client (Maps closed). gpsd `Log="LOGCAT" LogPriMask="LOG_DEBUG"` gives a very detailed log: use it before reversing gpsd's disassembly.

Open: on-chip and Android-host fixes disagree by ~150 m N / 40 m E indoors (same under Android, so not a ferros decode bug); the replay depends on that capture's LTO/ephemerides aiding. akita's GPS is a different (Samsung KEPLER) chip ([[akita_pixel8a]]).

**2026-11-18, no fix on the Pro:** the vault's replays are not the ones GPS.md describes. `gps.replay` (554 records) is a host-mode session (no `87 81`, no `d0 81`, no on-chip setup): the chip streams `02 80` measurements ~100 s, then goes silent; the kernel's restart uses it (no input). `gps.replay.sky` (509) is on-chip but has no `d0 81`, so no place reaches the chip (nothing in 9 min indoors). The lhd captures on /mnt/Harbor/tmp are gone. Check a replay with `tools/gps-replay-scan` before trusting it; GPS.md "No fix, 2026-11-18" and the proposals after it.

**2026-11-18 night (gps-rough-fix, GPS.md "Built 2026-11-18 night"):** the host frame's byte for flag G#02 is a reliable counter (+1 per reliable frame), and the chip refuses (flags G#29) a frame whose counter is not last + 1, and every frame after it. Cuts that closed up the sequences left counter jumps, so the sky replay's last four messages were refused at every start; the driver now renumbers it. It also puts a `d0 81` back after `87 81` (acked), predicts every GNSS bird from CelesTrak elements (`gps.orbits`, `ferros_gnss`) as tracks with C/N0 0, and starts on the sky replay without input. Any new replay surgery: renumber sequence, trailer AND this counter, and check `gps-replay-scan`'s "reliable counter" lines. Still open: which d0 81 field is the uncertainty, and a first fix with the sky in view.

**Why:** the owner asked for GPS lock; the measurement-decode + own-PVT path was judged hours of RE, the on-chip path was minutes once gpsd's own log was on.
**How to apply:** never commit coordinates (truncate in output); the replay holds Google's patch and this place's position: vault/Harbor only. See [[gs-google-reference-tree]].
