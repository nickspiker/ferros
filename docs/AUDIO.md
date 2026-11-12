# Audio on husky

## Speaker amplifiers: control side cold (2026-10-20, WORKS)

Two Cirrus Logic CS35L41 smart amplifiers on `spi_19` (BLK_HSI0 USI1, G#111C_0000), from `zuma-shusky-audio.dtsi` and Google's driver (`private/google-modules/amplifiers/cs35l41` in the husky kernel tree).

| | Left (`cs35l41@0`) | Right (`cs35l41@1`, `cirrus,right-channel-amp`) |
|---|---|---|
| Chip select | the controller's own nSS, manual mode | GPIO gpn1-0 (`pinctrl@15060000` bank G#20), active low, controller in auto mode (`chip-select-mode = 2`) |
| Reset | gpp1-1 (PERIC0 bank G#20), active high | gph2-1 (HSI1 bank G#40), active high |
| VA | sub PMIC buck A (S2MPG15 G#29 ctrl / G#2A out; on at cold boot: ctrl G#F8) | same |
| VP | the battery directly | same |

SPI: 32-bit register address (bit 31 set for a read), 16 bits of padding, 32-bit value, all big-endian; mode 0, up to 25 MHz (ferros runs ~6.1 MHz from the oscillator: the kernel's `spi.rs` opens the clock path, `ferros_spi` runs the controller). The controller only clocks with its select logic active: for the GPIO-selected right amplifier it must be in auto mode (`SLAVE_AUTO | NSC_CNT_2`); with the native select held inactive every transfer times out.
The HSI1 power-domain status (PMU G#1546_2B04) is a plain read; EL3's priv-reg read refuses it (-22), as it does for pd_ispfe.

The kernel's `diag "amp up"` from a cold ferros (2026-10-20): reset 2 ms low / 2 ms high, IRQ1_STATUS4 (G#1_001C) OTP_BOOT_DONE already set, then

```
left: DEVID G#35A40 REVID G#B2 FABID G#69
right: DEVID G#35A40 REVID G#B2 FABID G#69
with the left held in reset: right DEVID G#35A40, left DEVID G#0
```

### As a process (2026-11-10, KERNEL-OUT.md move 7)

The kernel's `amp.rs` is now only the `amp` power switch: the port's clocks and USI mode, buck A on if it was off, the pins' modes, the right's select held high, and both resets pulsed (on); both held in reset (off, and when the process ends). The registers are `payloads/amp`'s, thru the shared SPI crate (`ferros_spi`) and three grants: `usi1-spi` (the port), `gpio-gpn0` (gpn1's data bit, the right's select; touch drives only gpn0's registers in that block) and `gpio-gpp0` (gpp1's data bit, the left's reset, held low only for the right-alone check). At its start it prints the same three lines as the kernel's tool did, then serves `amp`: op 0 both DEVIDs, op 1 one register of either amplifier. A development tool until audio is built, so started by hand, never by init:

```
ferros-bridge spawn amp.bin --name amp --serve amp --hw usi1-spi --hw gpio-gpn0 --hw gpio-gpp0 --power amp
ferros-bridge diag console          # amp: left/right OTP boot, DEVID/REVID/FABID, the right alone
ferros-bridge diag "kill amp"       # the switch's off: both amplifiers held in reset
```

## The design: from a kist to the speaker (2026-11-11)

Owner, 2026-11-11: "Audio, that does need to work", with the order "priorities are a nice clean wonderful OS that works excellenter and secondary on the genesis audio".
Built the same night (below, "Built"), nothing of it run on a device yet.

```
kist ── call `audio` (the right) ──▶ audio service (payloads/audio)
                                      ├─ a ring per stream, the mixer, the volume, the sound mode
                                      ├─ the AoC's ring `audio_playback5` ──▶ AoC entry point 5 ──▶ sink 0, TDM0 ──▶ both CS35L41 ──▶ speakers
                                      └─ the amplifiers over SPI (spi_19), their power thru the kernel's `amp` switch
            the AoC rings mailbox channel 20 each period it takes ──▶ the service's clock
```

### Who owns what

- **The AoC drives the speakers' port** (TDM0); the AP never touches the serial link (the research below).
  Its playback ring and its control queue are ordinary AoC IPC services, the same kind of rings the sensors process speaks USF over, in the same `aoc-ipc` window.
- **One process owns sound out: the audio service** (`payloads/audio`).
  It holds entry point 5 (Android's own on husky: its boot log binds `ep 5` to `TDM_0_RX` and finds `audio_playback5` interrupt-driven), the control queue, the amplifiers' SPI port and their `amp` switch.
  No kist touches any of it; a kist holds only a send right on the `audio` endpoint.
- **The kernel starts it on demand** (process.rs `ON_DEMAND`, as the torch): the first call on `audio` with no server starts it from its vault capsule with its own grants (service.rs `audio_grants`), since init's grants page is full.
  It ends itself once every stream has closed and played out.
- **The AoC must already be up.**
  The sensors process asks for its start at every boot (the `sensors` switch, KERNEL-OUT.md decision 2); the audio service only checks the control block's magic, and refuses an open without it.
- **`payloads/amp` stays a development tool** for reading the amplifiers' registers.
  It and the service both drive spi_19: never both at once.

### The stream: what a kist sees

- **The right:** `audio`, appended to the kists' vocabulary (ferros_kist `RIGHTS`, code 8, "sound"). Since 2026-11-12 it is one of the implicit set (KISTS.md "The implicit set"; owner, "Yes on the implicit set"): every kist is handed it without declaring it or being asked, in the slot after its GPU slots (ferros_sys::kist `audio`).
  The kernel makes it a send right on the `audio` endpoint (process.rs `kist_object`).
- **The format:** one for everything, the one the AoC's entry point is set up for: 48 kHz, two channels interleaved, signed 16-bit little-endian.
  Integer, not float: it is what the AoC's ring carries (Android's back end for TDM0 is `S16_LE`, aoc_alsa_card.c:245), so nothing is converted twice, and a kist converts once at the edge (the Genesis core's f64 by a `1 << 15` scale and the cast).
- **The calls** (ferros_sys `audio`, the protocol's one copy): `OPEN` a class (media or alert) → a handle and the shape; `WRITE` the handle and up to 1021 frames → answered once all are in the stream's ring; `CLOSE`; `GAIN` the stream's own level; `STATE`, `TONE`, `SIGGEN` for the bring-up.
- **The ring and the doorbell:** each stream has a ring of 1024 frames (21.3 ms) in the service.
  A write that does not fit is held: the service keeps the reply and answers it when a period played makes room, so the writer's own thread waits on the hardware's clock.
  The reply is the doorbell: no poll and no timer on either side.
  A call carries its frames (the kernel copies at most 4096 bytes arc to arc); there is no shared memory between a kist and a server in ferros yet, and at 48 kHz a write of about 800 frames a 60th of a second is a few dozen calls a second.
  A ring shared like the surfaces (a region the kernel hands out) would drop the copy; the API (`Stream::write`) does not change if it comes.
- **A stream that goes quiet:** a ring empty for 256 periods (1.4 s) with no write waiting is closed, counted in periods played, never on a timer.
  That covers a paused game and a kist that ended (an ending process tells its servers nothing).
  A write on a closed handle answers "unknown", and the kist opens again.

### Mixing

- Each period (256 frames, 5.3 ms) every live stream is mixed in i32: its sample times its gain times the device's volume, the gains in Q15.
- **Volume** (preference 46, `volume`, a device setting, SETTINGS.md): 1-255, each step a 16th of an octave of amplitude, 255 full scale; 0, never set, is two octaves down.
  A level is a gain by shifts and a 16-entry table (ferros_sys `audio::gain_q15`), so equal steps of a ring sound like equal steps.
  Nothing sets it yet: the quick row's sound circle has no page (QUICK.md), and the audio page is where its ring goes.
- **The sound mode and do not disturb** (preferences 39 and 40): a stream is media (games, music, a call's voice) or an alert (a notification, a ring).
  Media plays in every mode; an alert plays only in sound and not while do not disturb is on, its frames still taken so its writer keeps time.
  Vibrate's buzz is the haptics driver's, not this service's.
- **The rails:** the sum is the one place a sample can leave i16's range; it is held to the rails there, the reason in mix.rs `rail` (a sample that wrapped would jump full scale the wrong way, a crack the unprotected speaker must never get).

### Latency

The budget is under the 40 ms a game wants.
A stream's ring holds at most 1024 frames (21.3 ms), and the service keeps two periods (10.7 ms) queued at the AoC: 32 ms from a write's last frame to the AoC's ring at worst, the AoC's own pipeline to the speaker on top, to be measured (below).
A kist that writes a 60th of a second at a time sits between 4.7 and 21.3 ms of its ring.

### Clocked by the hardware

The AoC rings the AP on mailbox channel 20 (aoc_alsa.h:241 `PCM_CHANNEL`), block f1's bit 4, when it takes from the ring; the kernel hands that line (SPI 78, `aoc-mbox-f1`) to the service.
At each ring the service tops the AoC's ring back up to two periods, a period mixed at a time, then answers every held write that now fits.
The only other wait in the service is on the control queue's replies, at a stream's start and end, never on the sound's path (aoc.rs `command`: provisional until the device says which channel the control queue answers on).

### Power

- Silence holds nothing: no service, no arc, both amplifiers in reset.
- The first open brings it up: the `amp` switch on (the rail, the port's clock, both resets pulsed), each amplifier from reset to ready (amp.rs), entry point 5 bound to the speaker and set up, two periods of silence queued, GLOBAL_EN on both, the source on.
- After the last stream closes or goes stale and 16 periods of silence have played (85 ms), the source goes off, the amplifiers are powered down (GLOBAL_EN off, PDN_DONE) and the speaker unbound, and the process ends; the kernel turns the `amp` switch off with it, holding both in reset.

### Speaker protection

Android loads the amplifiers' own DSP firmware (`cs35l41-dsp1-spk-prot.wmfw` and a tuning file each, `R-` for the right; Android's boot log) and plays thru it: excursion and temperature limits.
ferros runs the amplifiers in bypass (the DAC on the serial port), so the only protection is the level: the default volume two octaves down, the first tests far under that, the owner listening.
Loading the protection firmware is the next step for the amplifiers (its files from the vendor partition into the vault's `amp.` scope, as the AoC's scripts went, then the wm_adsp load and the mailbox the firmware speaks); until then a loud setting is the owner's risk, said here.
In bypass the right amplifier takes the second slot (DAC_PCM1_SRC = ASPRX2) itself: Linux leaves both on the first and lets the firmware pick.

### Open (the owner's)

- The rails: a hard hold now; a smooth knee near full scale would distort less when streams sum loud.
- The volume's page and the sound circle's ring.
- Whether a kist with `audio` may also open an alert stream, or alerts are a separate right.
- Microphones (they are the AoC's too: `audio_input_control`, the capture rings) and Bluetooth and USB sinks: the same service, later.

## The path to a first sound (research from source, 2026-10-25 night; nothing here run on the device yet)

**Who drives the speakers' port:** the AoC. Both amplifiers sit on `dai_be_tdm0_rx/tx` (zuma-shusky-audio.dtsi:47-59), DSP_A format, the codec a clock consumer (gs101-audio.dtsi:678-708, aoc_alsa_card.c:1229-1233); the AoC maps `PORT_TDM_0_RX` to `SINK_SPEAKER` (aoc_alsa_hw.c:41) and Linux's back-end ops program nothing on the AP. The controller is a DesignWare I2S inside the AoC (aoc.bin's `TDMDesignWare`; its peripheral table puts TDM0 at AoC-local G#01C0_3000, AP G#18C0_3000, a G#58-byte DW_apb_i2s window, useful for watching IER/ITER/ISR0, not for driving it: the AoC's audio clock mux and pin muxing are undocumented).

**The AoC side, shortest:** the AoC firmware has a signal generator, so a first sound needs no PCM stream. On `audio_output_control` (8-byte header: type, counter, len u16, id u16, reply s16; a 4-byte reply is an error):
1. CFG_SIGGEN (id 225, 15 bytes: wf_type u8 @8, wf_period u16 @9, wf_amplitude u32 @11; never sent by Linux; the firmware's strings list DC, Square, Triangular, Sawtooth, so wf_type 0-3 is a guess).
2. EP_SETUP (id 206, 28 bytes; aoc-interface-zuma.h:1000-1017, filled at aoc_alsa_hw.c:1735-1798) with mode 5 = signal generator, on entry point 1; then EP_SETUP2 (id 298).
3. BIND2 (id 271: src entry point @8, dst 0 = speaker @9, bind 1 @10).
4. SOURCE (id 201: source @8, on @9 = 1).
PCM instead goes thru the ring service `audio_playbackN` (S16_LE stereo 48 kHz; TDM0 runs 4 × 32-bit slots, BCLK 6.144 MHz).

**The amplifier side, no DSP firmware:** DAC_PCM1_SRC (G#4C00) defaults to ASPRX1, the bypass (cs35l41-tables.c:74). After reset: the B2 errata patch, the OTP trim unpack (SPI at 4 MHz or less; ferros runs ~6.1), the DSP held off, boost for 1 µH / 4 µF, ASP format G#4808 = G#2020_0000 with RX width 16, clocking from SCLK (REFCLK 6.144 MHz, PLL closed loop, G#2C04 = G#510), ASPRX1 on, AMP_EN, then GLOBAL_EN (cs35l41.c:2168-2296, 2594-3009, 3675-3686). **The amplifier's PLL needs the AoC's SCLK: nothing comes out until the AoC runs TDM0.** It has no internal source of its own, and VMON/IMON leave it only on the serial port's TX side, not over SPI.

**Verification without ears:** the replies; `diag aoc log` ("Speaker TDM Started with Clk…", the SigGen line); TDM0's registers moving; the amplifier's PLL_LOCK (G#1_0018 bit 1) and PUP_DONE (G#1_0010 bit 24) with no error bits; GET_EP_TOT_SAMPLES (id 231) counting. Strongest: an AoC microphone capture.

**Not done unattended on purpose:** without the DSP there is no speaker protection, so the first drive should be at low amplitude with the owner present. Unknowns: the generator's fields, which entry points take mode 5, whether the AoC wants more state first (DSP mode 277, SINK2 270, AP state), and whether the OTP unpack can be skipped.

## Built (2026-11-11, not yet run on a device)

- **`payloads/audio`**, the service: main.rs (the endpoint, the start and the stop, the clock thread), mix.rs (the streams' rings, the mixer, the test tone), aoc.rs (the AoC's IPC rings, its mailboxes, the audio commands), amp.rs (both CS35L41 from reset to playing, in bypass, and down).
  Every register, command and step carries the reference file and line it comes from (the AoC's ALSA driver and IPC core, aoc-interface-zuma.h, the CS35L41 driver and its tables, the husky device trees, Android's boot log).
  Native build 175 KB; the masked build passes the sandbox checker (6227 words, 0 refused), so it can run masked as the sensors process does.
- **ferros_sys `audio`**: the protocol's one copy, the format, `gain_q15`, `level`, and `Stream` (open, write, gain, close) for no_std kists; std kists inline the same calls (the Genesis kist).
- **The kernel**: the `aoc-mbox-f1` interrupt line (SPI 78), `audio` among the on-demand servers with its grants (`audio_grants`: serve audio, aoc-ipc, aoc-mbox, aoc-mbox-f1, usi1-spi, gpio-gpn0, the `amp` switch, entropy), the `audio` right made a send right on the endpoint, preference 46 kept as `volume`, and the amplifiers' port clocked at half (about 3.07 MHz, spi.rs `halved`) for the OTP's 4 MHz limit.
- **The kists' vocabulary**: `audio` appended (ferros_kist), its words in every language (ferros_lang `Msg::RightAudio`), the map's Permissions page naming it.
- **The Genesis kist** (secondary): genesis-kist branch `audio` (worktree /mnt/Harbor/tmp/genesis-kist-audio), a cargo feature `audio`, off by default; GENESIS.md "Audio".

## Verification on the device (husky; the owner present for every step that makes sound)

The kernel carries the new line, switch clock, right and preference, so it needs the owner's flash (slot b, as usual) before any of this; the capsules first, as always: `tools/husky-shell.sh audio` builds, signs and stores the service (it is not started: the first call starts it), and `amp` and `epcall` must be current.

**0. Nothing audible: the service comes up and its clock runs.**

```
ferros-bridge diag console                     # sensors: AoC online, serving (the AoC must be up first)
ferros-bridge spawn epcall.bin 04 --name epcall --use audio     # STATE: ERR the first time (no server yet); the kernel starts `audio`
ferros-bridge spawn epcall.bin 04 --name epcall --use audio     # STATE again: AoC up, playing 0
ferros-bridge diag console
```

Expected: `epcall: reply` with byte 0 = 1 (the AoC up), and the service's lines

```
audio: AoC up, playing 0, A#0 streams, ...
```

`STATE` alone does not bring the amplifiers up; the next step does.

**1. The amplifiers and the AoC's path, at a level nobody can hear: a tone at level 95 (ten octaves, about 60 dB, under full scale) for 375 periods (2 s).**

```
ferros-bridge spawn epcall.bin 055F7701 --name epcall --use audio   # TONE: level G#5F, 375 periods (G#0177, little-endian)
ferros-bridge diag console
ferros-bridge spawn epcall.bin 04 --name epcall --use audio         # STATE while it plays
ferros-bridge diag console
ferros-bridge diag "aoc log"
```

Expected, in order:

```
audio: audio_output_control: channel A#n, queue, DOWN at G#… A#… bytes
audio: audio_playback5: channel A#20, ring, DOWN at G#… A#… bytes
audio: left amplifier: ready (DEVID G#35A40 REVID G#B2, PLL_CLK_CTRL G#510, PWR_CTRL2 G#…)
audio: right amplifier: ready (DEVID G#35A40 REVID G#B2, PLL_CLK_CTRL G#510, PWR_CTRL2 G#…)
audio: left amplifier: on, IRQ1_STATUS1 G#… (PUP_DONE 1, errors G#0), IRQ1_STATUS3 G#… (PLL lock 1)
audio: right amplifier: on, … (PUP_DONE 1, errors G#0), … (PLL lock 1)
audio: playing: entry point A#5 to the speaker, A#2 periods of A#256 frames ahead, the AoC's ring A#… bytes
audio: tone, 1 kHz at level A#95 for A#375 periods
audio: AoC up, playing 1, A#0 streams, A#… periods, A#0 underruns, A#… interrupts, A#0 starved
audio: control on channel A#…, playback on channel A#20, A#2048 bytes queued at the AoC
audio: the AoC says entry point A#5 has taken A#… samples
audio: silent, ending                       # 85 ms after the tone
```

What each says, and what to do when it does not:
- **The interrupts count about 187 a second** (48000 / 256) and the periods with them: the clock works. A count of 0 with periods stuck at 2: the AoC took the primed periods without ringing f1 (or rings another channel): read the channel the line printed, and the f1 block's INTSR1 (`aoc-mbox` + G#2_004C) by hand.
- **The AoC's sample count moves** between two STATEs: the AoC plays the ring. `diag aoc log` should say the speaker's TDM started ("Speaker TDM Started with Clk…").
- **PLL lock 1 on both** (read once at GLOBAL_EN, before the source is on: a 0 there and the amplifiers fine afterwards means the AoC starts TDM0 only at the source, so read IRQ1_STATUS3, G#1_0018, again with the `amp` tool's op 1 after the service is up, never while the service runs: the two share the port).
- **An error bit** (AMP_SHORT, BST_SHORT, TEMP, BST_OVP, BST_DCM_UVP in IRQ1_STATUS1) stops the start: say which.
- **"the AoC is not up"**: the sensors process has not started it (check its console lines).
- **"… not … inside the aoc-ipc window"**: the AoC put the audio rings past the 2 MiB `aoc-ipc` window (cachemap.rs `AOC_IPC`): the window grows, nothing else changes.
- **A control reply timing out**: the AoC answers on a channel this process does not hear; the line names the control queue's channel. Linux sends a few `get` commands before the first bind (dmesg:7268-7282, ids G#CD, G#E9, G#D3); if the AoC refuses BIND2 without them, they are next.

**2. A sound you can hear, quietly.**
Level 111 (G#6F, nine octaves down), then up an octave (16) at a time: 127 (G#7F), 143 (G#8F), 159 (G#9F), 175 (G#AF, five octaves, about 30 dB under full scale); stop at the first clearly audible one.
Never above 191 (G#BF) in the first session: there is no speaker protection (above, "Speaker protection").

```
ferros-bridge spawn epcall.bin 056F7701 --name epcall --use audio   # level 111, 2 s
ferros-bridge spawn epcall.bin 057F7701 --name epcall --use audio   # level 127
```

Listen for: a clean 1 kHz from both speakers (the bottom and the earpiece-side: left and right), no crackle (crackle with a steady `starved` count is the service late; with underruns 0 and starved 0 it is the link: the slot width or the word length, below).
One speaker silent: the right's channel (DAC_PCM1_SRC G#09, ASPRX2) or its select; read the right's DAC_PCM1_SRC and SP_ENABLES with the `amp` tool after the service ends.

**3. Android's own settings, to compare** (the register oracle, slot b's Magisk root, a capture as SENSORS.md's): while Android plays a sound, `cat /sys/kernel/debug/regmap/spi19.0/registers` and `spi19.1`: SP_FORMAT (G#4808), SP_RX_WL (G#4840), SP_FRAME_RX_SLOT (G#4820), DAC_PCM1_SRC (G#4C00), PLL_CLK_CTRL (G#2C04), GLOBAL_CLK_CTRL (G#2C0C), PWR_CTRL2 (G#2018), BSTCVRT_PEAK_CUR (G#3808).
amp.rs's SP_FORMAT (slot widths 32, DSP_A) and RX width 16 are read from the drivers, not measured: this is the check.

**4. A stream and the mixer.** The Genesis kist with `--features audio` (GENESIS.md "Audio"), at the default volume (level 223, two octaves down): its console line every 512 frames gives the frames paced by sound and the ring's lowest point (0 means a gap heard); the service's STATE gives underruns and starved.
Then the volume: `ferros-bridge diag "pref volume 207"` (an octave down from the default) while it plays, the change heard within a period (every period reads the preference).

**5. The end of it.** After the last stream closes or its kist ends, about 1.4 s plus 85 ms later the console says `audio: silent, ending`, `diag arcs` no longer lists `audio`, and the amplifiers' DEVID reads 0 from the `amp` tool (held in reset: the switch went off with the process).

**Not measured yet, and how:** the latency from a write to the air (a microphone capture of a click against the write's time; the AoC's microphones are the next service), the CPU the service takes (`diag sched`), and the AoC's own pipeline delay.
