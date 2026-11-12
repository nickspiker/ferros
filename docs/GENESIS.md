# The Genesis kist

A Sega Genesis (Mega Drive) emulator as a kist: [jgenesis](https://github.com/jsgroth/jgenesis)'s Genesis core (GPL-3) under a ferros shell, a touch pad drawn by fluor, saves as VSF.
Scoped and shelved for night work on 2026-11-11 (memory `genesis_kist_shelved`); step A built the night of 2026-11-11 on the Pixel 8a (akita).

## Status

- **Step A runs** (2026-11-11, the 8a): Sonic 2 (the owner's own ROM, loaded at run time) on the panel at three times the Genesis's pixels, square, picture up top; the pad below it; every button and the D-pad's eight directions read from the touch server's held contacts; one emulated frame a panel frame from the display's frame wait.
  Played by injected touches: Start out of the demo, the title, a game in Emerald Hill, Sonic run right on a held D-pad (its right nub lit while held).
- **The pause page** (built the night of 2026-11-11, host-tested; not yet run on the 8a, which was off the USB bus all that night): back's Escape opens it over the game, which stops; Resume, Save state, Load state, Reset; Escape again resumes (below, "The pause page").
- **Save states** (built, host-tested): VSF all the way, the emulator's every field by name thru a `vsf` feature on jgenesis (below, "Saves"); about 340 KB for Sonic 2 (the VDP's frame buffer left out, redrawn by the next frame); a state saved, the game moved on and the state loaded gives the same frames again.
  Kept in the kist's memory until it ends: the store they belong in (STORE.md, a draft) does not exist yet, and the page says so.
- **H32 columns** area-weighted (below, "The picture"); **the icon** made from the owner's picture (below, "The icon").
- **Sound** (built the night of 2026-11-11 behind the kist's `audio` feature, off by default, host-tested; not yet run: no device was on the bus, and the `audio` service it writes to has not run on one either): below, "Audio".
  Without the feature the kist is silent, as in step A.
- **Saves:** the battery RAM's VSF schema is built and host-tested and goes the same way as a state; Sonic 2 has none.
- **Not a catalog kist:** GPL-3 shipping wants a lawyer's read first; nothing is published, no capsule is signed.

## Where it lives

| what | where |
|---|---|
| the kist's crate | `/mnt/Harbor/Code/genesis-kist` (its own git repo, no remote; the owner decides its home); the sound on its branch `audio` (worktree `/mnt/Harbor/tmp/genesis-kist-audio`) until merged |
| the core | `/mnt/Harbor/tmp/jgenesis`, branch `ferros` (v0.14.1, 386db85, plus the Sega CD and 32X gate, 7ad42bb, and the `vsf` feature, 04362e7 and d6f7413) |
| the build | `tools/genesis-build.sh` (this repo) |
| the icon | `genesis-kist/assets/icon.vsf`, made by `genesis-kist/tools/icon` from the owner's picture (the picture is not in the repo) |
| the ROM | the person's own file; never in a repo, an image or a capsule |

## How it fits together

- **The core** (`emu.rs`): `GenesisEmulator` ticked until a frame is done, the pad's buttons set on player one (a six-button pad, the core's default; the touch pad drives the three buttons and Start, and stays so: asked whether it should grow X, Y, Z and Mode, the owner, 2026-11-11: "nah on the six button"), the YM2612 and PSG muted in its mix without the `audio` feature (their samples would go nowhere; muting saves no time, below, "Audio"), mixed at 48 kHz and handed to the shell with it.
  The Sega CD and 32X are compiled out (owner: "Drop the CD and 32X"): genesis-core's new features `sega-cd` and `32x` (both default, so jgenesis's own frontends are unchanged) gate segacd-core, cdrom (with chd and its C zstd), s32x-core and sh2-emu; a facade module (`addon.rs`) stands in with uninhabited types when they are off, and asking for that hardware is a load error.
  jgenesis uses `float_algebraic` and `cfg_select`, stable after ferros-rust's nightly, so the build switches them on with `-Zcrate-attr` until the fork moves.
- **The window** (`ferros.rs`): the process's surface is the whole panel (WINDOWS.md); it is filled black, shown at the app level as fluor's 16-bit pixels (op 22 format 1, `G2T16`, which the display decodes) and opaque (mode bit 3), the whole panel its window.
  Each frame's show names only the rows that changed.
