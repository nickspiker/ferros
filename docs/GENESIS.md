# The Genesis kist

A Sega Genesis (Mega Drive) emulator as a kist: [jgenesis](https://github.com/jsgroth/jgenesis)'s Genesis core (GPL-3) under a ferros shell, a touch pad drawn by fluor, saves as VSF.
Scoped and shelved for night work on 2026-11-11 (memory `genesis_kist_shelved`); step A built the night of 2026-11-11 on the Pixel 8a (akita).

## Status

- **Step A runs** (2026-11-11, the 8a): Sonic 2 (the owner's own ROM, loaded at run time) on the panel at three times the Genesis's pixels, square, picture up top; the pad below it; every button and the D-pad's eight directions read from the touch server's held contacts; one emulated frame a panel frame from the display's frame wait.
  Played by injected touches: Start out of the demo, the title, a game in Emerald Hill, Sonic run right on a held D-pad (its right nub lit while held).
- **No sound** (below, "Audio").
- **Saves:** the battery RAM's VSF schema is built and host-tested; nothing is kept yet (no right reaches the person's vault, and nobody is attested on the 8a). Save states are designed below, not built.
- **Not a catalog kist:** GPL-3 shipping wants a lawyer's read first; nothing is published, no capsule is signed.

## Where it lives

| what | where |
|---|---|
| the kist's crate | `/mnt/Harbor/Code/genesis-kist` (its own git repo, no remote; the owner decides its home) |
| the core | `/mnt/Harbor/tmp/jgenesis`, branch `ferros` (v0.14.1, 386db85, plus the Sega CD and 32X gate) |
| the build | `tools/genesis-build.sh` (this repo) |
| the ROM | the person's own file; never in a repo, an image or a capsule |

## How it fits together

- **The core** (`emu.rs`): `GenesisEmulator` ticked until a frame is done, the pad's buttons set on player one (a six-button pad, the core's default; the touch pad drives the three buttons and Start), the YM2612 and PSG switched off in its config (their samples would go nowhere; it also saves their time).
  The Sega CD and 32X are compiled out (owner: "Drop the CD and 32X"): genesis-core's new features `sega-cd` and `32x` (both default, so jgenesis's own frontends are unchanged) gate segacd-core, cdrom (with chd and its C zstd), s32x-core and sh2-emu; a facade module (`addon.rs`) stands in with uninhabited types when they are off, and asking for that hardware is a load error.
  jgenesis uses `float_algebraic` and `cfg_select`, stable after ferros-rust's nightly, so the build switches them on with `-Zcrate-attr` until the fork moves.
- **The window** (`ferros.rs`): the process's surface is the whole panel (WINDOWS.md); it is filled black, shown at the app level as fluor's 16-bit pixels (op 22 format 1, `G2T16`, which the display decodes) and opaque (mode bit 3), the whole panel its window.
  Each frame's show names only the rows that changed.
