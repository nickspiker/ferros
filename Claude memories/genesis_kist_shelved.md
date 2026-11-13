---
name: genesis-kist-shelved
description: "Sega Genesis kist (jgenesis core, GPL-3) scoped 2026-11-11 and shelved for night work: VSF RGB primaries, CD/32X dropped, proper VSF save states, saves in the person's vault not the kist's"
metadata:
  type: project
---

Owner, 2026-11-11: "would like an agent to scope out getting a SEGA genesis kist, check out https://github.com/jsgroth/jgenesis" ("a good test"). Scope result: the core (backend/genesis-core + components, m68000-emu, z80-emu) links for aarch64-unknown-ferros with chd's fast_zstd dropped (pure-Rust ruzstd) and -Zcrate-attr=feature(float_algebraic|cfg_select) until ferros-rust's nightly moves; ~1.65 ms/frame on the desktop, est. 4-7 ms on the X3. Clone at /mnt/Harbor/tmp/jgenesis (v0.14.1, 386db85); bench harness in the session scratchpad (gbench).

**Decided (owner, 2026-11-11):** "I'd just assume the primaries are VSF RGB, it's close enough and we can tweak later, plus I'm sure it's not colour managed. Drop the CD and 32X. Proper VSF all the way. We can also note on saves to use the user vault rather than the kist. Let's shelve it for night work."
- Genesis colours taken as VSF RGB directly (no colour management).
- Sega CD and 32X dropped (feature-gate them out of genesis-core; drops 32X's unsafe and the cdrom/chd C path).
- Save states and SRAM as a proper VSF schema, never bincode at rest.
- Saves live in the person's (identity) vault, not the kist's domain: they follow the person, and a rebuilt kist (GPL-3: a modified build signed by another key) still finds them.

**Pixel aspect (owner, 2026-11-11: "Deal"):** square for H40 (6x6 is 9.4% wide vs 5x6 8.9% narrow, a tie on a log scale, square the convention), 7:6 for H32 (2% off); whole multiples, portrait and landscape, the picture up top in portrait with the controls below. **Open:** GPL-3 shipping (source carried in the kist) wants a lawyer's read before the catalog. Audio (AoC service, amps, mixer, `audio` right) and Bluetooth HID are system work it waits on.

**How to apply:** night work, when nothing else is queued: step A (Sonic, touch overlay, no audio, ~1 week) first. Related: [[kist-catalog]], [[permissions-asked-once]].

**Built 2026-11-11 night (step A, then the owner's answers):** A715s stay default ("middle cores is where it's at for now"); colour gamma 2 VSF RGB "good"; "No full screen"; "nah on the six button" (pad keeps A, B, C, Start); H32 "your call on the average" → area-weighted columns, straddles mixed in linear light. Back's Escape opens a pause page (Resume, Save state, Load state, Reset; no Quit, the house has none). Save states = jgenesis `vsf` feature (`#[derive(VsfState)]` beside every bincode derive, runs of structs as columns, ASCII strings: `x` would add 9 MB). Saves: owner "Two permissions, saving under the kist key and saving under the user key ... never in a directory tree" → docs/STORE.md (draft), Genesis its stage 0; until then `store.rs` `Session` keeps one per kind and game in memory, said on the page. Icon: genesis-kist/assets/icon.vsf from the owner's cartridge.jpeg (VSF image, `image`/`data` t_u3 256x256x3). Not yet run on the 8a (it left the bus at 20:45 that night).