- **The picture's top** comes from the display's hit map, not a constant: once the window is up, the first row where both ends of the picture's width are the window's (op 27), found by halving.
  On the 8a that is row 207: the console's circles reach that far down at the picture's right edge.
  A full-screen window (op 43) would give the picture the whole top; the owner, 2026-11-11: "No full screen."
- **The picture** (`picture.rs`, `layout.rs`): the largest whole multiple of the Genesis's lines that fits the width, the frame's pixels square in H40 (320 wide) and 7:6 in H32 (256 wide), an interlaced frame (448 lines, Sonic 2's two-player screen) the same size with its lines half as tall (owner, 2026-11-11, "Deal").
  On the 8a: three times, 960 × 672, at (60, 207).
  H32 at three times is 896 columns for 256 pixels, 3.5 a pixel. Owner, 2026-11-11, on the uneven columns: "your call on the average".
  The call: area-weighted columns. Output column x covers [x·F/W, (x+1)·F/W) of the frame's F pixels over the picture's W columns, so every frame pixel is the same width on the glass and a scroll does not shimmer (nearest-pixel columns, 3 or 4 wide, made a scrolling edge step unevenly).
  A column inside one pixel shows it, by the same 256-entry table as H40; a column across a pixel's edge (a straddle: never more than one edge, a column being narrower than a pixel) mixes the two by how much of it each covers, in linear light, `L = wa·La + (1 − wa)·Lb` with `L = s²`, then back to a gamma-2 `s` by its square root and to a darkness by a 4097-entry table of `s` in steps of 2⁻¹² (`Lut::blend`): light adds, a gamma-2 value does not.
  On the 8a every seventh column is a straddle, half and half (`layout.rs` `Straddle`, the shares tested equal); on the Pro, 1194 columns for 256, every share is still equal to 10⁻⁵.
  H40 stays whole multiples (no straddles, nothing mixed), and rows are always whole multiples (an interlaced frame's lines exactly half as tall).
  Each frame row that differs from the last frame's is built once in a cached row and written as many times as the scale repeats it; the surface is device memory, written once a pixel and never read.
- **The colour**: the owner, 2026-11-11: "I'd just assume the primaries are VSF RGB, it's close enough and we can tweak later, plus I'm sure it's not colour managed."
  A level `v` (jgenesis already models the DAC's non-linear steps) is a gamma-2 VSF RGB value `s = v / 255`, white at diffuse white, so its `g2t` code is `tanh(s)` of 2¹⁶ (fluor's darkness is that code's complement): one 256-entry table.
  Gamma 2.2 (a CRT's) would be the other reading; one line changes it. Seen on the 8a, the owner, 2026-11-11: "good".
- **The pad** (`pad.rs`, `layout.rs`): portrait, below the picture, centred on the band's middle row; sizes from the panel's span (a sixteenth of it is the unit: 93 px on the 8a).
  The D-pad (radius 2.2 units) in the left quarter, eight 45° sectors out to a fifth past its rim, a dead middle a fifth of its radius.
  A, B, C (radius 0.85 units) right of the middle, rising left to right as on the real pad, each reaching 1.3 radii, so a thumb laid between B and C holds both.
  Start a pill above, between them.
  Drawn by fluor front to back (label, then shape, then the band's black, by `under`) into a cached copy of the band, each control redrawn and copied alone when what it shows changes; a held control is lit.
  Landscape puts the D-pad and the buttons either side of the picture (laid out and tested on the host; the 8a has no rotation yet).
- **Touch**: the touch server's op 1 (HELD, the contacts on the glass now) read once a frame, right after the frame wait (with sound, the sound's write): a thumb held still sends no events, and the pad must still see it.
  A contact that came down where another window is in front (op 27 on its first frame) is not the pad's until it lifts.
  The buttons are the union of what every finger holds: multi-touch, a thumb on the D-pad and one on the buttons.
- **Pacing**: one emulated frame a panel frame from the display's frame wait (op 24), never a timer; with sound, the sound's write instead (below, "Audio").
  Op 24 returns at a frame start or, when none started, after a 120th of a second; on the 8a about half the waits came back that way, the count unmoved, and the first build ran Sonic at 83 frames a second.
  The shell waits again until the count moves.
  The panel's 60 Hz against the Genesis's 59.92 is a frame's drift every 12.5 s, unseen without sound (with sound, the audio clock leads and the picture follows it; below).
- **Behind and in front** (WINDOWS.md "The helm"): op 62 held on a thread of the shell's own, as fluor's ferros host does; behind, the loop sleeps on that thread's bell and the game is paused; in front again it is drawn whole.
  Back's Escape (G#29, a key event on that thread: the map sends op 61 when the helm's back is tapped) goes up one page: the game is the kist's main page, so from the game it opens the pause page, and from the page it goes back to the game.
- **The pause page** (`page.rs`; owner, 2026-11-11: "Yes on the page open with save/load/reset"): over the picture, the game's last frame drawn again at the picture's size (never read back from the surface) under a black veil at three quarters, "Paused", four of fluor's house pills (`Button::draw_pill_immediate`, the canonical button look) stacked in the middle, and a line under them saying where a state is kept ("A saved state is kept until the kist ends", then "Saved, kept until the kist ends").
  Composed front to back as fluor composes everything (the words and the pills, then the veil `under` them, then the frozen frame `under` all of it) into a buffer of the picture's size, and copied into the surface when something on it changes.
  Sizes from the span unit: pills six units wide and three quarters of one tall, a quarter between them.
  While it shows no frame is emulated; the loop keeps the panel's frames (op 24) and reads the held contacts as the game does, and the pad shows nothing held.
  A pill fires on release: the finger that came down on it lifts while still over it (held, it is lit); a finger already down as the page opens is never the page's.
  - **Resume**: the game again (as Escape).
  - **Save state**: the state written (below, "Saves") and kept; the page stays, saying so, and Load is live from then on.
  - **Load state**: the kept state of this game read back (whole and of this ROM, or refused, said on the page) and the game again, from there: the page closes and a frame is emulated before one is shown, so the first picture is the loaded game's next frame (whole: a state holds no frame buffer, below), never the frozen one.
  - **Reset**: the console's Reset button (jgenesis's soft reset: the 68000 and Z80 reset, RAM kept), and the game again.
  - **No Quit**: the house has none (deck backgrounds a kist, back bottoms out at its main page; WINDOWS.md "The helm"), and quitting would drop a state kept in memory.
  Each action's time is said on the console (`diag console`).

## Running it without an attested person (development)

The 8a sits at the attest screen with nobody attested, and ahau's window (the system level) covers everything.
The dev escape takes it down, then the kist is spawned over the bridge with the ROM as its input:

```bash
B=target/debug/ferros-bridge     # the debug bridge: the release one ignores FERROS_BUS
FERROS_BUS=1 $B diag "kill ahau"
tools/genesis-build.sh
FERROS_BUS=1 $B spawn /mnt/Harbor/Code/genesis-kist/target/ferros/aarch64-unknown-ferros/release/genesis.bin 00 \
    --name genesis --use display --use touch --dma surface --input-file <rom>
FERROS_BUS=1 $B diag console      # its lines: the title, the picture's row, a timing line every 512 frames, every change of the fingers and buttons
FERROS_BUS=1 $B diag "kill genesis"
FERROS_BUS=1 $B diag "restart ahau"
```

With sound (unrun): the kist built with the feature and the `audio` grant fourth, after the surface (the `audio` service up first):

```bash
tools/genesis-build.sh --features audio
FERROS_BUS=1 $B spawn /mnt/Harbor/Code/genesis-kist/target/ferros/aarch64-unknown-ferros/release/genesis.bin 00 \
    --name genesis --use display --use touch --dma surface --use audio --input-file <rom>
```

The byte after the image picks where the emulation runs: 0 the four A715s, 1 the X3, 3 core 7 alone (its cycle counter then tells its clock), 2 anywhere.
The ROM is the input file (a u32 length and its bytes after that byte); it never goes into the image.
The kist needs no right but display, touch and its surface for step A, and `audio` for sound.
`ferros-bridge input hold X Y FRAMES` presses the pad; see "Found on the way" for the injector's fix this needed on the 8a.
The pause page opens on back's Escape: a tap on the helm's back (bottom left), real or injected (`input tap`), which the map turns into the display's op 61 to the app in front; the map must be up under the kist for that (not yet tried with ahau killed).
The console then says each step ("back: paused, the page up", "state saved: a VSF document of A#n bytes", "Save took A#n us", "state loaded", "reset").

## Measured

Sonic 2 (Rev B, 1 MiB), the 8a, 2026-11-11, each line a block of 512 frames (`diag console`):

| where | emulation, mean | longest | clock | picture and pad | show | panel frames for 512 |
|---|---|---|---|---|---|---|
| X3 (option 1), one-player demos and play | 2.3 to 2.8 ms | 12 to 22 ms | 2.6 to 2.9 GHz | 0.14 to 0.2 ms | 0.17 to 0.6 ms | 512 to 519 |
| X3, the two-player demo (interlaced) and after | 4.5 to 7.0 ms | 25 to 35 ms | 1.7 to 2.2 GHz | 0.5 to 1.0 ms | 0.6 to 1.7 ms | 561 to 699 |
| A715s (option 0, any of four) | 7.5 ms | 11.8 ms | | 0.4 ms | 0.01 ms | 514 |
| one A715 (core 7, option 3), one-player | 7.5 ms | 11.9 ms | 1.06 GHz | 0.39 ms | 0.01 ms | 514 |
| one A715 (core 7), the two-player demo (interlaced) | 10.9 to 11.3 ms | 12.3 ms | 1.06 GHz | 2.0 to 2.3 ms | 0.04 to 0.3 ms | 512 to 533 |
| host: Ryzen 9 5950X | 2.1 to 2.2 ms | 3.5 ms | | | | |

- About 7 to 8 million cycles a frame on either core; the A715s run at 1.06 GHz under the current clock policy, the X3 at 2.8.
- The X3 is shared with the kernel and the display's render thread, and they fight: with the whole picture changing every frame (the interlaced two-player demo), the display publishes more and the emulation's frames stretch (the cycle counter over the wall clock fell to 1.7 GHz: the core waiting, or its clock lowered), and a quarter of the panel's frames were missed, staying so after the demo ended.
  One-player on the X3 is 2.5 ms with a few missed frames in 512 (its longest frames are other work in front of it).
  The A715s are steady: 7.5 ms one-player, 11 ms interlaced (about 4 ms to spare, 21 frames missed in one such block), the X3 left to the display.
  The default byte is 0 (the A715s); the owner, 2026-11-11: "middle cores is where it's at for now".
  The YM2612, the costliest chip, is already in these times: muted, jgenesis still runs it (below, "Audio"), so sound should add little to the emulation; what it changes is the deadline, a frame and its picture inside the ring's 16.7 ms, which the A715s meet and the X3's longest frames do not.
- The cycle column for "any of four A715s" is left out: a thread moving between cores mid-frame makes the per-core counter meaningless.
- The panel ran 59.8 frames a second; 512 frames took 8.56 to 8.61 s (59.5 to 59.8 a second).
- The image is 5.4 MB (release, thin LTO), its heap 64 MiB, its arc 72 MiB; 6.2 MB with the page and save states (the state code is the derive's, a save and a load a type).
  VSF's `x` text would have taken it to 15 MB (its encoder's codebook): the core's strings are written as `a` instead.

## Saves

Owner, 2026-11-11: "Proper VSF all the way. We can also note on saves to use the user vault rather than the kist."
A save is the person's, not the kist's: it goes in their vault (the common area, identity class), so it follows them to every device and a rebuilt kist (a GPL-3 build signed by another key) still finds it.
Never bincode at rest; jgenesis's bincode stays its desktop's business.

**Battery RAM** (built, `save.rs`, host-tested): one complete VSF file, section `genesis_sram`:

| field | type | what |
|---|---|---|
| `rom` | `hp` | BLAKE3 of the ROM as the core read it: which game this RAM belongs to |
| `title` | `a` | the ROM header's title, for a person looking at their vault |
| `saved` | `e` | Eagle Time |
| `ram` | `t_u3` | the RAM exactly as the cartridge holds it |

The document carries its own rolling hash; reading it back checks it (`parse_document`) and that `rom` is this game's.
The core writes the RAM when the game changes it; the shell makes the document, reads it back, keeps it in the store (below) and says so on the console.
Sonic 2 has no battery RAM, so this path did not run on the device.

**Save states** (built, jgenesis 04362e7, d6f7413 and 329b3b0, the kist's `save.rs`): one complete VSF file per state, two sections, every value typed and named, no positions and no blob of another format:

- `genesis_state`, the kist's: `rom` (`hp`, the ROM's BLAKE3), `title` (`a`), `saved` (`e`, Eagle Time).
- `genesis_core`, the core's (`GenesisEmulator::save_vsf`, `load_vsf`): first `core` (`a`, "jgenesis 0.14.1 ferros": a state from another core is refused, never guessed at), then the emulator, every field by its own name, dots for nesting: `m68k.registers.data` (`t_u5` of 8), `m68k.registers.ccr.carry` (`u0`), `z80.registers.a` (`u3`), `bus.memory.main_ram` (`t_u3`, 64 KiB), `bus.vdp.vram`, `bus.ym2612.channels.operators.phase.f_number` (`t_u4`, 6 by 4: each channel's four operators), `bus.psg.noise_channel.lfsr` (`u4`), `bus.input.inputs.p1` (`d`, "six_button") and its buttons under it, and so on, 1346 fields under 600 names for Sonic 2.

The mapping is generic, so it is the core's structure exactly, nothing hand-picked and nothing left out:

- `#[derive(VsfState)]` (jgenesis-proc-macros) sits beside every bincode derive in the crates a Genesis state reaches, behind a `vsf` feature in each (off by default, so jgenesis's own frontends are unchanged); the runtime half is `jgenesis_common::vsf_state`.
- A number is its sized VSF integer (`u3` for a `u8`, `i5` for an `i32`), a bool `u0`, a float `f5` or `f6`, a `usize` `u6`.
- A run of numbers (an array, a vector, an array of arrays) is one tensor of their type shaped as the run is: VRAM is a `t_u3` of 65536, a 16 by 4 array a 16 by 4 tensor.
- A run of structs is columns, one run a field, nested runs shaped by both: the VDP's 80 cached sprites are a `t_u4` and three `t_u3` (`bus.vdp.cached_sprite_attributes.v_position` and the rest), not four fields a sprite (a first cut did that to the frame buffer: 26 MB), and the YM2612's six channels of four operators are 6 by 4 tensors a field.
- A run of anything else (an enum, an option) repeats its fields in order: the k-th `bus.ym2612.channels.operators.envelope.phase` (`d`, "attack" and the rest) is the k-th operator's, channel by channel; never a decimal index in a name. A vector of such elements says its length first, in a field of its own name.
- An enum is a field of its own name holding its variant's name (`d`, snake case), its variant's fields under it; `None` a field of its own name with no value (a flag), `Some(x)` just `x`.
- Text is ASCII (`a`): the core's strings are a CPU's label and cheat codes, and VSF's `x` encoder would carry its codebook (about 9 MB) into the image.
- Left out: the ROM (`#[vsf(skip)]`: the game is not its state; the loaded emulator keeps its own, as jgenesis's `load_state` does), and the cheat overrides' 256 KiB address bitset (an index of the overrides, rebuilt on load).
  The VDP's frame buffer (owner, 2026-11-11, of its 816 KB in a 1.1 MB state: "Drop it to 340") is left out too: a state is taken between frames and the next frame writes every pixel it shows before anything reads one, so it loads cleared (black, as a new VDP's) and the first frame after a load is whole.
  Only interlace without jgenesis's `deinterlace` (on in the kist) draws one field a frame, so there the first frame shows the other field black and the second is whole.
  A state written with the frame buffer is refused by its fields (`bus.vdp.frame_buffer.r` where the next VDP field is expected); the core's name stays "jgenesis 0.14.1 ferros", since no such state outlived the run that saved it.
  Not dropped: the line buffers (about 3 KB; the sprites scanned and the CRAM dots carry across a line), and the audio resamplers' input rings (about 140 KB, mostly dead space past each live window, whose live samples feed the first outputs after a load; saving the live windows alone would take a state near 200 KB, not done).
  The Sega CD and 32X are not carried yet, so the feature refuses to build with them on.
- The reader walks the same structure in the same order and refuses any field whose name or type is not the one expected there, and any left over.

Tested on the host: a state thru a complete VSF file (read back verified) into another emulator saves the same fields again, bit for bit; it is bincode-identical to the original with that one's frame buffer cleared (bincode the test oracle only, never stored); both give the same frames from the first one after the load on, and after 64 frames are bincode-identical, frame buffers too.
Sonic 2 after 128 frames is 339422 bytes of VSF (1155714 with the frame buffer, 1134597 of bincode with it).
In the kist (`emu.rs` test `save_move_load`): saved, 41 frames on, loaded, the same first frame and the same 8 after it; a state of another game refused.

**Where saves are kept** (owner, 2026-11-11: "Two permissions, saving under the kist key and saving under the user key. We'll have to work out the filesystem since it won't have a directory structure. Find items by content, date, who created, whatever but never in a directory tree").
STORE.md (a draft for the owner's review) is that store, and Genesis its stage 0: items kept for the person, kinds "Sega Genesis save state" and "Sega Genesis battery RAM", found by `rom`.
Until it exists the kist keeps saves behind one boundary (`store.rs`), so the store plugs in there and nothing else changes:

- `Store::keep(doc)`: a whole document; the store reads what it is from the document itself (`save::what`: its first section, the kind, and its `rom`), checked whole first. STORE.md's make, write and finish, the card from the document.
- `Store::newest(section, rom)`: the newest of that kind for this game. STORE.md's find (the kind, `where rom` equal, newest first, one).
- `Store::lasts()`: how long a kept document lasts, which the page says.
- `Session`, the one store today: one slot a kind and game in the process's memory (a save replaces the last), gone when the kist ends; the page says "kept until the kist ends".

## The icon

Owner, 2026-11-11: "For the kist icon, /home/nick/Downloads/cartridge.jpeg is likely good" (a teal cartridge, gold contacts, on dark slate; 1024 square).
`genesis-kist/assets/icon.vsf` is it, in the form kist icons take today (KISTS.md: a VSF image, as tools/app-icons makes them; the same form as Opsin's orb): a complete VSF file, section `image`, field `data`, a 256 by 256 by 3 `t_u3` of VSF RGB gamma 2 codes, square, the circle inscribed in it the part that shows.
tools/app-icons takes such a file as an app's source (it cuts the circle with exact coverage and makes the photopic grey), and a directory entry names it with `--icon`.

`genesis-kist/tools/icon` (a host tool, outside the kist's build) made it: the picture decoded for ingest only (nothing is encoded to JPEG or PNG anywhere), taken as sRGB and brought into VSF RGB by vsf's `SRGB2VSF_RGB` as app-icons brings an app's PNG in; the cartridge found in it (teal or gold against the slate: its box 106 to 906 by 170 to 756, centre (506, 463), its farthest pixel 400 from there); the square centred there with the inscribed circle's radius a sixteenth past that (425), so the cartridge is centred and fills the circle; scaled by the exact area mean in linear light; stored as `√L` of 255.
The picture itself is not in any repo: whether it is the owner's own (and so may be committed with the tool) is the owner's to say.

## Audio

Built the night of 2026-11-11 behind the kist's `audio` cargo feature (off by default until it has run on a device), on the kist's branch `audio`; host-tested, never run: no device was on the bus, and the ferros `audio` service it writes to (AUDIO.md, ferros_sys `audio`) was written the same night and has not run on one either.

- **The core's side**: jgenesis mixes the YM2612 and PSG, resamples them to 48 kHz (its own default; the kist sets it anyway, `emu::RATE`) and pushes stereo `f64` held to -1..1 as they come, about 801 pairs a frame (48000 / 59.92; a PAL frame 966).
  No jgenesis change: the `ferros` branch is as it was.
  The kist makes each an i16 by `x · 2^15` and the cast, which saturates (+1 lands on 32767, a step short of the top), and collects a frame's pairs.
  The host test `sound_at_rate` checks 256 frames bring 48000 / 59.92 pairs a frame, to within 4 pairs in all.
- **What it costs**: muting the chips (`ym2612_enabled: false`, the kist without the feature) never saved their time.
  jgenesis runs the YM2612, the PSG, their filters and their sinc resamplers either way (genesis-core `bus.rs` `tick_components`) and only zeroes their volume in the mix, so the frames under "Measured" already carry the YM2612's emulation, the costliest chip.
  Sound adds the mix's few multiplies a pair, the conversion and one write a frame (a 3.2 KB message); read from the code, to be measured on the device.
  The image grows 4.7 KB (5906280 bytes against 5901480 without the feature).
- **The stream**: the `audio` grant is the fourth slot, after the surface (`--use audio`, above). As a kist (none signed yet) every right it takes is implicit (KISTS.md "The implicit set"): a capsule declaring nothing gets display, touch and surface in slots 1 to 3 and audio in slot 10, after the six and the GPU's three; one declaring display, touch, surface and audio in that order, as placements never asked, keeps these four slots.
  At start the kist opens a MEDIA stream (op 0) and says its rate, ring and period on the console, or "no sound" when there is no grant or no stream; a stream at a rate other than 48000 is refused aloud (it would play every note off pitch).
- **Pacing with sound**: the audio clock leads.
  After each frame its picture is drawn and shown (op 22), then its pairs are written (op 1, one write: 801 pairs is under the message's 1021) and the service holds the reply until all of them are in the stream's ring (1024 pairs, 21.3 ms), freeing 256 at each of the AoC's playback interrupts.
  That held reply is the game's clock, so it runs at 59.92 frames a second whatever the panel does; no panel frame is waited for, and the display puts the picture up at its next frame (one shown twice or one never shown now and then, never a sample dropped).
  Without the feature, without the grant, or once a stream is refused or the endpoint fails (said once), op 24 paces as before, silent.
- **Paused** (the page, or behind): no frame, so no write; the ring runs dry and the service goes quiet.
  A write the service answers with an unknown handle (it restarted, or dropped the dry stream) opens a new stream and writes the frame to it.
- **The deadline**: a write goes in only when the ring has room for the whole frame (801 of 1024), so as it lands at most 223 pairs (4.6 ms) are ahead of it, and its own 801 (16.7 ms) must last until the next write: the touch read, a frame's emulation, its picture and its show.
  On the A715s (7.5 ms one-player; 11 ms plus 2 ms of picture interlaced, 12.3 ms longest) that fits; on the X3 the longest frames (22 to 35 ms, "Measured") do not, and each is a gap heard.
  Writing in periods from inside the core's frame (256 pairs a write as they come) would keep the ring fuller while a frame runs and give a slow frame more room; not done, one write a frame first.
- **Latency**: a touch to its sound is the loop's 16.7 ms plus at most the 4.6 ms ahead of the write, about 21 ms; the picture and its sound come out within about a frame of each other.
- **On the console**: the 512-frame line adds "the sound paced A#n of them (its write their wait), the ring at least A#k pairs ahead of a frame's sound"; while the sound paces, "frame wait" is the write's wait and the panel frames counted stay 0.
  A least of 0 means the ring ran dry before a frame's sound went in: a gap heard.
- **To check on the device**: the stream's line at start; 512 frames in about 8545 ms (59.92 a second, against the panel's 8.56 to 8.61 s now); the least ahead never 0 on the A715s, the interlaced two-player demo the tightest; the emulation's time with sound against "Measured" (expected the same, the chips having run all along); the page and deck and back bringing the sound back (the console's "a new stream" if the service dropped it); the picture with no panel frame waited for (a frame doubled now and then is fine, a tear is not); the loudness at the device's volume (the core's mix runs near full scale).

## Bluetooth controllers (what they need)

- The bt process (payloads/bt) brings the radio up; HID is not built: pairing (BT.md, the identity owns pairings), the HID report descriptor read, and the reports served to the kist in front thru a `gamepad` right (a held call answered on each report).
- The kist maps a report's buttons to the Genesis's, a six-button pad's X, Y, Z and Mode too; the touch pad hides while a pad is connected.

## Found on the way

- **The input injector on the 8a** (payloads/inject, fixed on this branch): its frame clock started from op 5's frame count, which on the 8a is not op 24's frame-start count, so the first wait spanned some A#100000 frames and the whole script ran at once: a tap or a hold was down and up between two frames of the app under it.
  It starts from a first op 24 now; a 30-frame hold on Start is down for 14 of the kist's frames, a 240-frame hold on the D-pad drove Sonic right.
- **Op 24's fallback** (the 120th of a second with no frame started) is half the returns on the 8a: a client that runs one step a return runs fast. Worth saying in the display's op list, or answering only at a frame start.
- **The console's circles** reach row 207 at the picture's right edge on the 8a; full screen (op 43) would give the game the top, and the owner said no (2026-11-11: "No full screen.").

## Next

0. The pause page on the 8a: back opens it, save, move on, load, reset; the save's and the load's times on the device (built and host-tested; the 8a was off the bus the night it was built).
1. Saves kept past the kist's end: STORE.md's stage 0 (items for the person, the `items` right, the Genesis kinds as ideas), then `store.rs`'s `Session` swapped for it; the page's load then holds a watch.
2. Sound on the device: the `audio` service up, the kist built with `--features audio` and spawned with `--use audio`, the checks under "Audio"; then the feature on by default and the branch merged.
3. ROMs into the person's vault (over the bridge, a Photon transfer, the clipboard), and a page to pick one; the capsule never carries a ROM.
4. Bluetooth pads.
5. Landscape when the device knows it is turned.
6. The GPL-3 question before any listing.
