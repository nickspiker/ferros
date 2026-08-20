---
name: vault-ledger-split
description: ferros_ledger renamed to ferros_vault (object store), new ferros_ledger is the append-only event chain
type: project
---

Naming decision (2026-03-13): two distinct subsystems, two crates.

- **ferros_vault** — persistent object store. Flat, hash-indexed, content-addressed, mesh-replicated. Anchors, devices, commits, mesh consensus. Where bytes live.
- **ferros_ledger** — append-only event chain. BLAKE3-chained, categorized, capability-gated. Ordered record of what happened. VSF documents stored in the vault (eventually).

**Why:** Both were called "ledger" causing naming collision. "Ledger" fits the event chain (sequential record of transactions). "Vault" fits the object store (capability-gated, secured).

**Ledger API design:** `ledger.post(event)` — developer imports ledger, posts events. Ledger daemon runs kernel-side, issues caps per category. Handles chain maintenance, hashing, cap validation internally. Each subsystem gets a handle with its category cap.

**How to apply:** When referencing persistent storage/mesh/anchors, use "vault". When referencing event logging/chain/categories, use "ledger".
