---
name: clipboard-capability-handoff
description: "ferros clipboard = capability handoff brokered by the trusted input path, not a buffer; lazy typed VSF offers; paste/drag-drop one mechanism; fleet sync is a doorbell (who rang last), bytes fetched only on paste; name undecided"
metadata:
  node_type: memory
  type: project
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-11T03:15:42.380Z
---

Owner, 2026-11-10: "on ferros the clipboard shouldn't be a buffer at all. It should be a capability handoff, brokered by whoever owns the trusted input path (compositor/input service)." Also asked whether to call it a clipboard at all ("it's not a board that holds shit and it's a capability").

**The model (owner's):**
- Copy: the source mints a capability to its data and offers it with a list of typed representations (VSF types, never MIME strings).
- Paste: the input path sees the paste gesture on the focused surface and hands that surface a one-shot read capability. No gesture, no capability: background sniffing impossible by construction.
- Drag and drop is the same mechanism with a different gesture; build once.
- No primary selection (X11 middle-click): ambient state.
- Lazy: the destination picks a type, bytes produced only then via a read-only shared mapping. Small payloads (a few KB) snapshot at copy; large ones materialize on source exit or first paste, whichever first.
- Fleet sync: "a doorbell is a doorbell. I don't have to get the package but I would like to know who rang it last." Every device learns at once of the latest offer (source device, kist, types, sizes, time); bytes cross devices only on paste there.

**Raised by me, owner not yet answered:** copy should be gesture-gated too (focused surface only), else a process can swap what you paste; whether copied contents persist to disk or stay RAM-only/expire (passwords); a large offer is lost if its source device goes offline before any paste unless materialize-on-exit covers going offline.

**Name:** undecided. I suggested "kawe" (Māori: to carry, convey) or "handoff".

**How to apply:** design any copy/paste/drag/screencap-to-clipboard work on this model. Screencap lands as an offer ([[screencap-goes-to-clipboard]]). Related: [[feedback-no-polling]], [[feedback-fgtw-devices-first]].
