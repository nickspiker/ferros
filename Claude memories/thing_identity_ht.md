---
name: thing-identity-ht
description: Owner 2026-11-20 - every referable thing has a stable thing identity, VSF marker `ht` (32 bytes), beside `hp` (a version's content) and `hi` (its kind); fixed-size Rust newtypes per h-marker; the deck keyed by (kind, Ht) with icons by Hp
metadata:
  type: project
---

Decided 2026-11-20 (owner: "ht?", then "Yessirrrr!!!" to the plan).

**The two identities:**
- **`hp`, content identity:** these exact bytes. An icon, a photo, one build of a kist, one version of a document. Use it for caches and dedup.
- **`ht`, thing identity (new VSF marker, 32 bytes):** what stays the same while the content changes. A kist across updates, a contact across avatar changes, a hold item across edits.
- **A reference** is (kind as `hi`, an idea; the thing as `ht`), optionally with the current content as `hp`.

**Where each `ht` comes from:**
- **Kists and pucks:** derived from the developer's signing key plus the option, so it's stable across updates and nobody else can claim it.
- **Contacts:** derived from the person's identity.
- **Hold items:** minted when first kept.
- Each kind uses its own BLAKE3 context, so kinds never collide.

**In Rust:** fixed-size newtypes in vsf for every `h` marker (`Ht`, `Hp`, `Hi`, ...), `#[repr(transparent)]` over `[u8; 32]`, each converting only to and from its own VsfType variant. Never a bare `[u8; 32]` for an identity.

**The deck:**
- a proper `Circle { id: (kind, Ht), at, r, turn, icon: Option<Hp>, arranged }` and `Deck`;
- one deck document keyed by `ht`, replacing the per-name `map.place.<label>` keys, which broke on renames and collided at 26 bytes;
- the display's icon cache keyed by `Hp`, decoded once, so no baked numbered icon set, which caused the swapped settings and Kūaha icons and the display growing past the 3 MiB value limit.

Related: [[ideas_language_independent]], [[ferros_vocabulary]] (Kūaha, hold items), [[app_scale_theme_global]].
