---
name: fleet-add-needs-open
description: "Device add rules (owner 2026-11-10, -22, -24): confirm on the new device before any notification; the old device's Fleet page is passive until \"bind device\" is approved on the attest screen; then NFC (screen on), else BLE touch-select + Add; words only behind \"not in range\", never shown by default"
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
- The old device must be attested, hold the fleet key, and have the Fleet page open with the add flow opened by "bind device" (since 2026-11-24, below; until then ferros opened the add flow whenever the page showed: NFC reader, BLE scan, candidate list).
- A touch on a nearby row selects: `bind_select` is sent, the new device goes VSF green (0,1,0) "Selected by <name>", and the old device shows a green "Add <name>" plus "Didn't turn green? Press Back." Add binds and grows as one act. Back undoes nothing.
- NFC proximity (tag secret opens the request's commitment) binds and grows at once, with no Add.
- Typed words that match fully bind and grow at once.

**How to apply:** don't add passive listening by default. Use the word "touch" (screen) vs "NFC" (proximity), never "tap". The owner hasn't decided yet on a fleet-key check before the add flow opens (a keyless member could bind and then fail to grow). Related: [[feedback-fgtw-devices-first]], [[feedback-no-polling]].

**Added 2026-11-22 (owner, after a fleet notification reached his devices before he had confirmed on the new one):**
- No fleet notification reaches the person's devices until the new device's person has pressed the explicit confirmation ("Yes, I really would like to add this device and this is my identity and not a typo"). Nothing is announced on a typed handle alone.
- After that confirmation, the new device advises: put the two devices together (NFC), the other device's screen on and its Fleet page open, in Photon or ferros. Otherwise the click to add over BLE is the other option.
- The words are for odd cases only and never shown by default (shoulder surfing): behind a "Not in range? Show words to copy/paste or dictate" choice.

**Refined the same day (via the Photon session; Photon memory feedback_add_device_confirm_first.md):**
- ADD: the joiner types the handle once and taps attest, then waits; only when it sees the handle is taken and this device unbound does it ask "are you sure?"; only on yes does the request go out, as a persistent broadcast that "falls on deaf ears unless my fleet page is open on my other devices". No toast or light-up anywhere else ("don't want someone typing Nick in in Germany a bunch of times and making my device light up"). Then NFC, or BLE touch-select + green Add; words only behind "Not in range?".
- REMOVE, on the device being removed: press remove, type the handle, press unbind; the fleet is told at once (no Fleet page needed); the device then wipes its vault and keys (Photon on Android just closes; on ferros the killswitch).
- Both flows go into fgtw as one shared Join machine and departure; ferros's join (mint_request, the beacon) adopts it.

**Added 2026-11-24 (owner, routing fleet add through the approve on the attest screen):** "the fleet page does nothing unique until you press 'bind new device' or similar, then it does the scan/tap enable, all that."
- The button's words (owner, same day): "might not be new. just bind device is fine." So "bind device" (ferros_lang `BindDevice`), and the approve says "Approve: bind a device."
- The Fleet page is passive: no NFC read for an add, no BLE add scan, no list of devices asking, just because it shows.
- "bind device" asks for an approve (ahau_core::approve::FLEET_ADD): the device locks at once, the attest screen says what is approved, the handle typed there unlocks.
- The kernel holds the approval for the identity service. The map looks at it (SYS_UNLOCK op 20, looking only) and, once fgtw holds the identity as a member again (the lock wiped its add machine), tells fgtw op 8 [1] as any opening; fgtw opens the flow only by taking the approval from the kernel then (add.rs `add_page`), so one approve opens it once. fgtw closes the flow at every lock.
- Then NFC, touch-select + green Add, words behind "Not in range?" as above. It closes as before (Close or Done, Back, the page left); opening again is another approve.
- Built in forge (AUTH.md "Approve on the attest screen", "Bind device"); needs the kernel flash.
