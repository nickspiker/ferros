---
name: feedback-fgtw-devices-first
description: "FGTW is the person's devices and friends; fgtw.org is only the bootstrap. Device-to-device requests (departures, approvals) go to the fleet's devices first and notify there; never route them thru fgtw.org first"
metadata:
  node_type: memory
  type: feedback
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-10T15:04:18.355Z
---

FGTW is the network of the person's own devices and their friends' devices. fgtw.org (fgtw-bootstrap) only helps them find each other. A request one device makes of the others goes to those devices directly, as a message, and they notify the person and ask; fgtw.org is at most a last-resort mailbox for a device that cannot be reached.

**Why:** owner, 2026-11-10, after leaving the fleet from the 8a produced no notification anywhere and its screen talked about "fgtw.org answered" and devices "nearby": "not sure why it's consulting the fgtw bootstrap, should be contacting the actual FGTW first (my other devices and friends) since my devices are the ones that should be notifying and asking." He doesn't care whether devices are nearby or whether fgtw.org answered; those lines were debug leftovers on a user screen.

**How to apply:** design fleet flows (departure, add, lock) as peer messages over the FGTW peer network with a notification on every receiving device; LAN and fgtw.org are transports, not the design. Don't put reachability internals (seed answered, devices heard nearby) on user-facing screens. Test any such flow end to end on two devices before calling it done. Related: [[feedback-no-em-dashes]].

**Vocabulary (owner, 2026-11-10):** FGTW = the network of everyone's devices (leave Cloudflare out of it); fgtw.org = the bootstrap (the Cloudflare worker), only that; the `fgtw` process on a device = that device's FGTW node, its identity service (say "identity service" or "FGTW node" in prose, never bare "fgtw" for it). Changes go to FGTW (the person's devices) on change; the bootstrap maybe later, never first. Today the space sync pushes to siblings heard and to fgtw.org, and a reset device restores from fgtw.org's copy: to be turned devices-first.

**Join notices are local (owner, 2026-11-10):** "Not sure if I'm keen on getting join notifications. is that LAN? If so, fine. WAN, nope. Don't need everyone on the whole interwebs triggering a join toast." A join request raises a notice (toast, note, notification) only when the asking device is heard on this network or over Bluetooth / a tap; one known only from fgtw.org's registry shows nowhere unless the person opened "add a device" themselves.
