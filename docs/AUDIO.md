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
- Bluetooth and USB sinks: the same service, later. (The microphones are built: "Microphones" below.)
- The microphones' gain: ferros sets none but the DC blocker, so the AoC's own defaults apply, and the first capture on the Pro (2026-11-22, a quiet room at night) peaked about 78 dB under full scale. Android sets a record soft gain of -40 dB at boot and raises it per use (5 dB unprocessed, 22 dB handset: mixer_paths.xml:240, :757-758, :847-848), by INPUT_SET_PARAMETER block 136, component 30, key 16 (hw.c:2236-2255; whole dB or float bits unknown, hw.c:2245), and the hardware gain by SET_MIC_HIGH_POWER_HW_GAIN (254, centibels, -720 to 240). Which gain a use wants (a call, a recording, a level meter) and who sets it is a decision still to make; the recording's playback is scaled from its own peak meanwhile, so the level heard does not depend on it.

## Microphones (built 2026-11-22, research from source; captured on the Pro that night, see "The rings' windows")

husky has three built-in microphones, ids 0, 1 and 2 (mixer_paths.xml:750-753; the HAL's bottom, back and top for them are marked "fake data" by the HAL itself, so ferros names them by id). They are the AoC's, like the speaker: the AP asks for a capture on the input control queue `audio_input_control` and reads the ring `audio_capture0` (PCM device 8, `fe_ep1_tx`: gs101-audio.dtsi:63-205). The service's `mic.rs` and aoc.rs's capture section, every step with its source:

- **The commands**, on the input queue only (its ids collide with the output queue's: 208 is AP_INPUT_START there and OUTPUT_AEC_NOTIFY here, zuma.h:119-123): MIC_POWER_ON (203) for each microphone (the factory test's; Linux's own capture leaves the power to the firmware, so a refusal is only said), SET_MIC_DC_BLOCKER (252) on, MIC_RECORD_AP_SET_PARAMS (224: three channels, 16 bits, 48 kHz, fixed point, PDM mask G#07, 10 ms periods, four of them, channel k from microphone k, raw), MIC_RECORD_AP_SET_PARAMS2 (359: device 8, the buffer and period in bytes), the microphone list as INPUT_SET_PARAMETER (233: block 139, value G#FF020100), AP_INPUT_START (208, input 1), the ring flushed, MIC_RECORD_AP_START_DATA (293). Stopped with AP_INPUT_STOP (209) and MIC_POWER_OFF (204). SET_PARAMS goes before AP_INPUT_START (the firmware refuses them while its input runs). A reply must carry the command's id: the input queue can carry the AoC's notifications (NOTIF_AUDIO_INPUT_AP_MIC_RECORD_DATA_AVAILABLE), which are passed over.
- **The ring** is read from its UP region (ipc_core.c:613-652): tx less rx is what waits, at most a ring (anything older was written over and is skipped, the frame kept by the count), the samples read a word at a time where aligned, then rx and rp moved. The AP never signals the AoC after a read (aoc_service_core.c:500-510).
- **The clock**: when `audio_capture0`'s mailbox channel is on block f1 (Linux treats channel 20 as interrupt-driven, aoc_alsa.h:241), the AoC's ring of the line this process holds clocks the capture; otherwise the ring is read every 128th of a second (Linux's own fallback is a 10 ms timer, pcm.c:258-278). The console says which at the first capture.
- **Gains**: none set beyond the DC blocker. On a cold AoC the firmware's defaults apply (Android's HAL sets a record soft gain of -40 dB at boot and raises it per path); the meter shows what comes, and the playback is scaled from the recording's own peak, so the level heard does not depend on them.
- **Unknown until the device says**: `audio_capture0`'s channel; whether MIC_POWER_ON is needed or refused; the firmware's default gains; whether 293 is needed after 208 (Linux always sends it).

The protocol (ferros_sys `audio`): `MIC_LEVELS`, held until the next 80 ms of samples (eight periods) have come in, then each microphone's peak; `RECORD` + microphone + level, held while 2^17 frames (2.7 s) of that microphone are captured into the service's memory, then played back once on both speakers, scaled so the recording's loudest sample comes out at the level (never louder than the speaker test can be). The recording is never written anywhere and is gone with the process. A capture nobody asks for again within 64 periods (640 ms) is stopped, and the process ends once nothing else holds it.

## The rings' windows (measured on the Pro 2026-11-22; approved that night by the coordinator for the owner)

The first PROBE on the Pro (forge 516d7559, kernel 07f922ca) found both audio rings past the 2 MiB `aoc-ipc` window the kernel grants (G#95C0_0000 to G#95E0_0000), as "Verification" below foresaw. The service's dump of their headers, offsets from the IPC block's start (G#95C0_0000):

| Service | Type, channel | Region | Offset | Size | Physical |
|---|---|---|---|---|---|
| `audio_output_control` | queue, 16 | UP / DOWN | G#12B000 / G#12A000 | 64 slots of 64 | inside `aoc-ipc` |
| `audio_input_control` | queue, 16 | UP / DOWN | G#12F000 / G#12E000 | 4 slots of 1024 | inside `aoc-ipc` |
| `audio_playback5` | ring, 20 | DOWN | G#17C000 | 4 MiB | G#95D7_C000 to G#9617_C000 |
| `audio_capture0` | ring, 20 | UP | G#EA0000 | 92160 | G#96AA_0000 to G#96AB_6800 |

Both rings ring channel 20 (block f1, bit 4), the line the service holds, so playback and capture are both interrupt-driven; the control queues answer on channel 16 (block f1 too). Both rings lie inside the AoC's carve-out (G#9400_0000 to G#9700_0000). The playback ring runs across the end of `aoc-ipc` and across G#9600_0000.

**The fix (kernel cachemap.rs `AOC_PLAY_LO`, `AOC_PLAY_HI`, `AOC_CAPTURE`):** three more grants, to the audio service alone (service.rs `audio_grants` slots 9 to 11), EL0 Device memory: `aoc-play-lo`, 4 MiB at G#95C0_0000 (a superset of `aoc-ipc`), and `aoc-play-hi`, 2 MiB at G#9600_0000, for the playback ring; `aoc-capture`, 1 MiB at G#96A0_0000, for the capture ring. The service maps each ring onto the windows that hold it (aoc.rs `RingMap`: at most two, each byte thru the one that holds it; a word never crosses an edge, words at multiples of four and edges at multiples of a MiB) and refuses a ring they do not hold whole.

**The tradeoff, for the owner:** a process reaches a granted region thru its handle, a base plus an offset masked to the region's size, which needs a power of two aligned to itself (SANDBOX.md "Handles"). The playback ring straddles G#9600_0000, a 32 MiB boundary, so one window holding it whole would be 64 MiB (G#9400_0000 to G#9800_0000, the whole carve-out and past it), and growing `aoc-ipc` instead would take the masked sensors process's handle with it. Two playback windows and one capture window, each the smallest aligned power of two for its part, are the smallest overreach the handles allow: the rest of those 7 MiB is the AoC's heap (its own buffers, the other services' queues and rings), which the audio service can now read and write as well as its two rings. The audio service is ferros's own OS process, the same trust class as the sensors process, which already holds `aoc-ipc`. The AoC's firmware image (the carve-out's first G#1C0_0000) stays the kernel's.

(The first cut, forge ef883736, put the playback window at G#9600_0000 to G#9680_0000 from a wrong sum of the ring's address; the second PROBE, on that kernel, showed the capture ring reached and the playback ring still refused.)

**Verified on the Pro (forge f82206f1, boot b1143; nothing sounded, nothing buzzed):** PROBE found `audio_playback5` in `aoc-play-lo` and `aoc-play-hi` (4194304 bytes, 0 queued) and `audio_capture0` in `aoc-capture`; GET_EP_TOT_SAMPLES answered; both amplifiers came to ready (DEVID G#35A40, REVID G#B2, PLL_CLK_CTRL G#510, PWR_CTRL2 G#3321, IRQ1_STATUS1 G#0, errors G#0), GLOBAL_EN never set, back in reset at the end. One MIC_LEVELS: the capture started (every command answered, the AoC ringing channel 20 on f1 as it wrote) and the first 80 ms gave peaks of A#4, A#3 and A#4 on microphones 0 to 2 (about 78 dB under full scale: a quiet room at night, and the firmware's default gains, which the HAL raises per path); 640 ms later the capture stopped by itself and the process ended. The AoC answers each input command with the command echoed and its own word in the id's place (224 came back with G#67, 359 with G#74). Not yet heard: the tone, the recording's playback, the buzz (the owner's, by hand, on the Developer page).

**The control queues' replies:** on the same night MIC_RECORD_AP_SET_PARAMS (224) came back as 22 bytes of type 0 with 103 in the id's place, and nothing else came. The service now takes the first type-0 message as the reply, as Linux does (hw.c:278-302), and says one whose id differs with its bytes; a type-1 notification is passed over. A capture whose start fails part way is stopped and its microphones powered off.

## The Sound page's tests (2026-11-22; owner: "Just throw a mic and speaker test in the ferros settings and I can test it", and earlier "sound and buzz, why not")

Settings → Sound, between the battery and the torch (payloads/map devtest.rs). On the Developer page, after the light scope, until 2026-11-24 (owner: "Not sure why that's under developer options and not sound", then "yes, add a sound page"). Nothing on it is stored; every control starts at its default when the map starts.

- **Speaker test.** A level slider (from 1 to `TONE_MAX`, 191: four octaves, 24 dB, under full scale; it starts at 111, nine octaves down, the first step of "A sound you can hear, quietly" below), and three buttons, left, right and both: a 1 kHz tone plays while a button is held and stops as the finger lifts or slides off it, faded in and out over a period (5.3 ms) so it does not click. The page says there is no speaker protection yet. Under the buttons, after every answer: what the service did (playing, stopped, refused, an amplifier's error) and each amplifier's PUP_DONE, PLL lock and error bits.
  - **The ceiling is the service's** (ferros_sys audio `TONE_MAX`, WHY in its comment): a tone or a playback above 191, or at 0 (which means the default volume, 223), is refused, never cut down. The tone also carries its own bound, 1024 periods (5.5 s, `TONE_HOLD`), so a map that dies between its start and its stop leaves nothing playing.
  - **An amplifier's error bit stops everything at once.** While sound plays the service reads both amplifiers' IRQ1_STATUS1 and IRQ1_STATUS3 every period (four SPI transactions, about 120 µs of each 5.3 ms period, clocked by the AoC's interrupt), and any of AMP_SHORT, BST_SHORT, TEMP, BST_OVP or BST_DCM_UVP stops the tone, every stream and the recording's playback, turns the source off and powers both amplifiers down. The process stays, silent, until the next call has been told (its report's last byte), then ends.
- **Microphone test.** "listen": each microphone's peak as decibels under full scale and on a meter, about twelve times a second (the meter's call held by the service, answered as the samples come in). Then a microphone chosen by id and "record, then play back": 2.7 s recorded, then played back at the speaker test's level; the page says the recording's peak.
- **Buzz test.** A waveform slider (the CS40L26's ROM waveforms 0 to G#26; it starts at 1, the notes' tap) and "buzz": one waveform thru the NFC driver (its op 8). The ROM's waveforms are unnamed and their strength unknown (the tuning, the RAM firmware's, is not applied); 1 is the one played in the motor's bring-up. NFC switched off by its status circle means no motor driver, and the page says so.
  - **No buzz until 2026-11-24 (the Pro, b1168).** The owner pressed it and felt nothing: the `nfc` capsule in the vault was from before 28b7f9e8 (the reset after a NACK), never stored after its hand test. The motor NACKs the transfer that wakes it, so every later transfer went out behind the unsent 4-byte address: op 7 read DEVID G#0, REVID G#0 and the ROM state G#40A260 (DEVID's value), and op 8's two mailbox writes landed elsewhere. With forge's nfc stored and restarted, op 7 read DEVID G#40A260, REVID G#A1, the ROM in standby, and op 8 (waveform 1) left the mailbox acknowledged (G#0), the ROM active (3) and EINT1 G#C00EC100 against G#80004100 before: bits 15, 17, 18, 19 and 30 newly set, none of the error bits 20 to 29 (Google's driver names none of the five; by the datasheet as remembered, not checked, the global enable, the power-up done, the boost's overvoltage flag up and down, and the mailbox written). Whether it was felt is the owner's to say.

The map reaches the service thru a send right on `audio` the kernel hands a Settings holder, the slot after the torch's (process.rs, 2026-11-22): a kernel from before leaves the slot empty and the page says the kernel needs updating.

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
- **Aemulus**, the emulator kist (secondary): genesis-kist branch `audio` (cb986d8, from before the rename; its worktree in /mnt/Harbor/tmp is gone), a cargo feature `audio`, off by default; GENESIS.md "Audio".

## Verification on the device (husky; the owner present for every step that makes sound)

The kernel carries the new line, switch clock, right and preference, so it needs the owner's flash (slot b, as usual) before any of this; the capsules first, as always: `tools/husky-shell.sh audio` builds, signs and stores the service (it is not started: the first call starts it), and `amp` and `epcall` must be current.

**Silent checks (2026-11-22): `PROBE` and the microphones.** Neither sounds: `PROBE` brings both amplifiers to ready without GLOBAL_EN and never starts the AoC's speaker port; a capture plays nothing.

```
ferros-bridge spawn epcall.bin 07 --name epcall --use audio     # PROBE: the services described, a control command answered, both amplifiers ready and back in reset
ferros-bridge spawn epcall.bin 08 --name epcall --use audio     # MIC_LEVELS: the capture started, the first 80 ms's peaks per microphone
ferros-bridge diag console
```

Expected from `PROBE`: `audio_output_control` and `audio_playback5` described, `control answered: entry point A#5 has taken A#… samples` (or the AoC's error code: still a round trip), both amplifiers' `ready` lines and `ready, off` with errors G#0, `probe done, nothing sounded`. From `MIC_LEVELS`: `audio_input_control` and `audio_capture0` described, the capture's channel and whether it is f1's, and a reply with three peaks (a room's noise gives small non-zero peaks; all zero means the AoC sends silence, its gains or its power). About 640 ms later `microphones stopped (nobody listening)`, `silent, ending`.

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

**4. A stream and the mixer.** Aemulus with `--features audio` (GENESIS.md "Audio"), at the default volume (level 223, two octaves down): its console line every 512 frames gives the frames paced by sound and the ring's lowest point (0 means a gap heard); the service's STATE gives underruns and starved.
Then the volume: `ferros-bridge diag "pref volume 207"` (an octave down from the default) while it plays, the change heard within a period (every period reads the preference).

**5. The end of it.** After the last stream closes or its kist ends, about 1.4 s plus 85 ms later the console says `audio: silent, ending`, `diag arcs` no longer lists `audio`, and the amplifiers' DEVID reads 0 from the `amp` tool (held in reset: the switch went off with the process).

**Not measured yet, and how:** the latency from a write to the air (a microphone capture of a click against the write's time; the AoC's microphones are the next service), the CPU the service takes (`diag sched`), and the AoC's own pipeline delay.

## The bench and the sweep (2026-11-24, measured on the Pro)

The owner, 2026-11-24, for waves and beams: "q removal. Less q's the better and smaller the buckets without getting stupid", "design up front, try options, experiment, benchmark, continue, repeat", "Like I want this creepy fast" (Photon docs/ferros-media.md).
Every number here is from the Pro under ferros, stamped with the AoC's own counts: GET_EP_TOT_SAMPLES at a run's start and end, the playback ring's byte counts, the capture's frames.

### The bench (`BENCH`, ferros_sys audio op 10)

A development op on `audio`, sent by `epcall` to a hand-spawned service (target/farm-style hand spawn, the service's doc line, plus `--dma test-mailbox` and `--input-file <sweep>` for the sweep).
The message: [10, lead, buffer, the period's log2, level, count (u16), spacing's log2, flags, capture ms, capture width, entry point, record soft gain dB, hardware gain dB, and optionally the clock thread's core mask].
Flags: bit 0 clicks, bit 1 the capture on, bit 2 the gains set, bit 3 the spawn input played once (a sweep) with the capture dumped into the test mailbox, bits 4 and 5 one speaker alone, bits 6 and 7 the record path (0 normal, 1 MMAP, 2 raw).
It brings the sound up in that shape, plays at a level never above `TONE_MAX`, and says on the console: the interrupts and the gaps between them, what the AoC took between two rings and what it left queued, its samples against 48 kHz times the time, the capture's reads, and for a sweep the dump.
A sweep's dump is held (the process stays) until the next call, because the kernel zeroes a mailbox's payload when its writer ends.

### What the AoC does with each entry point

The service played thru `audio_playback5` from 2026-11-11 to now, taken for Android's low-latency path from the boot log's bind of entry point 5.
It is not: the device tree's front ends (gs101-audio.dtsi:63-190, :426-440) and Android's paths (aoc_alsa_path.c:43-58, mixer_paths.xml:363-418) make entry point 5 `fe_ep6_rx`, deep-buffer-playback, 70 ms to the speaker in Android's own table (audio_platform_configuration.xml:80).

| Entry point (ring) | Android's path | The AoC's take | Sound |
|---|---|---|---|
| 5 (`audio_playback5`, 4 MiB) | deep buffer | 480 frames every 10.000 ms, whatever period it is told | yes |
| 1 (`audio_playback1`, 16 KiB) | low latency | 10 ms, whatever period it is told; with 128 frames queued it starved (148,224 samples in 11.6 s) | yes |
| 0 (`audio_playback0`, 15 KiB) | AAudio MMAP | the period it is told, rounded up to whole ms: 256 frames 6 ms, 128 frames 3 ms, 64 frames 2 ms, 32 or 16 frames 1 ms (48 frames a ring) | no: taken at its rate, as a plain ring and as Linux's MMAP way (the ring marked full, written ahead of the AoC's read place, 16-bit or float), silent at the microphones |
| 9 (`audio_raw`) | raw | its ring lies past the granted windows (G#D80000 in the IPC block) | not tried |

With whole-period top-ups the 10 ms take starved the deep-buffer path: 5.84 s of samples in 7.29 s (the first bench).
The service now tops the ring up to its lead in frames, a period at most at a time: 148,320 samples taken against 148,031 expected.
At a 1 ms block (entry point 0, 32-frame period) with 64 frames queued, 0 underruns in 3 s: 148,224 against 148,144.

### The sweep (the owner, 2026-11-24: "Doesn't a sweep give you a better idea of device audio latency?")

Farina's exponential sine sweep (tools/sweep-ir: 2^17 frames, 100 Hz to 20 kHz, faded 2^8 frames at each end) played once at `TONE_MAX`; all three microphones captured; each deconvolved by the sweep on the host (Y·X*/(|X|² + ε), ε 60 dB under the sweep's strongest bin, by FFT, f64).
The impulse's peak, placed to a fraction of a frame by a parabola, is the arrival; the capture's read anchors (frames in, counter) turn it into the counter's time, and the latency is from the sweep's first frame written into the AoC's ring.
The harmonics fall ahead of the main impulse and are read off too; so is the response in octave bands.

```
tools/sweep-ir/target/release/sweep-ir gen sweep17.raw 17
target/farm/audio/sweep.sh <BENCH hex with flags bit 3> dump.bin     # spawn, bench, read the frame `diag arcs` names, release, analyse
```

| Path (playback, record) | Write to arrival | Of it, queued ahead | Notes |
|---|---|---|---|
| EP5 deep buffer, normal record (1 ms) | 34.24 to 34.32 ms | 0.67 ms | h2 -29 to -48 dB |
| EP1 low latency, normal record | 34.98 to 35.07 ms | 0.67 ms | |
| EP5, normal record at 10 ms periods | 34.75 to 34.83 ms | 0.67 ms | the capture's period does not move it |
| EP5, normal record 32-bit (width code 3) | 34.79 to 34.87 ms | 0.67 ms | the AoC carries 32-bit capture |
| EP5, raw (low-latency) record, 2 channels | 34.85 ms | 0.67 ms | the record path does not move it |
| EP5, MMAP record (`audio_capture1`) | none | | the ring fills at 1 ms with zeros |
| EP0 MMAP playback | none | | silent |

So about 34 ms of the loop is inside the AoC, the same for every working playback and record path: its speaker chain or the microphones' common front end, not the rings.
Splitting play from capture needs one side of known delay; both MMAP sides, which Android's AAudio uses for its lowest latency, are silent here, so the next step is what Android's HAL sends for an MMAP stream (its userspace sends commands the kernel tree does not show: an oracle capture of the AoC's control queues while an AAudio MMAP stream plays, on slot b's Magisk root).

### The microphones' places, measured

One speaker at a time (flags bits 4 and 5): the left channel (cs35l41@0) reaches microphone 2 first at -45 dB and microphone 1 1.4 cm later at -48 dB, microphone 0 only at -68 dB; the right channel (cs35l41@1) reaches microphone 0 first at -48 dB and 1 and 2 at -69 and -72 dB, 18 frames (12.6 cm, about the device's length) later.
Two microphones sit at the top and one at the bottom, so microphone 0 is the bottom (voice) microphone, the left channel the top (earpiece-side) speaker, the right the bottom speaker; 1 and 2 by the HAL's own order (bottom, back, top) and 2 being nearer the earpiece: 1 back, 2 top.
A tap test by the owner settles 1 against 2. The AoC's default gains on a cold AoC: hardware 0 cB, record soft gain 0 dB.

### The AoC's line, interrupt to run (`diag dev`, aoc-mbox-f1, INTID 110)

Idle: mean 24 to 36 us, longest 66 to 383 us; the line lands on the X3 (G#100) and its waiter runs on the A715s (G#F0).
Under load and pinned to an A510: to be measured (Lumis pins its intake to cores 0-2 and its camera thread to core 3).

### Proposals for the owner (nothing of these is built)

- **A mic right.** Today any kist holding `audio` (implicit for every kist) can call `MIC_LEVELS` and `RECORD`, and would read the planned `mic` mailbox. Proposed: `mic`, a right of its own in the kists' vocabulary, asked once per person per kist (permissions_asked_once), the proof for SYS_MAILBOX op 0 on `mic` and for `MIC_LEVELS` and `RECORD`; the Sound page's tests keep it as a Settings holder. Until the owner decides, the proof is the `audio` right, swapped in one place.
- **The capture runs only while someone holds a view.** A kernel-kept count of views (a header word, +1 at an open, -1 at its process's end) the service reads every period: at 0 the microphones stop, so a kist that dies cannot leave them on, and the same count is the mic-in-use indicator.
- **Gain policy.** The record soft gain (block 136, component 30, key 16, whole dB) and the hardware gain (cB) are the service's, never a kist's: one setting per use, a call's voice and a recording at Android's measured values (soft 0 dB, hardware 130 cB for voice; 5 dB unprocessed), the level meter at what a recording uses, the defaults stored as device settings; a kist gets the samples at that gain and scales its own copy. The calibration session (the owner's measurement microphone) turns them into dB SPL.

### What Android sends, and the MMAP path on ferros (2026-11-24, the Pro)

Captured under Android with tracefs kprobes on `aoc_service_write` (tools/aaudio-bench), the HAL's AAudio MMAP streams opened by tools/aaudio-bench's NDK test:
- **Playback (entry point 0):** CMD_MEM_MAP_REGION (212) on the AoC's `control` service maps a 16 KiB buffer in the playback heap (AoC G#9ABF_C000, AP G#96BF_C000), then BIND2 (0 to the speaker), the speaker's tuning (11,272 bytes into the ring `audio_bulk_tx`, then SET_GET_PARAM_BULK 213 on `audio_output_tuning`: block 16, component 2), EP_SETUP (two channels of 32-bit float), EP_SETUP2 (the buffer the whole 15,360-byte ring, the period 384 bytes, 1 ms), SOURCE; at the end SOURCE off, BIND2 off, the buffer unmapped.
- **Record (`audio_capture1`, device 9):** the capture heap's 64 KiB mapped (AoC G#9ABE_C000, AP G#96BE_C000), the record soft gain 5 dB, MIC_RECORD_AP_SET_PARAMS (one channel, 32-bit), SET_PARAMS2 (the buffer the whole 46,080-byte ring, the period 192 bytes, 1 ms), the microphone list, AP_INPUT_START, MMAP_ENABLE_2 (one channel).
- The samples of an MMAP stream are in the mapped heap, never in the IPC ring, which carries only the counts: why entry point 0 played nothing and `audio_capture1` stayed empty until ferros mapped the heaps (kernel `aoc-heaps`, forge ebfc18ee).
- The tuning only shapes the sound: entry point 0 with the heap mapped and no tuning plays at the same level as entry point 5. The tuning is computed by the HAL (it is in no vendor file verbatim; its id 5 follows Android's volume); the Waves preset in the vendor tree is its likely source.

| Loopback, the same sweep and tool | Write to arrival | Of it, queued |
|---|---|---|
| Android, AAudio MMAP (96-frame bursts) | 21.5 ms | 5.3 ms |
| Android, legacy (raw entry point 23) | 88.1 ms | 21.3 ms |
| ferros, entry point 5, normal record | 34.2 to 34.8 ms | 0.67 ms |
| ferros, entry point 0 MMAP (heap), normal record | 19.3 ms | 0.33 ms |
| ferros, entry point 0 MMAP, MMAP record (heap) | 10.7 to 19.0 ms over four starts | 0.33 ms |

The spread from one start to the next (about 8 ms) is under study: where the AoC reads the heap against where the service writes.
**Clicks** (the owner heard them): with 64 frames (1.33 ms) of lead, the clock thread's wake-up tail (up to 4 ms, unpinned on the A715s) let the AoC read the zeroed heap: 2 to 3 holes of 3 to 5 ms in every 2.7 s of tone, each at a late turn. The lead must cover the wake-up's tail, or the tail must shrink (KERNEL.md's scheduling proposal).

## Where audio stands (2026-11-24, end of the audio session)

**The Pro:** forge ebfc18ee RAM-reloaded (boot b1178; the identity unlocks at boot), `dev follow` off as found. No audio process runs. The vault's stored `audio` capsule is older than this session's work (from before 59abc9f6: entry point 5, the whole-period top-up that starved): the kernel's on-demand start (the Sound page, Photon's WRITE) still gets that one until `tools/husky-shell.sh audio` stores forge's. Everything measured today ran from hand spawns (target/farm/audio/bench.sh and sweep.sh: the doc line in main.rs, plus `--dma aoc-heaps --dma test-mailbox --input-file`).

**What works, measured:** entry point 0 (MMAP) plays from the playback heap and `audio_capture1` records into the capture heap at 1 ms blocks; speaker to microphone 10.7 to 19.0 ms against Android's 21.5 ms; no holes with a 2 ms lead since the amplifiers' watch has its own thread (051e07d0). Entry point 5 stays the default shape.

**Next, in order:**
1. The watch's SPI read still takes up to 2 ms (4.5 ms seen): its cause, ferros_spi's wait against a kernel lock on the mmio path, by measurement.
2. The lead toward 1 ms and the clock thread's pinning, measured again now the turn is 76 us (pinning was worse only while the watch stalled the turn); Lumis pins cores 0 to 3.
3. The about 8 ms spread from one MMAP start to the next: where the AoC reads the heap (Linux counts from rx at prepare) against where the service writes (from rp).
4. The split of the loop into its speaker and microphone halves: the amplifiers' V/I sense on TDM0's TX as an electrical reference; Android's stage list; the owner's calibrated microphone at the calibration session.
5. The service's default to entry point 0 with the heaps, then the shared playback ring (SYS_SHARE, the network agent's) and the `mic` mailbox (its kernel part unwritten: the grant, the op-0 proof, the view count), the APIs drafted for Photon.

**Open, the owner's:**
- A `mic` right of its own (today any kist with `audio`, which is implicit, can record).
- The gain policy (the record soft and hardware gains set by the service per use; Android's values as the start).
- Audio's priority in the scheduler (a real-time class or a core for device threads), with the numbers above.
- The speaker tuning's source: Android's HAL computes it (11,272 bytes, block 16, component 2; its id 5 follows the volume), most likely from the vendor partition's Waves preset; entry point 0 plays without it. Reading it from the phone's own vendor partition (partfs) is the recommendation, decoding the preset the last resort.
