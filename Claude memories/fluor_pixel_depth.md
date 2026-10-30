---
name: fluor_pixel_depth
description: fluor renders at 8-bit (Argb8) or 16-bit (G2t16) from one source since 2026-10-29; both α+darkness of VSF g2t codes; ferros display decodes G2t16 surfaces natively (op 22 format 1)
metadata:
  node_type: memory
  type: project
  originSessionId: 73becc5a-a246-4afa-9e54-76a7ff3d89c6
  modified: 2026-10-29T22:07:08.393Z
---

Since 2026-10-29 fluor (/mnt/Harbor/Code/fluor) is generic over `P: Pixel` (src/pixel.rs): `Argb8` (u32, unchanged output, golden hashes in src/golden.rs) and `G2t16` (u64, α<<48|R<<32|G<<16|B, 16-bit darkness of VSF g2t codes; an 8-bit code c is the 16-bit code c·256, same light). Owner decisions: α + darkness at both depths (faster); ferros's display takes G2t16 natively so fluor passes its buffer straight through (fluor = ferros's one compositor).

**Why:** ferros composites 16-bit (gamma 2, Q14, room to 4.0) and fluor's 8-bit g2t banded and had no headroom; one source keeps the depths aligned, a cross-check test (golden.rs) holds the numbers.

**How to apply:**
- The 8-bit kernels' `>> 8` for ÷255 is deliberate (owner: "I'm aware it's dark"); the cross-check pins that bias per scene, don't "fix" it. Overlay's swapped branches were a real bug, fixed (346282f).
- A source gate test (src/gate.rs) refuses 8-bit layout literals outside listed files; the list only shrinks.
- ferros: op 22's 19th byte = surface format (0 working values, 1 G2T16); display `working()` decodes (payloads/display/src/main.rs), fluor host/panel.rs keeps the reference decode in tests. ferros_sys `window::show_as`.
- Desktop (winit/softbuffer), Android and the gpu host present G2t16 too since e9cedec (2026-10-29): `paint::Finalize` per depth, 8×8 Bayer dither down to the 8-bit surfaces; `DefaultChrome::<G2t16>::with_depth`; `AndroidShell<App<G2t16>, G2t16>`; run_app infers the depth from a single `FluorApp<P>` impl; chrome crossings are Q16 (u16, u16, u16); `examples/depth.rs` is the reference 16-bit desktop app.
- Photon renders G2t16 on desktop, Android and ferros since b0da7536 (2026-10-29): `pub type Px` in photon src/ui/mod.rs, one cfg line per target (flip one back to Argb8 there); `src/ui/depth.rs` trait `Depth`; tapuwae `render_disc_g2t16`/`pack_g2t16`. Still 8-bit inside: opsin's viewer (`View::render` takes an 8-bit canvas, widened under the frame), AV1 user pictures and pie avatars (8-bit textures widened exactly at draw).
- ferros heaps at 16 bits: Photon's arc is 256 MiB (≈ 156 MiB real heap incl. slack), fluordemo's 64 MiB (≈ 60); declaring more only spends Photon's room before its arc doubles to the whole 512 MiB big pool (std/*/build.rs comments).
- Open: nothing 16-bit has run on glass yet (phone busy with another agent); the display's G2T16 decode is scalar — NEON later; colours at fluor's API are still authored 8-bit u32 (fine per owner). Plan: /home/nick/.claude/plans/greedy-brewing-sifakis.md. Related: [[colourspace-pinned-vsf-gamma2tanh]], [[tapuwae-neon-then-gpu]].
