---
name: photon_on_ferros_no_bootstrap
description: Photon on ferros needs no FGTW bootstrap, no seed, no TCP/HTTP/TLS — peers come from physical pairing (phonebook handed over) + LAN broadcast; net stack = ARP/IPv4 + IPv6 (NDP/SLAAC) + UDP
metadata:
  type: project
---

Decided with the owner 2026-10-15: a ferros device running Photon does NOT use the fgtw.org bootstrap (challenge/announce, pb_put/pb_get over HTTPS) and needs no seed.

- **Peers:** enrolment into the owner's fleet is physical (LAN/USB from an existing device) and hands over the phonebook (signed records + last-seen addresses) into the vault; LAN broadcast discovery (Photon already does it) finds local peers; every later boot starts from the vault's last-known addresses, refreshed by peers' phonebook exchange.
- **Public address:** any peer's pong echoes it. Punching rides the ping cycle. All UDP 4383.
- **Trust:** addresses are hints only — peers prove identity by signed pongs/records against pinned keys, so a stale address costs a wasted ping.
- **So the ferros net stack is ARP + IPv4 + IPv6 + UDP** (no TCP, HTTP, TLS, DNS). IPv6 is a must ("def", owner): NDP + router advertisements/SLAAC, the firmware told to accept the NDP multicast groups; Photon binds dual-stack. Interface ID default: stable-privacy (RFC 7217 style, keyed from a device secret), not MAC-derived EUI-64 — owner may prefer per-session random. TLS never mattered for Photon (app-layer crypto: pinned FGTW keys, signed records, chain encryption); fgtw.org is a Cloudflare Worker (HTTP/WS only, plain HTTP works).
- **Gap on the Photon side:** FgtwMessage FindNode/Query/QueryResponse exist in photon/src/network/fgtw/protocol.rs but no peer answers them yet ("peers-are-fgtw"); a brand-new contact behind hostile NAT with no shared peer would still need a peer relay.
- If a cold-start seed is ever wanted: a DNS seed (seed.fgtw.org kept by the Worker, opted-in always-on peers), never an HTTP endpoint.

Related: [[ira_network_owns_assignment]], [[threat_model_harm_not_observation]].
- **Enrolment (owner, 2026-10-15):** the ferros device's Photon device key is derived from the ira; the owner's desktop Photon sponsors it into the fleet chain (Add op with ferros's consent signature, consent within 1 h → clock from peer pongs; PQ consent sigs if the fleet's floor is raised); pairing over USB/LAN from the host; the ferros phone holds the identity seed like any fleet device. Per-contact lane state (lane_root, era tag, conversation token) is provisioned at pairing and refreshed on era change. Chat ≤1 KB = one signed VSF datagram on UDP 4383 (no CLUTCH, no PT stream); body text needs vsf's `x` codec (ASCII Huffman subset planned).