- **The picture's top** comes from the display's hit map, not a constant: once the window is up, the first row where both ends of the picture's width are the window's (op 27), found by halving.
  On the 8a that is row 207: the console's circles reach that far down at the picture's right edge.
  A full-screen window (op 43, the map's) would give the picture the whole top.
- **The picture** (`picture.rs`, `layout.rs`): the largest whole multiple of the Genesis's lines that fits the width, the frame's pixels square in H40 (320 wide) and 7:6 in H32 (256 wide), an interlaced frame (448 lines, Sonic 2's two-player screen) the same size with its lines half as tall (owner, 2026-11-11, "Deal").
  On the 8a: three times, 960 × 672, at (60, 207).
  H32 at three times is 896 wide, its columns 3 or 4 pixels by nearest frame column; an area-weighted column is the better answer when an H32 game is tried.
  Each frame row that differs from the last frame's is built once in a cached row and written as many times as the scale repeats it; the surface is device memory, written once a pixel and never read.
- **The colour**: the owner, 2026-11-11: "I'd just assume the primaries are VSF RGB, it's close enough and we can tweak later, plus I'm sure it's not colour managed."
  A level `v` (jgenesis already models the DAC's non-linear steps) is a gamma-2 VSF RGB value `s = v / 255`, white at diffuse white, so its `g2t` code is `tanh(s)` of 2¹⁶ (fluor's darkness is that code's complement): one 256-entry table.
  Gamma 2.2 (a CRT's) would be the other reading; one line changes it.
- **The pad** (`pad.rs`, `layout.rs`): portrait, below the picture, centred on the band's middle row; sizes from the panel's span (a sixteenth of it is the unit: 93 px on the 8a).
  The D-pad (radius 2.2 units) in the left quarter, eight 45° sectors out to a fifth past its rim, a dead middle a fifth of its radius.
  A, B, C (radius 0.85 units) right of the middle, rising left to right as on the real pad, each reaching 1.3 radii, so a thumb laid between B and C holds both.
  Start a pill above, between them.
  Drawn by fluor front to back (label, then shape, then the band's black, by `under`) into a cached copy of the band, each control redrawn and copied alone when what it shows changes; a held control is lit.
  Landscape puts the D-pad and the buttons either side of the picture (laid out and tested on the host; the 8a has no rotation yet).
- **Touch**: the touch server's op 1 (HELD, the contacts on the glass now) read once a frame, right after the frame wait: a thumb held still sends no events, and the pad must still see it.
  A contact that came down where another window is in front (op 27 on its first frame) is not the pad's until it lifts.
  The buttons are the union of what every finger holds: multi-touch, a thumb on the D-pad and one on the buttons.
- **Pacing**: one emulated frame a panel frame from the display's frame wait (op 24), never a timer.
  Op 24 returns at a frame start or, when none started, after a 120th of a second; on the 8a about half the waits came back that way, the count unmoved, and the first build ran Sonic at 83 frames a second.
  The shell waits again until the count moves.
  The panel's 60 Hz against the Genesis's 59.92 is a frame's drift every 12.5 s, unseen without sound (with sound, the audio clock leads; below).
- **Behind and in front** (WINDOWS.md "The helm"): op 62 held on a thread of the shell's own, as fluor's ferros host does; behind, the loop sleeps on that thread's bell and the game is paused; in front again it is drawn whole.
  Back's Escape is said on the console and does nothing yet: the game is the kist's main page (a menu for save states and reset is its next page).

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

The byte after the image picks where the emulation runs: 0 the four A715s, 1 the X3, 3 core 7 alone (its cycle counter then tells its clock), 2 anywhere.
The ROM is the input file (a u32 length and its bytes after that byte); it never goes into the image.
The kist needs no right but display, touch and its surface for step A.
`ferros-bridge input hold X Y FRAMES` presses the pad; see "Found on the way" for the injector's fix this needed on the 8a.

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
  The default byte is 0 (the A715s). Sound will add the YM2612, the costliest chip; with it the emulation wants either a faster A715 clock or the X3 with the display's render kept off it: the owner's call.
- The cycle column for "any of four A715s" is left out: a thread moving between cores mid-frame makes the per-core counter meaningless.
- The panel ran 59.8 frames a second; 512 frames took 8.56 to 8.61 s (59.5 to 59.8 a second).
- The image is 5.4 MB (release, thin LTO), its heap 64 MiB, its arc 72 MiB.

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
The core writes the RAM when the game changes it; the shell makes the document, reads it back, and says so on the console ("not kept: the person's vault is not reachable yet").
Sonic 2 has no battery RAM, so this path did not run on the device.

**Where it will be kept:** the person's common area, a name of the kist's choosing (`genesis`), each game's RAM by its `rom` hash.
That needs a right the kist asks for (KISTS.md's wants already name "a common-area name") and a person attested; until then the store is a stub.

**Save states** (designed, not built): one complete VSF file per state, named sections, every value typed, no positions and no blobs of another format:

- `genesis_state`: `rom` (`hp`), `title` (`a`), `saved` (`e`), `slot` (`u`, from 0), `core` (`a`, jgenesis's version: a state from another core version is refused, not guessed at), `timing` (`u`: 0 NTSC, 1 PAL), `region` (`u`).
- `m68k`: `d` and `a` (`t_u5`, eight each), `usp`, `ssp`, `pc` (`u5`), `sr` (`u4`), `stopped` and `halted` (flags), the prefetch and any part-done instruction state the core keeps.
- `z80`: each register pair by name (`af`, `bc`, `de`, `hl`, the shadow set, `ix`, `iy`, `sp`, `pc`: `u4`), `i`, `r`, `im` (`u3`), `iff1`, `iff2`, `halted`, the bus request and reset lines.
- `memory`: `work_ram` (`t_u3`, 64 KiB), `z80_ram` (`t_u3`, 8 KiB), `sram` (as above), the cartridge mapper's bank registers by name.
- `vdp`: `registers` (`t_u3`, 24), `vram` (`t_u3`, 64 KiB), `cram` and `vsram` (`t_u4`), the control port state (code, address, the pending second word, DMA in flight, by name), the counters (line, dot, frame, interlace field), the sprite state carried between lines.
- `ym2612`: the register file, then a nested `channel` section per channel and an `operator` section in each per operator (phase, envelope level and phase, by name), the timers and the DAC; nested sections, never `ch0_op1` names.
- `psg`: the tone and noise registers, the volumes, the counters and the noise shift register.
- `input`: the six-button pads' TH counters and timeouts.
- `timing`: the master clock's position, the cycles owed each chip.

Built as a `vsf` feature on jgenesis's ferros branch: each component writes and reads its own section by name (`to_vsf`, `from_vsf`); a host test takes a state thru VSF and back and compares it with the original by the core's own bincode, which is a test oracle only, never stored.
States live beside the RAM in the person's common area, by game and slot.

## Audio (what it needs)

- **The core's side is ready**: jgenesis mixes the YM2612 and PSG and resamples to any output rate (`update_audio_output_frequency`); it pushes stereo `f64` samples a frame at a time (about 800 at 48 kHz).
  Turning it on costs the YM2612's emulation (expect the frame to grow by a large part; measure on the X3 first).
- **The system's side is not**: the speaker amplifiers answer (AUDIO.md, the CS35L41s as a process), but there is no path for samples yet: the AoC's audio service or the I2S/TDM link to the amps, a mixer process that owns the output, and an `audio` right a kist asks for, with a stream op (a ring of samples in shared memory and a doorbell, never a poll).
- **Pacing with sound**: the audio clock leads; the emulation runs a frame when the ring wants one, and the picture is shown at the next panel frame (59.92 against 60 is one frame in 12.5 s: one shown twice now and then, never a sample dropped).
- Latency budget: a frame of emulation plus one of the ring, about 33 ms.

## Bluetooth controllers (what they need)

- The bt process (payloads/bt) brings the radio up; HID is not built: pairing (BT.md, the identity owns pairings), the HID report descriptor read, and the reports served to the kist in front thru a `gamepad` right (a held call answered on each report).
- The kist maps a report's buttons to the Genesis's, a six-button pad's X, Y, Z and Mode too; the touch pad hides while a pad is connected.

## Found on the way

- **The input injector on the 8a** (payloads/inject, fixed on this branch): its frame clock started from op 5's frame count, which on the 8a is not op 24's frame-start count, so the first wait spanned some A#100000 frames and the whole script ran at once: a tap or a hold was down and up between two frames of the app under it.
  It starts from a first op 24 now; a 30-frame hold on Start is down for 14 of the kist's frames, a 240-frame hold on the D-pad drove Sonic right.
- **Op 24's fallback** (the 120th of a second with no frame started) is half the returns on the 8a: a client that runs one step a return runs fast. Worth saying in the display's op list, or answering only at a frame start.
- **The console's circles** reach row 207 at the picture's right edge on the 8a; a game would rather be full screen (op 43) with the helm left.

## Next

0. The owner's eyes on the 8a: the pad's sizes and places, the colour reading, X3 or A715.
1. Saves kept: a common-area right for kists, then the battery RAM in it; save states (the `vsf` feature on jgenesis) and a page for them behind back's Escape (save, load, reset).
2. Audio once the system has an output path.
3. ROMs into the person's vault (over the bridge, a Photon transfer, the clipboard), and a page to pick one; the capsule never carries a ROM.
4. Bluetooth pads.
5. Landscape when the device knows it is turned; full screen for a game.
6. The GPL-3 question before any listing.
