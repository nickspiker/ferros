# Settings

What a person sets on a ferros device, where it is kept, and which way it is allowed to flow.

## Device and identity

A setting lives in one of two places:

- **The device** — the device's vault, in its settings document (below). It is what this device does for whoever holds it.
- **The identity** — the person's preference, carried with their identity across their fleet (Photon's fleet-wide `display.base` is one).

The two meet in one direction only. **When an identity attests on a device, the identity's preference is written to the device.** Nothing ever goes the other way: a device setting is never read and written into an identity's preference, whatever its value. A device can be set by anyone holding it; an identity's preferences are that person's own, and a device that someone else configured must not be able to change them.

A device setting sticks until it is set again — by the person holding the device, or by the next identity that attests.

## How the device keeps them

One spec VSF document in the vault under the key `device` (ferros_kernel `settings.rs`, since 2026-10-27; owner: "All device settings should be proper spec VSF for aid in vault storage"): a section `device` with a typed field per setting, stamped with its Eagle time and a provenance hash. It replaced the raw one-byte `device.*` keys, which nothing reads any more.

| Field | Type | Values | Absent |
|---|---|---|---|
| `base` | unsigned | tatau's code: 10 arabic, 12 DMS, 16 hex, G#8C DLS | DMS |
| `labels` | unsigned | 0 off, 1 on | 0 |
| `brightness` | unsigned | 32nds of a stop below the brightest white, 0-255 (whole stops 0-8 until 2026-11-08) | 2 |
| `dither` | unsigned | 0 spatial and temporal, 1 spatial only | 0 |
| `theme` | unsigned | an index into ferros_sys::theme::THEMES | 0 |
| `prox_wake` | unsigned | 0 off, 1 on | 0 |
| `scope` | unsigned | 0 off, 1-6 the light scope's channel | 0 |
| `layout` | unsigned | 0 Dvorak, 1 QWERTY with Q on the bottom row (honeycomb) | 0 |
| `unlock_at_boot` | unsigned | 0 off (the handle is typed each boot, nothing of the identity on disk), 1 on (the seed is stored so the phone unlocks without typing) | 0 |
| `radios_off` | unsigned | the radios switched off, a bit each: 0 cellular, 1 WiFi, 2 Bluetooth, 3 NFC, 4 GPS (a tap on a status circle; the kernel stops that driver while its bit is set) | 0 |
| `radios_before` | unsigned | the radios that were off when the quick row's "radios off" switched them all off (`radios_off`'s bits), put back by its next tap (QUICK.md) | 0 |
| `sound` | unsigned | the sound mode, preference 39: 0 sound, 1 vibrate, 2 silent (the quick row) | 0 |
| `dnd_identity` | unsigned | do not disturb, preference 40, as the identity's service last wrote it: 0 off, 1 on; never read back into the identity (below) | 0 |
| `torch_level` | unsigned | the level the torch's tap lights it at, preference 42: 1-255 on its perceptual dimmer, set by a pull from its quick circle or its page (TORCH.md "Settings") | 0: the whole dimmer |
| `torch_strobe` | unsigned | the torch's strobe, preference 43: 0 steady, 1-193 32nds of an octave up from a flash every 4 s (16 a second at 193) | 0 |
| `torch_duty` | unsigned (a signed byte) | the lit share of each strobe flash, preference 44: 256ths either side of a half, within 120 | 0: a half |
| `torch_timeout` | unsigned | when the torch turns itself off, preference 45: 0 never, 1-161 16ths of an octave up from 16 s (2^14 s at 161), counted again from every touch of it | 0 |
| `volume` | unsigned | the sound's volume, preference 46: 1-255 dimmest to loudest, a 16th of an octave of amplitude a step, 255 full scale (AUDIO.md "Volume") | 0: two octaves under full scale |
| `tz` | signed | the fixed offset, seconds east of UTC: used when no zone is in effect | 0 |
| `zone` | ASCII | this device's zone by hand, an IANA id (docs/TIME.md) | none |
| `zone_auto` | unsigned | this device's choice: 1 automatic (the zone found from the cellular network, then GPS; `zone` stands in until one is), 0 by hand | 1 |
| `zone_follow` | unsigned | 1 the device follows the identity's zone choice, 0 it keeps its own | 1 |
| `zone_identity_auto`, `zone_identity` | unsigned, ASCII | the identity's zone choice as its identity service last wrote it (present once it has); never read back into the identity | none |
| `zone_found`, `zone_found_by` | ASCII, unsigned | the zone last found here, and by what: 1 the network, 2 a position | none |
| `wifi_power_save` | unsigned | WiFi power save: 0 off, 1 on (PM1), 2 fast (PM2); a radio knob (RADIO-CONTROLS.md), set on the WiFi page | unset: off |
| `wifi_band` | unsigned | the band WiFi joins on: 0 any (5 GHz first), 1 5 GHz only, 2 2.4 GHz only; takes at the next join | unset: any |
| `language` | language (`wl`) | this device's own language, its ISO 639-3 code (no script or region) | none: English |
| `language_follow` | unsigned | 1 the device follows the identity's language, 0 it keeps its own | 1 |
| `language_identity` | language (`wl`) | the identity's language as its identity service last wrote it; never read back into the identity | none |

