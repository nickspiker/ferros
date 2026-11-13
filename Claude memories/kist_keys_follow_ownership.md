---
name: kist-keys-follow-ownership
description: "A kist's developer key is its owner's: Photon and ferros are commons (project keys, like Fedora's), Lumis and Mahere are the owner's own apps (his developer key)"
metadata:
  node_type: memory
  type: project
  originSessionId: 434f1d40-b023-4c88-80b3-11913c5cd18f
  modified: 2026-11-10T19:08:24.694Z
---

Owner, 2026-11-10, choosing the key that signs the Photon kist: "Photon tho? that's everyones ... Same category as ferros. I don't own it. Linus does not own redhat, fedora."

- **Photon** and **ferros** are commons. Their capsules are signed with the project's key: Photon's kist with Photon's release key (the one that signs its update manifest and binaries), not the owner's personal key and not keys/ferros.secret.
- **Lumis** and **Mahere** are the owner's own apps: signed with a developer key of his.
- A kist's identity is (developer key, option), so custody of a project key is the ownership question. It may later move to the project (the DUNA, a threshold of maintainers); that needs a signed hand-off from one developer key to the next so a kist and everyone's answers survive it. Not built yet (KISTS.md gap).

**How to apply:** never sign a commons project's kist with a personal key; when adding a kist target, ask whose it is first. Related: [[ferros_vocabulary]], [[fgtw-duna-runanga]].
