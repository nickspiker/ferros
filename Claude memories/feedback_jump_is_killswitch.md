---
name: feedback-jump-is-killswitch
description: "The jump from Android is a killswitch: Android is hostile once killed. Never wait for or adopt anything it started (in-flight UFS commands, its links, its DMA). Crowbar it: reset the block or cut its rails, then bring it up fresh"
metadata:
  type: feedback
---

On 2026-11-04 I suggested making the jump wait for Android's in-flight UFS command so ferros could adopt the link. The owner rejected it: "if it's mid command, hit it on the fucking head with a crowbar or kill the rails and reboot it. If I killed it, it's probably because it's being malicious."

**Why:** the person pulls the jump because they no longer trust Android. Anything Android set in motion may be the attack: a storage write, a DMA engine, controller state it configured. Letting it finish, or trusting state it left behind, hands the attacker the last word.

**How to apply:**
- At a jump, never wait for Android's work to complete, and never adopt a controller, link, queue or descriptor Android configured. Reset the block (or cut its rails) and initialise it fresh, the cold-boot way.
- Speed comes from making the crowbar fast, never from waiting.
- Bus masters Android left running must be stopped or fenced (S2MPU deny, rails off) before ferros's memory is at risk.
- The same applies to any future "switch away from a hostile OS" path, including the reverse jump.

Related: [[hv-jump-to-ferros]], [[feedback-killswitch-irq-mask]], [[ufs-firewall-decided]].

**Built and verified 2026-11-04 (1ee8369):**
- The hv sets the hsi1, hsi2 and misc S2MPUs to deny-all before copying ferros in.
- ferros cuts the unused power domains on its first pass after a jump.
- The UFS device's VCC is cut for 16 ms, then full init.
- First jump: all three deny-alls took, the GPU and EH were still on and got cut, UFS came up in 222 ms (it took about 16 s when Android's command was in flight).
- Not covered: the cpucl0 S2MPU, because what sits behind it is unknown.

**cpucl0 (2026-11-04):** this S2MPU guards the CoreSight debug/trace master, not the cores (shown with deny-all on a running cold ferros). It is closed on cold boots. Closing it at the jump froze the jumped ferros after vault open, every core silent and the kill chord with them. Most likely Android's trace (ETM to ETR) was still running on the reused cores and backed up into them once the sink was cut off. Switch tracing off on every core first (TRFCR_EL1/EL2, or the ETMs), then close it. This also showed that the volume-chord killswitch dies when every core stalls. The only backstops then are the hardware watchdog and the power long-press.
