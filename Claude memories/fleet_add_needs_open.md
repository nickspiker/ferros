---
name: fleet-add-needs-open
description: "Device add rules (owner 2026-11-10): Fleet page open on the old device; NFC proximity adds at once; a touch on a nearby row only selects (green) and Add binds+grows; typed words add at once; passive listening only if proven spam-proof"
metadata:
  node_type: memory
  type: project
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-10T22:24:49.193Z
---

Owner, 2026-11-10, after a Bluetooth pick 50 ft away stayed bound when Back was pressed, and after "tap" turned out to mean two things:
- "if you tap, it shouldn't become part of the fleet, it should just get notified it was selected", "I need a distinct green add, and note to press back otherwise".
- "Touch on screen (tap was a bad word to use in hindsight) required for BLE OR no touch on screen, just physical proximity (NFC). Both of which need the fleet page open."
- On listening without the page: "I think it needs to be open unless there's a way to do it passive and secure, i.e. not allowing someone to spam the bootstrap or my devices."

**The rules (ferros 6b4d4b0, 0606c09; Photon feb786b5, fgtw 328a4bf `bind_select`):**
- The old device must be attested, hold the fleet key, and have the Fleet page open (ferros opens the add flow whenever the page shows: NFC reader, BLE scan, candidate list).
- A touch on a nearby row selects: `bind_select` is sent, the new device goes VSF green (0,1,0) "Selected by <name>", and the old device shows a green "Add <name>" plus "Didn't turn green? Press Back." Add binds and grows as one act. Back undoes nothing.
- NFC proximity (tag secret opens the request's commitment) binds and grows at once, with no Add.
- Typed words that match fully bind and grow at once.

**How to apply:** don't add passive listening by default. Use the word "touch" (screen) vs "NFC" (proximity), never "tap". The owner hasn't decided yet on a fleet-key check before the add flow opens (a keyless member could bind and then fail to grow). Related: [[feedback-fgtw-devices-first]], [[feedback-no-polling]].
