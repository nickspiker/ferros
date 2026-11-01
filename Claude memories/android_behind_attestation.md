---
name: android-behind-attestation
description: "Planned (owner, 2026-11-01): always boot ferros, Android launchable only after attest and back to ferros; needs a blunt \"not a phone\" disclaimer, OEM-unlock kept reachable; build after the OS swap"
metadata:
  node_type: memory
  type: project
  originSessionId: fef83e98-ba6e-4773-9615-f79c25605c30
  modified: 2026-11-01T21:29:22.590Z
---

Direction once the OS swap is sorted (owner, 2026-11-01): every boot lands in ferros; the gate offers no Android; Android opens only when ferros sets the gate's ANDROID_ONCE flag after a successful attest; "drop back to ferros" = any Android reboot (no flag → ferros). Stage 0 through a reboot (the hv's stage 2 must hide the flag page from Linux, or Android could set its own next boot); stage 1 without one = the forward/reverse swaps (blocked on Trusty FF-A teardown and the modem's reset; the reverse also needs the owner to lift the 2026-10-14 "handoff = register oracle only" rule).

**Required before it can be enabled: the disclaimer.** Off by default; turned on from a ferros setting after attest, text in full plus a typed confirmation (no tap, no timed hold). Draft:

> This turns your phone into something that is not a phone. From now on this device starts only into ferros. ferros cannot make calls or send texts, including emergency calls (911, 112, 999). Android, and with it calling and emergency calling, opens only after you attest with your handle. If you do not know your handle, you cannot call anyone — not with a full battery, not under a cell tower, not in an emergency. There is no bypass and no reset that brings calling back. Before you turn this on: be certain you know your handle; if you need to be able to call for help, carry something else that can. To undo it: attest and turn it off in ferros, or unlock the bootloader and reflash over USB (unlocking erases data).

(The one place "phone" is right — it names what is lost; see [[feedback-device-not-phone]].)

**Why:** owner: "if you're standing under a cell tower with a full battery and don't know that handle? you fucked" — the trade must be explicit, not discovered. Ownership: this husky is the owner's wife's phone ([[pixel8-dedicated-dev-device]], [[feedback-identity-belongs-to-a-person]]) — whose identity gates Android here is hers to answer too.

**How to apply:** recovery under the production lock — ferros repairs its own slots after attest (no unlock needed); unlock+reflash needs "OEM unlocking allowed", whose Android switch would be behind the gate, so ferros keeps that bit reachable; Tensor's boot-ROM USB recovery (18d1:4f00) restores only Google-signed boot stages — no firehose on Pixel. Check before relying on unlock+reflash whether the ferros partition (the vault) survives `fastboot flashing unlock`'s wipe; if not, the disclaimer says so. Related: [[feedback-lock-prep-not-lock]], [[mvno-data-only]], [[ufs-firewall-decided]].
