---
name: feedback_handle_not_password
description: A handle is NOT a password: never mask it (no dots, no show/hide), never make the person retype it to confirm; show it as typed, keep a wrong one on screen to correct (owner 2026-10-27)
metadata:
  type: feedback
---

The owner rejected the attest screen's masked entry: "Oh no, this hiding of chars bit, nopety nope nope. this is NOT a password." (2026-10-27).

**Why:** a handle is a name the person proves they hold (TOKEN identity), not a secret typed against shoulder-surfers; the owner's threat model is harm, not observation ([[threat_model_harm_not_observation]]). Password rituals (dots, reveal toggles, type-it-twice) treat it as something it is not.

**How to apply:** anywhere a handle is entered (ahau, Photon on ferros, any future screen): show the characters as typed; on a mismatch keep the text on screen to fix; confirm a new identity Photon's way (enter once to see what becomes permanent, enter again), never by retyping. Handles still never go into committed files or printed logs ([[public_except_keys_and_handles]]): not masking on the person's own glass is a different thing from publishing.
