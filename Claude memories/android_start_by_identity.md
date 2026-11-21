---
name: android-start-by-identity
description: "Owner 2026-11-21 - Android keeps its own Google identity (no ira/wairua under Android); Android starts only when the attested identity chose it, by a ferros-controlled reboot; a \"start into\" setting is that choice"
metadata:
  node_type: memory
  type: project
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-21T20:53:55.349Z
---

Owner, 2026-11-21, asked whether the hv should hold the ira and wairua under Android so both OSes share one machine identity: "Since the long term goal is to not Android, maybe we keep the plate and papers since it would match if ferros was not installed. Not opposed to triggering a reboot to enter Android as long as ferros controls it, i.e. Android cannot be started unless the attested identity chose to start it."

**Decided:**
- Android keeps Google's own device identity and attestation (the "plate and papers"), exactly as it would look without ferros installed. No ira or wairua under Android; the hv holds no keys for it.
- Android starts only by ferros's hand: a reboot into Android that ferros triggers (ANDROID_ONCE), and only when the attested identity chose it. No gate key, bootloader path or Android reboot may start Android on its own.
- A default-boot setting ("start into ferros / Android") counts as the identity's standing choice: ferros boots, attests, then reboots into Android. Asked by the owner the same day: "a setting in the ferros settings page to set the default boot path, would be nice if you wanted to stay on Android for a while".

**Still binding from 2026-11-01** ([[android_behind_attestation]]): turning on the Android-only-by-identity mode for a person needs the disclaimer and typed confirmation first (it costs emergency calling without the handle).

**How to apply:** remove every path that starts Android without the attested identity (the gate's volume-down choice, `diag android` without a present identity, the 8a's separate slot a, which reboots into itself). The hv must hide the ANDROID_ONCE flag from Linux so Android cannot set its own next boot.

Related: [[ferros_attest_os_level]], [[hv_chords_decided]], [[ufs_firewall_decided]].

**The dev gate (owner, same day):** "the 1s boot select override only exists on dev builds, which should stay if possible, but then of course be set to Android, if the user chose, or ferros. When we do a production build, that screen will go away completely and be flagged out."
- Development builds keep the gate's 1 s boot select and its override (volume down Android, volume up hold); its auto-press default follows the person's "Start into" setting.
- Production builds have no gate screen at all (compiled out): straight to ferros, attest, then Android only by the identity's choice.
