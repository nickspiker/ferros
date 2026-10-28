---
name: feedback_handle_not_password
description: A handle is NOT a password: never mask it (no dots, no show/hide); show it as typed, keep a wrong one on screen to correct; but MINTING a new identity asks for it twice (visible) since that happens once (owner 2026-10-27, ahau and Photon)
metadata:
  type: feedback
---

The owner rejected the attest screen's masked entry: "Oh no, this hiding of chars bit, nopety nope nope. this is NOT a password." (2026-10-27).

**Why:** a handle is a name the person proves they hold (TOKEN identity), not a secret typed against shoulder-surfers; the owner's threat model is harm, not observation ([[threat_model_harm_not_observation]]). Password rituals (dots, reveal toggles) treat it as something it is not. The owner then asked for one retype when MINTING an identity: "you only do that once" (a typo there is permanent), and the same in Photon.

**How to apply:** anywhere a handle is entered (ahau, Photon on ferros, any future screen): show the characters as typed; on a mismatch keep the text on screen to fix; minting a new identity asks for the handle a second time (visible, the first cleared so a typo is not copied), attesting an existing one is a single entry. Photon got the same request via photon-e0 on 2026-10-27. Handles still never go into committed files or printed logs ([[public_except_keys_and_handles]]): not masking on the person's own glass is a different thing from publishing.
