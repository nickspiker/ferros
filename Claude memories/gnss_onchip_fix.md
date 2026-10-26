---
name: gnss-onchip-fix
description: husky GNSS fix under ferros (2026-10-25) = the BCM4776's own engine via a replay of gpsd's on-chip session; how it was found (IgnoreOnChipRequest, batchOption 2), the fix-blob bit layout, the ~150 m disagreement with Android's host engine
metadata:
  type: project
---

ferros gets GPS fixes (2026-10-25 night) from the BCM4776's on-chip engine: `diag gnss boot [s]` replays the vault's `gnss.replay` (gpsd's on-chip session, cut from /mnt/Harbor/tmp/gnss-capture-2026-10-25/lhd4.trace) with the 87 81 / d0 81 time messages rewritten from the RTC; first fix 11 s, ±8 m; session kept in the service loop (`diag gnss fix`, `diag gnss off`). Details in GNSS.md "The on-chip engine".

Found by: gps.xml `IgnoreOnChipRequest="false"` (bind-mounted copy) + `$pglirm,req_breadcrumb,pvtOnChip,periodSec,1,batchOption,2` into glgpsctrl with no other location client (Maps closed). gpsd `Log="LOGCAT" LogPriMask="LOG_DEBUG"` gives a very detailed log — use it before reversing gpsd's disassembly.

Open: on-chip and Android-host fixes disagree by ~150 m N / 40 m E indoors (same under Android, so not a ferros decode bug); the replay depends on that capture's LTO/ephemerides/position aiding.

**Why:** the owner asked for GPS lock; the measurement-decode + own-PVT path was judged hours of RE, the on-chip path was minutes once gpsd's own log was on.
**How to apply:** never commit coordinates (truncate in output); the replay holds Google's patch and this place's position — vault/Harbor only. See [[gs-google-reference-tree]].
