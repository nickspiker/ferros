---
name: feedback-device-not-phone
description: "Say \"device\", never \"phone\", for what ferros runs on (owner, 2026-11-01) — in docs, comments, UI text and replies"
metadata:
  node_type: memory
  type: feedback
  originSessionId: fef83e98-ba6e-4773-9615-f79c25605c30
  modified: 2026-11-01T14:51:29.498Z
---

Say **device** for what ferros runs on, never "phone": in docs, code comments, UI text, commit messages and replies to the owner.

**Why:** ferros runs on devices, and eventually on the glyph, which is not a phone: it places no calls (the MVNO is data-only; no voice, SMS or numbers). The owner reworded AUTH.md by hand on 2026-11-01 after a night of "phone" throughout; [[mvno-data-only]] is the same stance.

**How to apply:**
- "the device", "this device", "the device-only secret" (AUTH.md's term now).
- Name the hardware model where the model matters ("husky", "the Pixel 8 Pro").
- Sweep your own new text for "phone" before committing.
