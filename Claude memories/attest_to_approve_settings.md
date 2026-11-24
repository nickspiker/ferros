---
name: attest-to-approve-settings
description: Owner 2026-11-24 - protective settings are approved on the real attest screen (lock at once, Approve/Decline, fail = stay locked); any debatable setting can carry a "require attest" guard the owner toggles
metadata:
  type: project
---

Owner, 2026-11-24, after the unlock-at-boot confirm screen took Enter differently from the attest screen: asking for such a setting "should immediately lock the device anyways and drop to the attest screen, pending success to apply the setting and the button instead of attest ... should say 'approve' or similar and keep the user locked out if fail." Then: "for any debatable settings you could add a toggle that requires attest, and to untoggle, of course requires an attest but then after that your kid can change it all day long until you toggle 'require attest' on again."

**The design:**
- **Protective settings** (unlock at boot, fleet add/remove, Android on, identity export, and the like) are approved on ahau itself, never a copy: the request locks the device at once; ahau shows "Approve: <what>" with Approve (and Decline below it, which goes to the plain attest screen); success unlocks and applies; failure or decline stays locked, nothing applied. One screen, one input path (the Enter-key mismatch can't recur).
- **Debatable settings** each get a "require attest" guard. Turning a guard off needs an attest; while off, anyone holding the device may change that setting freely; turning it back on restores the guard.
- Ordinary settings (the battery cap, appearance) need nothing.

**How to apply:** never build a second handle-entry screen; route every approval through ahau's approve mode. Related: [[ferros-attest-is-os-level]], [[identity-not-on-disk-default]], [[feedback-handle-not-password]].

**One device, one identity (owner, 2026-11-24):** "Nope. 1 device, 1 identity. My wife wants to use it? I need to unbind first, then she needs to bind it to her fleet." A bound device refuses every other identity; no guests. Attest screen lines: unbound "This device is unbound. Present an identity to bind it to."; bound "This device is bound. Present its identity to unlock it." (never whose). The button is "bind device"; leaving is "unbind".

**What runs through a lock (owner, 2026-11-24):** sound, microphone and camera already held by a running kist keep going when the device locks ("I'd keep it going. we always assume it's not malicious."); GPS claims drop at the lock (as built). Touches never reach kists or the deck while nobody is present (audit fixes 00b8b286, 020b473a).
