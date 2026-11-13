---
name: notifications-in-view
description: "Notifications decided 2026-11-12: in view can notify, out of view is silent (the deck's resting view is the reach); three kinds live/personal/status; no sliders, no permission lists"
metadata:
  type: project
---

Owner, 2026-11-12, after rejecting sliders and permission lists ("we have a brand new UI so we have an opportunity to do notification proper"): "I think we try it with the in view, it can notify me, out of view, silent. Screen's big enough, we'll see."

**The design:**
- **Reach = the view.** A source (kist, person, puck) whose circle is in the deck's resting view can notify; out of view it is silent (it may still glow where it sits, seen when you look there). The resting view is where the person last settled the deck (also what sits behind an open kist); a pan that doesn't settle doesn't change reach. Focus = zoom in on the circles you want; nothing reaches you = zoom into empty space. This replaces do-not-disturb toggles and per-app settings.
- **Three kinds, set by what the thing is, never chosen by the app:** live (a beam, an alarm: rings, takes the screen), personal (a person wrote: one buzz or sound + glow), status (updates, downloads: glow only, never sound).
- **The glow** is the "for you" 400 nm state with its count (ferros_glow); a kist in front shows waiting things on the Deck corner's wedge, nothing covers it.
- **Always through:** alarms the person set and safety (battery dying).
- Notes clear when seen on any of the person's devices (fleet doorbell, "who rang last"), never on a timer; a kist may set one deadline on a note (like an alarm).
- Open: whether a stranger's beam rings; whether updates stow themselves silently.

Related: [[clipboard-capability-handoff]], [[ferros_vocabulary]], [[feedback-no-polling]].
