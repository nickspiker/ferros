---
name: photon_phone_join_status
description: "2026-10-26 night: the ferros phone's Photon client runs (payloads/photon), paired with the owner's roots, hands its binding request to the desktop over PT; the bind itself needs the owner at the desktop (Settings → Fleet → Add device, type the words). Traps found: AP passes no multicast between WiFi clients, the dev box firewall drops unsolicited UDP, the dev box's subnet broadcast leaves on the wired DMX NIC"
metadata:
  node_type: memory
  type: project
  originSessionId: b668c420-897f-4700-9fe3-8f3c4472a1c2
  modified: 2026-10-27T03:50:06.156Z
---

Where the phone-joins-the-fleet work stands (PHOTON.md has the full picture):
- Phone: `payloads/photon` spawned with `ferros-bridge spawn photon.bin [hex sponsor addr] --name photon --use udp --store photon --ira`; device key ira-rooted (`SYS_IRA`, 677 ms to derive the three key families), roots in vault scope `photon` (pushed from this box with `tools/photon-interop/target/debug/roots <dir>` + `store put`), prints its 23 words, beacons, hears fleet devices, hands the 10.7 KB `bind_req` over PT. Desktop runs today's Photon (dev.sh installed) with `bind_req`/`fleet_resp`/`fanout_resp` + the joiner-beacon answer-back.
- Remaining human step: the owner opens Settings → Fleet → Add device on the desktop and types the phone's words (console line "photon: pairing words - …") or taps the nearby candidate; the chain and fan-out then flow back and the phone logs BOUND / THE FLEET KEY IS OURS.
- Network traps on this LAN: the AP passes no IPv4 multicast between two WiFi clients (the phone hears wired devices' beacons, not the dev box's); the dev box's firewall drops unsolicited inbound UDP (phone→desktop frames pass only after the desktop sends to the phone's 4383 first — hence the unicast beacon answer-back from the main socket); `ip route get 192.168.1.255` goes out the wired DMX NIC, so the desktop's subnet broadcast never reaches WiFi. The sponsor-address spawn input is the dev workaround.
- Kernel: arc pool moved to 16 MiB at G#8300_0000 (8 MiB was full with four guarded 1 MiB arcs); flashed to slot b 2026-10-26 20:40.
- Open: an app heap region (the client's heap is 160 KiB in .data to fit a 1 MiB arc); words on the phone screen; the client started by init once bound; notes-to-self as the first photon; Photon's attest off the seed.

**How to apply:** when the owner is at the desktop, walk him through the Add-device step and read the phone console for the bound/key lines; don't re-derive the roots (they are in keys/photon-roots and in the phone's vault).
