# Ideas

Status: decided in outline by the owner 2026-11-10; the first cut built the same day (`ferros_idea`, no_std + alloc, 24 tests), not yet used by anything.
This is the spec for the owner to mark up: each part says **Decided** (with the owner's words) or **Proposed** (mine, not yet answered).

The owner: "We need a generic idea bubble that we can expand and add languages as the translation/need fits. Like think 'Remove from fleet' is an idea."
An idea is a meaning.
Its words in any language are phrasings of it, added as the need comes; the meaning keeps its name thru all of them.

## What an idea is

- **Decided:** an idea is named by 32 bytes, pinned by its first phrasing: "ideas of course are pinned by their locale hashed with their first literation in whatever that locale is, VSF proper x style and that produces a 32 byte unique 'idea'" ("Sounds excellenter").
- **Decided:** the name is its own VSF type, `hi` (vsf 1b87e15, format v11, not yet published): "The VSF literally needs an idea type, pretty sure. your call there." 32 bytes, in the hash family (an `x` plus a letter would collide with EWE's markers for integers wider than 128 bits), `TypeConstraint::Idea`, shown by inspect as `hi`.
- **Decided:** phrasings in other languages attach later and never change the id; ferros's own words (ferros_lang's `Msg`) become ideas, so a new language is phrasings, not code; a kist's text references ideas.
- **Decided:** same words in the same language are the same idea, whoever mints them (a signature is on a phrasing, never in the id).

## The id

**Decided** in shape (keyed BLAKE3, locale and first phrasing, `x`); the exact bytes below are **Proposed** and built (`ferros_idea::id`).

```
id = BLAKE3.derive_key(context = "ferros idea",
                       material = flatten(a(locale)) ‖ flatten(x(scrub(text))))
```

- **The context** `ferros idea`: BLAKE3's derive-key mode, so an idea's 32 bytes never equal any other hash over the same bytes (domain separation, as fgtw's `PHOTON_KEY_BUNDLE_v1`).
- **The locale**: the ISO 639-3 code, three lowercase ASCII letters, as fgtw's `Lang::iso` gives ferros_lang's sixteen (`eng`, `spa`, `mri`, … `fil`) and as vsf's `wl` keeps any language. Hashed as the VSF `a` value of those letters: `a`, its size marker, its length, the letters (`61 33 03 65 6e 67` for `eng`). The length makes the material self-delimiting.
- **The words**: the scrubbed first phrasing ([Scrubbing](#scrubbing)) as the VSF `x` value: `x`, the NFC character count (EWE), then the Huffman bytes of vsf's codebook. "Remove from fleet" is `78 33 11` then 12 bytes `03 30 f6 dd 62 85 d6 1c a1 d1 9a 80`.
- **Worked example** (pinned in `the_id_is_pinned`): `id(eng, "Remove from fleet")` = G#4a7321a0 4a0fd95d ee18a342 07710b4d 0a987548 ebdbacc8 cd592129 6f624af2.
- **The codebook coupling**: the `x` bytes are the codebook's, `huffman_codes.bin`, pinned in vsf's build.rs by BLAKE3 51b6e8eb748c56c4991ce8fe556422ec95f2bcfff78927a7778b1a892d5806f9. Every idea id, like every handle thru ihi, depends on it: after v11 is published the codebook must never change again, or every idea is renamed. vsf's no_std `x-full` writes the same bytes as its std encoder for every Unicode scalar value, so the device mints and checks ids in any script.
- **Errors**: a locale that is not three lowercase letters; words that scrub to nothing.

## Scrubbing

**Decided** (owner: "Yup", after "we need to scrub it, not sure about colissions either"): NFC, whitespace runs collapsed to one space, trimmed, zero-width and control characters dropped, look-alike spaces and dashes unified; never case-folded; punctuation kept.
The exact lists are **Proposed** and built (`ferros_idea::scrub`); once one idea is published they are frozen, since changing one renames ideas.

Each character, in order:
- **Line break → a line break** (a run of whitespace holding one becomes one `\n`): LF, VT, FF, CR, NEL (U+0085), LINE SEPARATOR (U+2028), PARAGRAPH SEPARATOR (U+2029).
- **Space → a space** (a run becomes one U+0020): TAB, SPACE, NO-BREAK SPACE (U+00A0), OGHAM SPACE MARK (U+1680), EN QUAD to HAIR SPACE (U+2000 to U+200A), NARROW NO-BREAK SPACE (U+202F), MEDIUM MATHEMATICAL SPACE (U+205F), IDEOGRAPHIC SPACE (U+3000).
- **Dropped** (invisible): SOFT HYPHEN (U+00AD), COMBINING GRAPHEME JOINER (U+034F), ARABIC LETTER MARK (U+061C), MONGOLIAN VOWEL SEPARATOR (U+180E), ZERO WIDTH SPACE (U+200B), LRM and RLM (U+200E, U+200F), the bidi embeddings and overrides (U+202A to U+202E), WORD JOINER and the invisible operators (U+2060 to U+2064), the bidi isolates and the deprecated format characters (U+2066 to U+206F), ZERO WIDTH NO-BREAK SPACE (BOM, U+FEFF), the interlinear annotation marks (U+FFF9 to U+FFFB), and every other control (C0, DEL, C1).
- **Dash → U+002D**: HYPHEN to HORIZONTAL BAR (U+2010 to U+2015, the en and em dash among them), MINUS SIGN (U+2212), TWO-EM and THREE-EM DASH (U+2E3A, U+2E3B), SMALL EM DASH (U+FE58), SMALL HYPHEN-MINUS (U+FE63), FULLWIDTH HYPHEN-MINUS (U+FF0D).
- **Kept**: everything else, case and punctuation included, and (**Proposed**, open question 0) ZERO WIDTH NON-JOINER and ZERO WIDTH JOINER.

Then leading and trailing whitespace is trimmed, then NFC (unicode-normalization, the crate vsf's own `x` encoder normalizes with).
NFC comes last: dropping a character between a letter and its combining mark leaves a pair NFC composes (`e`, ZWSP, U+0301 is `é`).
No character NFC produces is one the classes change (U+2000 and U+2001 decompose to U+2002 and U+2003, but are spaces before NFC sees them), so the result is its own scrub.

**Two forms** (**Proposed**, open question 1):
- **Shown** (`shown`): the above. What a phrasing record keeps and a person reads. ferros_lang has ten messages that are multi-line passages ("on, the device opens without your handle:\nwhoever holds it holds your identity"), so a line break survives as one `\n`.
- **Scrubbed** (`scrub`): the shown form with each `\n` a space. What the id hashes; so a line break never makes another idea.
- A phrasing's text must already be in its shown form or the record is refused, so the words shown and the words hashed differ by line breaks alone: a bidi override or a zero-width space can never make two look-alike phrasings hash apart, or a first phrasing look like something it does not hash as.

## Records

A record is a section in a complete VSF file: a **sheet**.

### The sheet (**Proposed**, built: `ferros_idea::Sheet`)

- A complete VSF file (`RÅ<`, version, Eagle creation time, provenance, the signer's `ke`, an egg per scheme in `gm`), its sections `phrasing` and `link` in any number, built with vsf's schemas (`SectionSchema` + `SectionBuilder`).
- **Signed with every egg** (Ed25519, Falcon-512, SPHINCS+), as a kist's capsule and Photon's releases are: `fgtw::release::release_slots` to reserve, `sign_release` to sign, `verify_signed_by` to check against the signer's whole bundle, never Ed25519 alone.
- **The author and the time are the file's own**: the header's signer and its creation time, under the eggs with everything else. A section does not repeat them (the brief listed `author ke` and `created e` per record): a second copy could disagree with the first, and the header's is the one the signature proves.
- **Many records, one signature**: the eggs are about 8.6 KB a sheet (SPHINCS+ 128s alone is 7856 bytes), so an author writes everything they have at once in one sheet: ferros's catalog, 233 messages in sixteen languages, is one sheet, not 3728 files of 8.6 KB.
- **A capsule is a sheet too**: its `phrasing` and `link` sections ride under the capsule's eggs, read by the same `Sheet::read`.
- **Reading**: `Sheet::read` takes any VSF file, checks its provenance and its Ed25519 anchor (`read_verified`) and names no author. `Sheet::verify` also checks every egg against the signer's bundle from the caller's list and names the signer as every record's author. A record from a sheet only read has no author: the display rule never trusts it, except a first phrasing, which proves itself.
- **Refused whole**: not a whole VSF file; a phrasing whose text is not in its shown form, without its idea or words, or whose locale is not three letters; a link without its two ideas. **Skipped, not fatal**: a phrasing whose `register`, or a link whose `kind`, this reader does not know (a later writer's).

### Phrasing (section `phrasing`)

| field | type | |
|---|---|---|
| `idea` | `hi` | the idea it phrases |
| `locale` | `wl` | its language; the ISO 639-3 code is what counts (script and region ride in `wl` and are not yet read, open question 3) |
| `text` | `x` | the words, in their shown form |
| `register` | unsigned | only when not plain: 0 terse, 1 plain, 2 verbose (**Proposed**; owner: "A phrasing may later carry a register (terse/plain/verbose) beside its locale") |

A phrasing is the **first** phrasing of its idea when its locale and scrubbed words hash to the idea: it proves itself, whoever signed it, and needs nobody's trust to be shown.

### Link (section `link`)

| field | type | |
|---|---|---|
| `kind` | unsigned | 0 same as, 1 related |
| `ideas` | `hi`, `hi` | the two ideas |

- **Decided:** "Collisions of meaning (not hashes): the same meaning minted twice (e.g. Korean with Chinese phrasings, later English) is joined by a signed 'same as' link; a device follows links signed by keys the person trusts; neither id is rewritten; the older id is shown first."
- **Decided:** a related link (close, not the same) only helps search, never changes what is shown.
- **Decided:** an ambiguous phrasing ("Close") is the minter's to fix with a second idea, not machinery.
- The kind is a typed unsigned field, never a section name or a word (AGENT.md: a row's kind is a field).

### A preference or vouch record: not needed (**Proposed**)

- A person preferring a phrasing signs it themselves: the same idea, locale and words in a sheet of their own key, which ranks first (their key is first in trust). Nothing new on the wire.
- A correction is a newer phrasing by the same key in the same locale and register: the newest wins.
- "Most used" (owner, as a later tiebreak) needs counts, not vouches; deferred.
- What is missing is withdrawing a link (open question 6).

## The display rule

**Decided** (owner, 2026-11-10): "Shown in the person's language if a phrasing exists, else the idea's first language marked as such; never blank, never guessed."
The order of trust and the details are **Proposed**, built as one pure function (`ferros_idea::show`) of the idea, the phrasings and links the device holds, the person's languages in order, the keys they trust in order, and the register the place wants:

0. **The family**: the idea and every idea a same-as link by a trusted author joins to it, link to link. Related links are never followed. Oldest first ("the older id is shown first"): an idea's age is the earliest creation time of a trusted sheet carrying its first phrasing; ideas with no known age come after, and ties go by the ids' bytes, so every device orders a family alike (open question 4).
1. **The person's languages, in their order**; for each, the family oldest first; for each idea, its phrasings in that language by keys the person trusts, best-ranked key first, then its first phrasing if it is in that language, whoever signed it. Among one key's: the register asked for, then plain, then any; then the newest.
2. **Else the first phrasing**, in its own language, marked as such (`Shown::First`; its locale is on it, for the mark): the oldest idea's that the device holds.
3. **Never blank**: holding none of the family's first phrasings is `Shown::Missing`, and the caller draws the id itself (its first bytes in the person's base). Every carrier of an idea carries its first phrasing (below), so this is a broken carrier, never a normal state.

## Trust inputs

**Decided:** "a device follows links signed by keys the person trusts"; ferros's own phrasings are signed by the owner's developer key until the project key exists ("My dev key is fine").
**Proposed** order (the `trusted` list, best first):
0. **Theirs**: the person's own key (open question 10: the identity's key, or each of their devices').
1. **Their contacts'**: the keys of people in their phonebook (fgtw), in the order the person ranks them, else as listed.
2. **The developer's**: for a kist's ideas, the bundle the person's list pins for that developer (KISTS.md "Trust on first stow"); for ferros's own words, the owner's developer key.
3. **The project's**: ferros's project key once chosen (KISTS.md, "Which key signs the directory"); Photon's release key for Photon's words.

Verifying a sheet needs the signer's whole bundle (`fgtw::pq::KeyBundle`), which the device already holds for each of these (the pin, the phonebook, the pinned release bundle); ranking needs only their Ed25519 keys.
A key the person does not trust contributes nothing but self-proving first phrasings.

## The directory

**Decided:** an ideas directory beside the kist directory (kists.fgtw.org), no account, no gatekeeper, signed records; "anyone, yes": it accepts writes from anyone; devices show only phrasings from keys the person trusts.
**Decided:** minting searches existing ideas' phrasings in every language first ("is it one of these?").
**Proposed:**
- **Where**: the same bucket and host as kists, plain HTTP (the device has no TLS and fetches exact paths): `/ideas/<blake3>.vsf` a sheet by its own hash, `/ideas/by/<hi hex>.vsf` the list of sheet hashes that carry an idea.
- **A write** is one signed sheet that carries its signer's bundle (`kb`, as a capsule does), so the directory checks every egg at the door against the key the sheet names: not trust (anyone's key passes), only that the sheet is whole and its signer real. Any sheet that verifies is kept; nothing is edited or removed by the directory.
- **A read** is by idea: the device fetches the sheets for the ideas it shows and keeps those whose signers it trusts (plus every first phrasing).
- **Search** for minting: the directory's page and the device's minting screen search phrasings in every language, so a person minting "Remove from fleet" finds "Quitar de la flota" first.

## What a capsule carries

**Decided:** "a capsule carries the ideas it uses with its developer's phrasings."
**Proposed:**
- The capsule's own `phrasing` (and `link`) sections, under its eggs: for every idea it names, at least the first phrasing, and the developer's phrasings in every language they have.
- Kist text names ideas: a want's reason becomes an `hi` in place of today's `a` line, as may the kist's description; its name stays a name, never translated (ferros_lang's rule).
- At stow the device keeps the capsule's sheet beside the capsule: the developer's phrasings are trusted as the pin is, and every first phrasing proves itself, so a kist's reasons always have words.

## Migrating ferros_lang (plan only, nothing migrated yet)

**Decided:** ferros's own UI strings become ideas; a new language is phrasings, not code.
**Proposed** steps:
0. **Mint**: a host tool (`tools/ideacat`, std) walks `Msg::ALL` and, for each message, mints its idea from its English arm (`eng`, ferros's lower-case voice: "settings" is not Photon's "Settings", a same-as link joins them later if wanted) and writes all sixteen languages' arms as phrasings into one sheet, signed by the owner's developer key (`kistcat devkey`). Two messages whose English is the same are refused until one is reworded: today `ScopeClear` and `KeyClear` are both "clear" in English but "sin filtro" and "limpiar" in Spanish, two meanings ("the minter's to fix with a second idea").
1. **Freeze the first phrasings**: the tool writes a generated table, checked in, `ferros_lang/src/ideas.rs`: `pub const IDEA: [Id; Msg::ALL.len()]`, one `hi` per message in declaration order, beside each the first phrasing's words as a comment. From then the English arm may change (a newer phrasing by the developer key, which the display rule prefers) but the id never does; a test checks every id still hashes from its frozen first phrasing, not from the current arm.
2. **Ship the sheet**: in the vault as device data, and later on the directory. `ferros_lang::text(l, m)` stays the compiled fallback until the shell reads phrasings from the vault; then `t(m)` becomes `show(IDEA[m], …)` with the person's languages and trust, and the per-language match arms go away one language at a time.
3. **Holes after**: `Say` (words with holes: a name, a typed word, a count in the person's base) is not migrated in the first step (open question 5).
4. **fgtw's words** (`fgtw::text::Msg`, the fleet's and identity's, shared with Photon) migrate in fgtw with the same tool, signed by Photon's release key, the project's.

## Built (2026-11-10, `ferros_idea`)

- `scrub`, `shown`, `is_shown`; `id`, `CONTEXT`; `Register`; `Phrasing` (`new`, `mint`, `is_first`), `Link`, `LinkKind`, `Sheet` (`unsigned`, `read`, `verify`), `Refused`; `show`, `family`, `Shown`.
- no_std + alloc: vsf with `x-full` and `crypto-verify`, unicode-normalization without std, fgtw's verify-only base; builds for aarch64-unknown-none. Signing (`fgtw::release::sign_release`) is the host's, as kistcat signs.
- Tests (24): same words give the same id, another language another; scrubbed variants agree (NFC, whitespace, zero-width, bidi, dashes, line breaks); case or punctuation make another idea; every script hashes; the id's bytes pinned; a sheet round-trips and verifies; a line break is kept and hashed as a space; a forged signature is refused (eggs by another key, a changed byte, a bundle whose post-quantum halves are another's, a signer not held); unknown registers and kinds skipped; text not in its shown form refused; the fallback is the first phrasing marked; languages in order, then keys in order; a link followed only when its signer is trusted, and the older id first; a related link changes nothing; a phrasing by an untrusted key is never shown; the register asked for, then plain, then the newest.

## Open questions

0. **ZWJ and ZWNJ.** The owner's rule drops zero-width characters; the built scrub keeps ZERO WIDTH JOINER and NON-JOINER, because they spell words (Hindi's half forms, Persian) and join emoji sequences (👩‍💻), and dropping them from the shown form changes what a person reads. Keep them, or drop them in the scrubbed form only?
1. **Line breaks.** The shown form keeps one `\n` per break and the id hashes it as a space. The alternative, collapsing them in what is shown, loses ferros_lang's ten multi-line passages.
2. **The context string.** `ferros idea`, as the brief named it; Photon and fgtw will mint ideas too, so a project-neutral context (`vsf hi`, `TOKEN idea`) may read better. Fixed forever once one idea is published.
3. **Locale.** The id hashes the ISO 639-3 letters only, and the records' `wl` carries script and region unread: `zho` in Hant and Hans with the same words are one idea, and a person reading Hant is shown a Hans phrasing when it is the best. Hash the whole `wl`, or keep it to the language and let display prefer a matching script?
4. **"Older".** An id's age is the earliest trusted sheet carrying its first phrasing. A link could instead name which of its two is older; the signer then decides, and two trusted links can disagree.
5. **Holes and plurals.** `Say` needs holes: proposed, a phrasing's `text` as values in order, `x` runs and unsigned hole numbers counting from zero, hashed in that order (a phrasing without holes hashes as today). Counts need plural forms per language (Polish has three): a phrasing per plural category, like a register?
6. **Withdrawing a link.** Nothing undoes a same-as link a key signed; a kind "not the same" from the same key, newest wins?
7. **Register values** (0 terse, 1 plain, 2 verbose, absent is plain), and whether a key with only a verbose phrasing should be passed over for a plain one by a lower-ranked key.
8. **The directory.** Its host and paths; that a write carries `kb`; bounds on size and rate for writes from anyone.
9. **Cost per sheet.** Every egg is about 8.6 KB, heavy for one person adding one phrasing. The envelope tier (Ed25519 and Falcon, fgtw's `sign_envelope`) for person-written sheets would be about 0.7 KB; the owner's rule so far is every scheme.
10. **"Theirs".** The person's own key: the identity's, or each of their devices' (a device signs with its own bundle today).
11. **"Most used"** as a later tiebreak among trusted phrasings: needs counts the device does not keep.
12. **Publishing order.** Ids depend on vsf v11's codebook and `hi`, both unpublished: no idea is published before v11 is.
13. **Feature unification.** ferros_idea turns on vsf's `x-full`: a whole-workspace build gives every crate that links vsf the `x` codec and NFC (about 133 KB); payloads built alone (`-p`) are unchanged.
