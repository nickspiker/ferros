---
name: feedback-harbor-tmp-is-scratch
description: "/mnt/Harbor/tmp is scratch the owner clears at will; agents build in their own temp space and delete it; only device-unique data persists, in keys/<device>/"
metadata:
  type: feedback
---

Owner, 2026-11-17, after clearing /mnt/Harbor/tmp and the snapshots on a full disk (Harbor hit 100%, 5.8 GB free of 1.9 TB, much of it our build trees): "I'd save that as a rule, yes".

**Why:** the folder grew to hundreds of GB of build trees, factory images and boot-LU dumps that never needed to persist, filled the disk, and took the few things that did matter (device partition backups) with it when cleared.

**How to apply:**
- /mnt/Harbor/tmp is scratch: the owner may clear it any time without asking; never keep anything there that must survive.
- Agents build in their own scratchpad (/tmp/claude-...) and delete their trees and targets when done; nothing heavy left on Harbor.
- /tmp is a RAM tmpfs with a per-user quota shared by every Claude session (2026-11-17: 25 GB of build targets across the ferros, Photon and Lumis sessions hit it, and every command failed with EDQUOT). Big cargo targets (anything over a few hundred MB) go in a `target/` dir under the repo (gitignored, e.g. /mnt/Harbor/Code/ferros/target/agent-<name>), deleted when done; finished images and capsules go to /mnt/Harbor/Code/keys/<device>/ if they must survive.
- Device-unique data (persist, efs, efs_backup, modem_userdata, devinfo, pinfo, misc, GPT) lives in /mnt/Harbor/Code/keys/<device>/ with sha256 lists, local only, never in git ([[husky_factory_partitions_backup]], [[secrets_canonical_keys_dir]]).
- Factory images are downloaded again when needed; capsules and builds are rebuilt from git; findings go into docs and commit messages, raw logs are disposable.