- **Loaded** by the kernel right after the vault opens: the defaults, then every valid field, into every process's preferences block.
- **Written whole** on every change: a process holding Settings (`SYS_SET_PREF`) or a radio knob or the zone (`SYS_CONTROL`, docs/RADIO-CONTROLS.md; the clock page sets the zone a quarter hour at a time), `ferros-bridge diag "pref <name> <value>"`, or a `device` document stored from the bridge (applied at once).
- **The language** is kept as a language (vsf `wl`), as the zone is kept (2026-11-10, below).

## The language (`device.language`)

The shell speaks Photon's sixteen languages (English, Español, Māori, Português, Français, Bahasa Indonesia, Deutsch, Kiswahili, Tiếng Việt, Türkçe, Русский, हिन्दी, Українська, Italiano, Polski, Filipino), from one list shared with Photon (fgtw::text `Lang`, Photon's storage order and autonyms).

- **Two choices, as the zone.** The identity's (the person's space, ferros_space `LANG`, written to each device by its identity service, payloads/fgtw) and this device's own. The device follows the identity's while "follow my identity" is on (the default) and the identity has one; else its own. The kernel works out the one in effect (lang.rs) and knows no list: a language is its ISO 639-3 code packed as `wl` packs it, so a language the shell cannot show is kept and reads as English.
- **Set** on the settings' language page (payloads/map): while following, a pick is the identity's (fgtw op 23: into the space, to every device that follows it); else this device's (SYS_LANG op 0). `ferros-bridge diag "pref language <code>"` (`eng`, `mri`, … or `none`) and `"pref language_follow 0"` on the bench.
- **Read** by every process from its second preferences block (preferences 32-38 at G#FB0: in effect, this device's, the identity's, the follow switch), at draw time (ferros_lang `current`).
- **The words** are ferros_lang's (the settings pages, the attest screen, the keyboard) and fgtw::text's (the fleet's and identity's, shared with Photon, in Photon's wording where Photon has the message): whole messages with holes, every number in the person's base.
- **What the display draws.** One face, Oxanium +glyphs, by its character map with no shaping (measured with fc-query, 2026-11-10): Latin-1 and most of Latin Extended-A. Twelve languages draw whole. Russian and Ukrainian (no Cyrillic), Hindi (no Devanagari, and it needs shaping) and Vietnamese (no ơ ư, no precomposed tone letters, and no combining hook above, dot below or horn) do not: they are carried in full, are not offered on the page, and read as English while they are the setting. They need a face that covers them (and, for Hindi, shaping); a font is the owner's decision.

## Do not disturb (the identity's)

Do not disturb is the person's, not the device's (owner, 2026-11-11: "a fleet/identity preference so it follows the person"). No device keeps its own.

- **Kept** in the person's identity space (ferros_space `DND`), switched from the quick row of any of their devices: the map asks the identity service (payloads/fgtw op 24), which puts it in the space, hands it to their fleet and fgtw.org, and writes it to the kernel.
- **On each device** the identity service writes it to the kernel whenever it changes (preference 40, a call the kernel takes from the process named fgtw alone, as the language's), and the kernel keeps it in the settings document as `dnd_identity`, so the device keeps the person's last word thru a boot before the space is read. A Settings holder cannot set it.
- **Read** by every process from its preferences (`ferros_sys::dnd::on`). Nothing silences anything by it yet: what it holds back is decided when notifications exist (Photon).

## The sound mode (`device.sound`)

Sound, vibrate or silent, preference 39, a device setting set from the quick row thru the Settings capability and kept as every device setting is. Nothing plays a sound or a buzz by it yet (the audio and haptics drivers come first: AUDIO.md).

## The number base (`device.base`)

Every number a person reads is drawn in one of four bases (`Code/tatau`): arabic (decimal, with unit words), hexadecimal (linear, for coders), DMS (dozenal magnitude: doublings) and DLS (dozenal linear: the value with its sign arrow and SI unit, `↑Zil·Lun Zil Zil s`). Time of day and dates read the same in DMS and DLS.

- **Kept** as the settings document's `base`: tatau's code, 10 arabic, 12 DMS, 16 hex, G#8C DLS. Absent means DMS, the house base.
- **Loaded** by the kernel right after the vault opens, into tatau's process-wide base, so everything drawn from then on reads in it (`ferros_kernel/src/main.rs`).
- **Set** by the person on the device — the device's own circle on the map, the settings page (SHELL.md), where each base is a row showing two seconds and a mebibyte written in it, and a tap chooses — or `ferros-bridge diag "pref base <code>"`, or by an attesting identity's preference.
- **Drawn** as glyphs only where the font is ours (fluor with the Oxanium `+glyphs` face: the shell and the apps) and as words everywhere else. Terminals, bash and anything else without font control are not a supported surface for glyphs.

## Names under circles (`device.labels`)

Whether the map writes a circle's name under it: one byte, 0 off (the default: icons identify) or 1 on, preference 1 in every process's block. Set from the same back of the device's circle, kept and applied exactly as the base is.

## Brightness (`device.brightness`)

Brightness is a whole number of stops below the brightest white, 0 to 8, one byte, preference 3 (absent = 2). It is set on the device's back as a row of nine dots, dimmest on the left.
- **Step 0** puts white at half the panel's peak: the owner's "max white one stop under max". On this unit's E-white peak of 1178 cd/m², that is 589 cd/m².
- **Each further step halves white:** 294, 147, 73, 37, 18, 9, 4.6 and 2.3 cd/m².
- **The panel's own brightness** stays at its high-brightness peak while that peak is within 16× of white, the headroom the working format holds. Below that, it comes down so its peak is exactly 16× white, set from the measured DBV table.
- **The display server** rebuilds its one table (tanh into the peak, white exactly on target) and republishes the frame when the preference changes. On the X3 the rebuild takes about a millisecond.

Measured on the phone, 2026-10-22:

| Step | White | E peak | Panel |
|---|---|---|---|
| 0 | 589 | 1178 | HBM |
| 2 | 147 | 1178 | HBM |
| 5 | 18 | 294 | DBV 2126 |
| 8 | 2.3 | 36 | DBV 831 |

## How a process sees and changes them

- Every process reads the current values from its own grants page (`ferros_sys::device_base`, `ferros_sys::pref`): the kernel rewrites that block in every live process the moment a value changes.
- Changing one takes the Settings capability (`ferros_sys::set_pref`): init holds it and passes it to the map, whose device circle is where settings live.

Developer output keeps its own convention (`G#` hexadecimal, `A#` decimal, CLAUDE.md): it is read by developers in terminals and logs, not by the person holding the device.
