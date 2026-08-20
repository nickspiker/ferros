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
- Global hooks at `~/.config/git/hooks/{pre-commit,pre-push}` invoke it (`git config --global core.hooksPath ~/.config/git/hooks`); pre-commit chains to each repo's own hook. Tested: `zeno`/`esme`/`Carmen Sandiego` blocked, `Nick` passes, missing map blocks.
- **The one hole:** ungateable handles (1-2 chars or numeric, e.g. the handle `1`) match everything and are skipped — those people MUST rotate to a distinctive handle to be gateable. Handle-choice policy: distinctive handles only.
- Per machine (MacBook/desktop): guard + map ride in `keys/` (Chiton/MEGA mirrored); just set `core.hooksPath` there too.

**How to apply:** publish freely — memories, code, patents. Refer to people by PETNAME (right column). Never write a handle or key into anything git-tracked; the guard enforces it. See [[secrets_canonical_keys_dir]].
