---
name: ira_is_brand_not_credential
description: "The ira is the fleet BRAND (public, permanent), not a credential; theft/reset/enumeration are defended by the fleet-key wrap design + lock/shrink, NOT by ira secrecy"
metadata: 
  node_type: memory
  type: project
  originSessionId: 35f77793-aafd-4fba-bd97-4cd4e60013b9
---

Settled 2026-08-20 while reconciling the ferros ira work with photon/docs/fleet-key.md.
The ira is a permanent, PUBLIC brand derived deterministically from the machine — it authenticates nothing by itself.
Guessing an ira only yields a name that is already public.

**Why:** the credential is opening a fleet-key WRAP, which needs two factors the ira alone does not supply — the device ira SECRET (ECDH half) AND the handle-derived identity seed (which exists nowhere at rest).
An identity-seed-only KEK is explicitly FORBIDDEN, so enumerating handles gets you nothing without the target device's actual keypair secret.
Theft is handled by the KEY moving, not by the brand being secret: lock -> SHRINK mints a fresh key, re-wraps to every unlocked member EXCEPT the stolen one, re-seals state; the locked ira stays in the chain forever as testimony (ostracism, not erasure) so it can never launder itself as a fresh device.
The handle is SECRET (a thief does not have it, see [[public_except_keys_and_handles]]) and the fleet is a PHYSICAL local mesh (a remote thief cannot reach the Redmond macbook to sync from it) — the France-thief attack fails on both.

**How to apply:** stop treating ira bit-count as a confidentiality property. The three panics (theft = auth, factory-reset -> type handle -> in, enumerate-into-the-fleet) all dissolve once you see the ira as a brand and the wrap-open as the credential. See [[ira_entropy_is_unforgeability]] for the one property the ira DOES owe, and [[ira_network_owns_assignment]] and [[pipe_ira_is_meaningless_alone]] which said this all along.
