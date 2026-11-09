---
name: devbox_network_for_phone_tests
description: Phone→dev-box UDP tests: the box's wired NIC is a separate DMX network (use WiFi 192.168.1.156), its firewall drops unsolicited inbound UDP (receiver must send to the phone first), rejoin refusals after an unclean stop are the AP holding the old MFP association, not throttling, and the bridge must never be cut mid-transfer
metadata:
  type: project
---

Learned the hard way on 2026-10-15 night sending a beam-format test stream (not a beam) from husky to the dev box (BEAM.md, [[photon_on_ferros_no_bootstrap]]):

- **Wired `enp5s0` (192.168.1.100) is "dmx-eth", a separate DMX lighting network** on the same /24: the phone on WiFi cannot ARP it. Target the box's WiFi address, `wlp4s0` = 192.168.1.156.
- **The box's firewall drops unsolicited inbound UDP from the phone** (Photon's own 4383 works only because its peers' pings open the flow). A receiver must send a datagram to the phone from its listening port first, so the phone's stream arrives as replies on an established flow (`tools/photon-interop` `beam-recv … <phone address>`). Reading firewalld config needs root; `firewall-cmd` hangs on an auth prompt here.
- **Not throttling (owner, 2026-11-09: "The router is not throttling joins")**: refused rejoins after a driver was killed or the device rebooted are the access point still holding the old protected (MFP) association for that MAC; it refuses that address's authentication (event 3 status 2) for minutes. A fresh MAC joins at once (before attest pcie1 joins under a random one). pcie1 backs off 64 s doubling to 1024 s.
- **Never let a timeout cut the bridge mid-transfer** — ferros takes its PT-stall exit (PREV_EXIT G#B6) to fastboot. A 1.8 MB image stages in ~74 s, 730 KB in ~30 s; size timeouts accordingly. Bring-up, spawn and drain back to back.
- `pkill -f <pattern>` matches the shell running it; kill by `pgrep -x` PIDs.
