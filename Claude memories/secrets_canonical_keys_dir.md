---
name: secrets-canonical-keys-dir
description: "ALL secrets go in /mnt/Harbor/Code/keys, synced only to two SSDs, the MacBook and MEGA (never GitHub: the old keys repo there was deleted 2026-11-10 and the ferros key rotated)"
metadata: 
  node_type: memory
  type: feedback
  originSessionId: 873bc7fe-6da9-4b85-b1db-5d51c15b7843
---

Hard rule from the user (2026-08-20, after scattered secrets cost them a week abroad when one was "strangely absent"): **every secret lives in `/mnt/Harbor/Code/keys/`** — that is the one directory the user backs up to two drives + MEGA. Nothing secret goes anywhere else.

- Do NOT write secrets to scratch dirs (I mistakenly put an SSAID backup in `/mnt/Harbor/husky-identity-backup` — moved it into `keys/`), repo dirs, or home config. Default every backup/key/token to `keys/` from the first write.
- The ferros repo has NO in-repo `keys/` dir (I briefly made a symlink, user rightly rejected the indirection — removed it). Nothing in the repo hardcodes `keys/` (only `mkimg` doc-comment examples); `mkimg` takes `--key <path>` explicitly. Sign with `--key ../keys/ferros.secret` (or absolute `/mnt/Harbor/Code/keys/ferros.secret`). The repo's `.gitignore` still has `keys/*.secret` + `keys/` as defense-in-depth. The whole key cleanup left ZERO tracked repo changes.
- Divergence is the enemy: there were TWO different `ferros.secret` keypairs (canonical Mar-18 pubkey `0e952ebe…` vs repo-local Mar-30 `3809ad3a…`). Neither was baked into any built/deployed artifact (husky runs the chainload path — raw ferros.bin, unsigned), so on 2026-08-20 I regenerated ONE fresh authoritative pair via `ferros-mkimg keygen -o keys/`: **live pubkey `05302376d7d2e8305ae158e911bbbd86fb09ffaa7e084d8eaca9300e5065ec71`** (fingerprint `993aaefc4c26a95d`). Both old pairs archived as `ferros-STALE-2026-03-18.*` and `ferros-STALE-2026-03-30-fromrepo.*` (kept, not deleted, as insurance — user may delete).

**Why:** mkimg patches this pubkey into `ferros_seed` `.rodata.pubkey` at sign time and signs kernel/seed with `keys/ferros.secret`; a wrong/missing key = unverifiable images with no obvious cause.

**Rotated 2026-11-10 (owner):** the 2026-08-20 pair had been pushed to a private GitHub repo (nickspiker/keys, created 2026-08-23, pushed through 2026-08-25). On 2026-11-10 that repo was deleted and the local keys repo's `origin` removed: keys sync only on the owner's two SSDs, the MacBook and MEGA, never GitHub. New live pubkey `3b472b6972369e179f7dfe0e7bb9ebbf6de0e8d41bfd78bbcea46324ed3cb80d` (fingerprint `adf41f9823b7ac2d`); the exposed pair archived as `archive/ferros-2026-08-github-exposed.*`. Nothing on the devices pinned the old key: the hv boot path does not use the seed image, and the kernel's gate checks each capsule against its own signer. Also there now: the owner's developer key for his kists (`developer-nick.key.vsf`, a `kge` seed under label FERROS_DEVELOPER_NICK, made with `kistcat devkey new`) and its signed public record `developer-nick.pub.vsf`. Photon's release key, the Spirix key, the macOS codesign cert, TOKEN.p12 and the SSH keys were in that push too; the owner left their rotation open.

**How to apply:** secrets → `keys/` always. Known straggler still outside `keys/`: the GitHub token lives only in `~/.config/gh/hosts.yml` (offer to mirror). See also [[android_id_userkey_seed]].
