---
name: ideas-language-independent
description: An idea = a meaning with a 32-byte id (blake3 of locale + first phrasing as VSF x); phrasings in any language added over time; ferros UI Msgs and kist reasons/names are ideas; ideas directory beside kists; decided 2026-11-10
metadata:
  node_type: memory
  type: project
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-11T04:36:41.348Z
---

Owner, 2026-11-10: "We need a generic idea bubble that we can expand and add languages as the translation/need fits. Like think 'Remove from fleet' is an idea." Then: "ideas of course are pinned by their locale hashed with their first literation in whatever that locale is, VSF proper x style and that produces a 32 byte unique 'idea'" ("Sounds excellenter").

**Decided:**
- An idea is a meaning; its id is 32 bytes, blake3 over the locale and the first phrasing encoded as a VSF `x` value. Phrasings in other languages attach later and never change the id.
- ferros's own UI strings (ferros_lang `Msg`) become ideas; a new language is phrasings, not code.
- Kist text (a right's reason, names, descriptions) references ideas; a capsule carries the ideas it uses with its developer's phrasings; an ideas directory sits beside the kist directory (no account, no gatekeeper, signed).
- Shown in the person's language if a phrasing exists, else the idea's first language marked as such; never blank, never guessed.

**Decided 2026-11-10 (owner: "Yup", after "we need to scrub it, not sure about colissions either"):**
- Scrub before hashing: NFC, collapse whitespace to one space, trim, drop zero-width and control characters, unify look-alike spaces and dashes; never case-fold; keep punctuation.
- Collisions of meaning (not hashes): the same meaning minted twice (e.g. Korean with Chinese phrasings, later English) is joined by a signed "same as" link; a device follows links signed by keys the person trusts; neither id is rewritten; the older id is shown first. "Related" links (close, not the same) only help search, never change display.
- Minting searches existing ideas' phrasings in every language first ("is it one of these?").
- An ambiguous phrasing ("Close") is the minter's to fix with a second idea, not machinery.

**Proposed by me, owner not yet answered:** blake3 keyed/derive mode with a context ("ferros idea") for domain separation; NFC + trim before hashing, never case-fold; the locale as ferros_lang's code in ASCII; same words = same idea (signature on a phrasing is separate from the id); which phrasing wins in one language (minter's, then keys the person trusts, then most used); an idea reference as its own VSF type vs `hb` (owner decision, Rule -1).

**Coupling:** idea ids (like handles via ihi) are tied to the x codebook (hash 51b6e8eb…): the codebook must never change again.

**Decided 2026-11-10 (owner: "The VSF literally needs an idea type, pretty sure. your call there. My dev key is fine. anyone, yes."):**
- VSF type `hi` (vsf 1b87e15, v11, unpublished): 32 bytes, in the hash family (`x` + a letter would collide with EWE's markers for integers wider than 128 bits), `TypeConstraint::Idea`, shown by inspect as hi.
- ferros's own phrasings are signed by the owner's developer key until the project key exists.
- The ideas directory accepts writes from anyone (devices show only phrasings from keys the person trusts).

**Models later, not now (owner, 2026-11-10: "I think the pick an idea, pin it and add some languages is fine. we can version in the actual ideas later"):** discussed a model fingerprint per idea (SONAR-style encoder/decoder, Matryoshka sizes, 256-bit prefix for Hamming search, a generative model for verbose phrasings, phrasings as anchors so a new model re-encodes). Deferred: build pinned ids + phrasings first; a model is at most a labelled contributor of phrasings and "same as" suggestions, never the identity. A phrasing may later carry a register (terse/plain/verbose) beside its locale.

Rejected: a fixed per-right purpose vocabulary ferros translates (owner: "I don't like it").

Related: [[kist-catalog]], [[permissions-asked-once]], [[feedback-check-published-before-bump]].
