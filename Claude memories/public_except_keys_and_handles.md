---
name: public-except-keys-and-handles
description: "EVERYTHING is public (memories, patents, code) EXCEPT exactly two things that live in keys/ and never touch a repo — KEYS and HANDLES"
metadata: 
  node_type: memory
  type: feedback
  originSessionId: 873bc7fe-6da9-4b85-b1db-5d51c15b7843
---

The disclosure policy, stated by the user emphatically 2026-08-20 after handles leaked into a public repo (again): **everything ferros/photon is PUBLIC — memories, patents, code, all of it, for humanity. EXACTLY TWO things are private, live only in `keys/`, and NEVER enter any repo: KEYS and HANDLES.** Handles are the maiden-names/SSNs; keys are the passwords. Everything else, always public.

- **Petname vs handle (do not invert — I inverted it twice):** in the map `keys/claude-pseudonym-map.txt`, format is `handle = petname`. LEFT = **handle** = SECRET (`seed = BLAKE3(handle)`, so a handle IS a key). RIGHT = **petname** = the human's ordinary name, SAFE in all public content. Use PETNAMES everywhere committed; a HANDLE in a repo is a key leak.
- Correction to earlier bad instincts: do NOT recommend a private memory corpus — memories are PUBLIC. The old CLAUDE.md "don't commit memory" line was a hallucinated default (removed). The photon README's "keep memories private" is likewise wrong; memories are public + petnamed.

**Enforcement (machinery, because "be careful" has failed repeatedly):** a fail-closed guard now BLOCKS any commit/push containing a handle, on EVERY repo:
- `keys/handle-guard.sh` reads the handle list (LEFT column) from the map and scans stdin; exit 1 = handle found, exit 2 = map unreadable (fail-closed).
- Global hooks at `~/.config/git/hooks/{pre-commit,pre-push}` invoke it (`git config --global core.hooksPath ~/.config/git/hooks`); pre-commit chains to each repo's own hook. Tested: three handles from the map blocked, a petname passes, missing map blocks. (This line itself once named those three handles: published before the guard covered ferros, replaced with this wording 2026-10-15.)
- **Exclusions (2026-10-23):** `keys/handle-guard-exclusions.txt`, one approval per line: `petname | repo | path-glob  # why, who, when`. It allows that person's handle only in diff content of matching files in that repo (never in commit messages, path names or plain text), and names people by petname, so it never holds a handle. The hooks now hand the guard whole `-U0` diffs and it follows file headers with a state machine (a content line cannot pose as a header). First entry: ferros `payloads/display/src/glyphs.rs`, whose swash geometry-crate imports collide with the owner's own handle. Add an entry only after a hit is confirmed benign; the owner said exclusions were needed. Backups of the pre-exclusion guard + hooks: keys/handle-guard-backup-2026-10-23.
- **The one hole:** ungateable handles (1-2 chars or numeric, e.g. the handle `1`) match everything and are skipped — those people MUST rotate to a distinctive handle to be gateable. Handle-choice policy: distinctive handles only.
- Per machine (MacBook/desktop): guard + map ride in `keys/` (Chiton/MEGA mirrored); just set `core.hooksPath` there too.
- **Repos with their own hooks path bypass the global hooks entirely** (a local `core.hooksPath` replaces the global one). Until 2026-10-15 that left ferros, photon and auscultatio unguarded, and handles were published from ferros and photon. Their own pre-commit/pre-push now chain the global hooks wherever they are installed. Any new repo that sets its own hooks path must do the same.

**How to apply:** publish freely — memories, code, patents. Refer to people by PETNAME (right column). Never write a handle or key into anything git-tracked; the guard enforces it. See [[secrets_canonical_keys_dir]].

**Refined 2026-11-03 (owner):** a handle is like a phone number. A celebrity does not want it scattered across a public repo, so the guard and the keys-only rule stand; but the owner puts it on a business card and hands it to people he wants to talk to, and sends it to people all the time. So "secret" is the wrong word: the handle is PRIVATE and handed out, never given to a machine that keeps it, and never written in anything public. Knowing it must get an attacker nothing (the fleet device is the credential, [[ira_is_brand_not_credential]]); the web page says so (ferros.org "No passwords?"). Design consequence to hold TOKEN to: nothing of consequence may derive from the handle alone.
**And (owner, 2026-11-03):** a handle can be changed, to any unclaimed one. So the handle is a label on an identity, not its root: continuity across a rename lives in the fleet chain (the anchor key, ira-rooted), the new handle is claimed at the roll, the old one is released, and contacts follow the chain, not the string. Any derivation that takes the handle alone would make a rename a new identity, which is one more reason none may exist.

**Refined 2026-11-12 (owner):** "Handles can go anywhere you'd like except for in the repo, btw." So in chat, the session's scratch files and device logs a handle is fine; never in any git repo (any branch, commit messages included). This memory folder IS in the ferros repo (Claude memories/, symlinked from ~/.claude), so handles never go in memory either, and never record which identity a test device uses.
