---
name: cellular-ttl-63
description: "TTL and IPv6 hop limit 63 for everything ferros originates, both families, WiFi and cellular (owner 2026-10-19, widened to all 2026-10-31); one constant HOP in payloads/ip; the husky Android kernel is built with the same 63"
metadata:
  type: project
---

Everything the ferros IP stack originates goes out with IPv4 TTL and IPv6 hop limit **63**, not the usual 64. This covers both families on both links (WiFi and cellular). The owner set it for cellular IPv4 on 2026-10-19 ("we may set it back at some point but not today"), then widened it to all traffic on 2026-10-31.

The value lives in one constant: `HOP` in payloads/ip/src/main.rs. Exceptions are what a protocol fixes:
- neighbour discovery: 255 (RFC 4861);
- IGMP and MLD reports: 1.

The husky Android kernel is built with the same 63 in /mnt/Harbor/husky-kernel/aosp:
- `IPDEFTTL` in include/uapi/linux/ip.h;
- `IPV6_DEFAULT_HOPLIMIT` in include/net/ipv6.h;
- ndisc.c ignores a router advertisement's hop-limit override.

That's compiled in, so it survives wipes.

**Why:** owner's call; the reason was not given and does not need to be asked.

**How to apply:** new sending paths use `HOP`, never a literal. To revert, change that one line, and the kernel defines for Android.
