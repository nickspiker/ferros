---
name: fgtw_plain_http_for_ferros
description: fgtw.org answers plain HTTP (port 80, no redirect) — checked 2026-10-28; ferros (no TLS) reaches the seed that way via the ip server's TCP + DNS (payloads/httptest proves both); never enable Cloudflare "Always Use HTTPS" on the fgtw.org zone
metadata:
  type: reference
---

Checked from the dev box 2026-10-28: `GET http://fgtw.org/status` and `POST http://fgtw.org/` both return 200 over plain HTTP, no redirect (Cloudflare does not force HTTPS on the zone). Every seed call is one POST to `/` carrying a signed VSF document (fgtw-bootstrap src/lib.rs dispatches by section), so plain HTTP costs nothing in integrity: chains are folded and genesis re-verified on the client, bind requests carry device + identity signatures; a man in the middle can only withhold or delay (loud, never capture — docs/pairing-v2.md threat ledger). What crosses in the clear is public anyway (handle proofs, device keys, signed ops), and the owner's threat model is harm, not observation ([[threat_model_harm_not_observation]]).

**Why:** ferros has no TLS client; its network stack does DNS through the DHCP server and plain HTTP over the `ip` process's client TCP (payloads/httptest). The seed path for ferros's Photon client (chain fetch = the phonebook request, bindreq_post = the bind announcement, genesis, announce) rides on that.

**How to apply:** never turn on "Always Use HTTPS" (or an HTTP→HTTPS redirect rule) for fgtw.org without giving ferros TLS first; if ferros seed calls start failing with 301s, check that setting first.
