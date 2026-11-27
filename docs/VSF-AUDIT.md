# VSF-AUDIT: what ferros keeps or sends that is not VSF proper (2026-11-17)

Owner, 2026-11-17, on learning the flash log was never the ledger: "if it's not VSF proper and wow...".
A read-only sweep of forge (excluding `.claude/worktrees`) for every structure stored on disk, flash or in the vault, or sent between host and device, that breaks the house rules (photon AGENT.md: complete VSF documents built with the schema builders, EWE on disk, Eagle time, text for human words only, no web encodings; ferros CLAUDE.md).
Lean encrypted data frames, USB/TCP framing, hardware register formats and vendor partition formats are exempt.
No JSON or base64 exists device-side.

## Persistent (disk, flash, vault)

| # | What | Where | Breaks | State |
|---|---|---|---|---|
| P1 | the flash log: `FLOG` u32 header, `b<boot> t<ms>` text lines | kernel/src/flashlog.rs | fixed-width, counter time, numbers in text | being rebuilt on the ledger (branch ledger-stage0) |
| P2 | install's genesis spine entry: `hp` and `u` values pushed by hand, no magic, header or time | tools/install/src/main.rs:677-726 | hand-built bytes | live (install) |
| P3 | install's stem (kernel ring) entry: VSF assembled byte by byte, time hard-coded 0, no `kernel` field | tools/install/src/main.rs:931-1046 | hand-built VSF; the seed cannot parse it (ferros_hal ring.rs:384-390) | live, a real bug |
| P4 | ring and HAMT blocks stamp counter ticks as creation time | ferros_hal ring.rs:112,211,224; hamt.rs:207,361 | counter time as Eagle | dormant writers |
| P5 | `FEANCH01` anchor image | tools/mkimg main.rs:294-299 | fixed-width | dead (nothing reads it) |
| P6 | `.signed` trailer `FERROSIG` | mkimg main.rs:683-688; install :893-907; bridge :1510-1531 | fixed-width | live (host to device) |
| P7 | gate header `FERROSGT` | mkimg main.rs:1081-1084; ferros_hv gate.rs:37,63 | fixed-width | live (flashed) |
| P8 | legacy vault crate: `OBJ0`, `RCM0`, `VLT0`, MeshAnchor; wrapper with no time, plaintext labelled `v'e'` | crates/vault/src backend.rs, root_commit.rs, vault_anchor.rs, anchor.rs, vsf_wrapper.rs | fixed-width, hand-built | host tools only; `ferros_vault/` is an unbuilt copy |
| P9 | `FAT1` font atlas, `ICN3` icon bundle (display still reads `ICN2`) | ferros_kernel build.rs:66-92, text.rs; tools/app-icons; payloads/display main.rs:686,702 | fixed-width, a compatibility reader | live (baked) |
| P10 | kernel vault stamps seconds since boot as Eagle time | ferros_kernel kvault.rs:200 (open, put, delete) | counter time as Eagle | live, every commit |
| P11 | `\0names`, `\0id.names` stored newline-joined | kvault.rs:394-407 | delimiter string parsed back | live |
| P12 | bare bytes as vault values: `sealed`, `\0addrs.*`, `\0id.addrs`, `\0id.secret`, `\0id.unlock`, `\0id.bound`, `genesis` text | service.rs, ferros_kist seal.rs, kvault.rs, vault_boot.rs | no VSF document | live |
| P13 | sealed values stored as a raw `[nonce][ct][tag]` envelope | kvault.rs:337-352; crates/store/src/lib.rs:303 | no VSF document (the kvault tag record shows the right pattern) | live |
| P14 | vault process stamps seconds since boot as Eagle time | payloads/vault main.rs:198 into store heads, lists and every batch | counter time as Eagle | live, every commit |
| P15 | `gps.last`: two i32 LE | payloads/gps main.rs:627-636 | fixed-width | live |
| P16 | `gps.replay`, `.sky`: `GNR1` + `[u16 len][u16 gap]` records | payloads/gps main.rs:641-745 | fixed-width, counter time | live (placed by hand) |
| P17 | `map.place.*`, `map.view`: bare EWE runs; `map.dev`: one byte | payloads/map main.rs:2268-2278, 2407-2415, 5152; ferros_space lib.rs:402-450 | raw VSF types outside a document | live, synced |
| P18 | raw 32-byte secrets: `ahau.proof`, `ahau.wallpaper`, `fgtw.identity_seed`, `fgtw.handle_proof`, `fgtw.fleet_key`, `wallpaper.made.key` | ahau, fgtw, wallgen | no VSF document | live |
| P19 | `fgtw.sponsor`: fixed 19-byte struct | payloads/fgtw main.rs:702-706 | fixed-width | placed by hand |
| P20 | camera inputs `CFRM`, `CPR2`/`CPRG`, `CPR3`, the init table | ferros_cam inputs.rs, job.rs; tools/camera-*.py | fixed-width containers (the register contents are exempt) | live |

## On the wire (host and device, USB)

| # | What | Where | Breaks |
|---|---|---|---|
| W1 | bridge commands `[cap][op][params]` with fixed-width parameters and replies | ferros_pt command.rs; ferros-bridge, ferros-send | control messages not VSF |
| W2 | text status replies `ERR:…` / `OK` matched by prefix | service.rs; bridge :739,760,1627 | a kind in text |
| W3 | DIAG text commands with numbers parsed on the device | service.rs, prof.rs | numbers in text (development builds only) |

## RAM only

- R1: ferros_ledger entries use private one-letter tags, no magic, Eagle time 0; nothing posts them yet (rebuilt on ledger-stage0).
- R2: the kernel's wall clock is kept in UNIX seconds, converted to Eagle before anything is stored.
- R3: SYS_STORE's key listing is newline-separated (how P11 reaches processes).
- R4: DRAM handoff words (`SEEDTIME`, the hv's reset words, the ramoops vault report).

## Plan (proposed)

- Wave 0: Eagle time in both vault engines (P10, P14); every bare value and list as a VSF document (P11-P13, P15, P17-P19); old values converted once at first boot, their readers removed the same day. One flash with matching capsules.
- Wave 1: install's stem and genesis entries thru the builders (P2, P3), so the seed reads what install writes.
- Wave 2: our image headers and asset bundles (P5-P7, P9, P16, P20); the legacy vault crate (P8).
- Wave 3: bridge commands and replies as VSF control messages (W1-W3), with BRIDGE.md B2's signed commands.

## The reply form (owner, 2026-11-17)

> All I'm saying is error is a boolean, kinda? like we can add shit, like that's the whole point of VSF. RA<simple header>[error:true,"teency message as to why"]

Every reply is a complete VSF document: a typed `error` field (true or false), a short human message saying why, and any typed fields the command answers with.
No reply is ever matched by a text prefix; W2 goes this way first, and every new reply is built this way from now on.
