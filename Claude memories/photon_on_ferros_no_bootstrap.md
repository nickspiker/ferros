---
name: photon_on_ferros_no_bootstrap
description: Photon on ferros needs no FGTW bootstrap, no seed, no TCP/HTTP/TLS — peers come from physical pairing (phonebook handed over) + LAN broadcast; net stack = ARP/IPv4/UDP only
metadata:
  type: project
---

Decided with the owner 2026-10-15: a ferros device running Photon does NOT use the fgtw.org bootstrap (challenge/announce, pb_put/pb_get over HTTPS) and needs no seed.

- **Peers:** enrolment into the owner's fleet is physical (LAN/USB from an existing device) and hands over the phonebook (signed records + last-seen addresses) into the vault; LAN broadcast discovery (Photon already does it) finds local peers; every later boot starts from the vault's last-known addresses, refreshed by peers' phonebook exchange.
- **Public address:** any peer's pong echoes it. Punching rides the ping cycle. All UDP 4383.
- **Trust:** addresses are hints only — peers prove identity by signed pongs/records against pinned keys, so a stale address costs a wasted ping.
- **So the ferros net stack is ARP + IPv4 + UDP** (no TCP, HTTP, TLS, DNS). TLS never mattered for Photon (app-layer crypto: pinned FGTW keys, signed records, chain encryption); fgtw.org is a Cloudflare Worker (HTTP/WS only, plain HTTP works).
- **Gap on the Photon side:** FgtwMessage FindNode/Query/QueryResponse exist in photon/src/network/fgtw/protocol.rs but no peer answers them yet ("peers-are-fgtw"); a brand-new contact behind hostile NAT with no shared peer would still need a peer relay.
- If a cold-start seed is ever wanted: a DNS seed (seed.fgtw.org kept by the Worker, opted-in always-on peers), never an HTTP endpoint.

Related: [[ira_network_owns_assignment]], [[threat_model_harm_not_observation]].
