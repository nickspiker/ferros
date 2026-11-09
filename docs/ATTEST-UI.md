# The attest screen, laid out — a proposal (2026-11-02, for the owner's decision)

What the glass shows between power-on and the map: the attest screen (payloads/ahau), the corners (the display server's chrome), and the two things a device with no identity yet must be able to do before anyone attests: pick a language and reach a network.
Nothing here is built yet beyond "what exists". The decisions are at the end.

## What exists

- **The attest screen** (ahau): the mark (tapuwae, 900 px), the handle line under it (typed in the clear, the bar's left key clears), the honeycomb keyboard in the device's layout. A first entry is not a claim: fgtw (the identity service) looks the handle up (the fleet's devices nearby first, then fgtw.org); minting asks for the handle twice, visibly. Optional unlock-at-boot stores the seed.
- **The corners**: the four corners outside the app circle belong to the system (WINDOWS.md). Only the clock is drawn, top left, in the device's base, redrawn once a minute. Signal and navigation are noted as "later".
- **Facts** a process can read (SYS_FACTS): boot-to-home time, battery (%, temperature, voltage, current and its sign), USB link, processes alive, keys in the vault, the wall clock and zone, the hottest thermal zone and whether throttled, gravity, the spectral and proximity samples, how the boot began. **Not there:** the radios' link state.
- **WiFi**: the driver joins at boot from the vault's credential (the person's `wifi.cred`, else the device's `wifi.net`; a VSF document) and serves Ethernet frames on `wlan`. Before any join it serves the network page: SCAN, JOIN and STATUS (below).
- **Settings** (SETTINGS.md): base, labels, brightness, dither, theme, prox_wake, scope, layout, unlock_at_boot, tz. **The language is not a field yet:** it joins as its own VSF type (ISO 639-3 packed, with script and region), which vsf does not have today (its text encoding knows scripts, not a language tag).
- **First boot** (SHELL.md): "pairs with a device you trust before anything else. On the map that is the one circle there is."

## The screen

One screen, no modes, nothing animated or timed. Top to bottom:

1. **The corners** (the chrome, drawn by the display server, present on every screen; since 2026-11-02 the server composes windows as a layer stack, WINDOWS.md "The layer stack", and the corners are its topmost layer):
   - Top left: the time, as now.
   - Top right: the radios, as four dots in a row: WiFi, cellular, Bluetooth, NFC. A dot is lit (the theme's "changed" colour, the map's only colour) when its link is up, the text colour when the radio is on but not linked, faint when off. No bars, no numbers: a dot says up, on, or off, and nothing shouts. After a jump from Android the radios are off, and the dots say so.
   - Bottom left: the battery, a ring swept like the clock's rings: the fill is the charge, the ring's colour the text colour, the lit colour while charging (current positive). Below a sixteenth it is drawn in the warn colour; that is the one exception to "nothing shouts", and it is the owner's call.
   - Bottom right: home, the one navigation control (WINDOWS.md, "The corners": the map is home, recents and search; back is the app's; a tap here from anywhere and the map is there). Proposed 2026-11-02, nothing built.
2. **The mark**, as now, centred.
3. **The handle line**, as now.
4. **A line of words under it**, in the device's language, saying what this screen is: "Type your handle" when an identity has attested here before; the first-use words (below) when none has.
5. **The keyboard**, as now.

Every size derives from the span (the harmonic mean of width and height) or the screen: the dots' radius span/128, their spacing span/32, the battery ring's radius the same as the clock's rings. Colours come from the theme (ferros_sys::theme), nothing hard-coded.

## Indicators: where the facts come from

The display server owns the corners and already reads the facts once a second for the clock. The battery and charging need nothing new. The radios need a link-state word the kernel does not have:

- **Proposal:** `ip` publishes its links the way the sensors process publishes its samples: one word, bit 0 WiFi associated, bit 1 WiFi has an address, bit 2 cellular registered, bit 3 cellular has an address; Bluetooth and NFC publish their own bit each (bit 4 Bluetooth on, bit 5 NFC on) from their driver processes. The kernel answers it as fact 30. Each driver sets its bits when they change, not on a timer.
- **Why not polling the drivers:** the corners would then depend on four processes answering calls every second; a published word costs nothing and survives a driver that is busy.

## Language

The words on the attest screen are the first words a person sees, so the language is the first setting, before any identity.

- **Where it lives:** the device document, as every device setting does; an identity's own language overrides it once it attests and is written back to the device (SETTINGS.md's one direction).
- **The type:** the VSF language type (ISO 639-3 + script + region, all numbers), as decided on 2026-10-27. **Blocked on vsf.** Two ways through:
  - (a) wait for the type, and build the screen with the language fixed to one until then;
  - (b) an unsigned field `language` in the device document holding the packed ISO 639-3 code now (a number, not text; no new format), replaced by the VSF type when it exists, the field dropped the same day.
- **The picker:** on the device circle's back with the other settings, and once more on the attest screen at first use: a row of the languages the shell has words for, each written in itself. No flags.
- **The words:** one table per language in the shell, the strings' keys unsigned (count from zero). The shell starts with the owner's languages; adding one is adding a table.

## First use

A device with no identity in its vault. The order is forced by dependencies: the words need a language, the lookup needs a network, the identity needs the lookup.

1. **Language.** The row of languages, each in itself. One tap. Nothing else on the screen but the mark.
2. **Network.** The WiFi dot and, under the mark, the networks the chip can see, strongest first, each with its name and a dot lit for "known". A tap on one asks for its passphrase on the keyboard; the credential goes to the device's vault (`wifi.cred`, as now) and the driver joins. Cellular needs nothing typed: if a SIM is there and registers, the dot lights by itself. A device with no network can still mint, and attest later.
3. **The handle.** The line and the keyboard, as now, with the first-use words: "No one has attested on this device yet. Type your handle, or a new one to mint." A known handle attests (the fleet lookup); an unknown one mints, asked twice.
4. **Pairing.** After a mint, or an attest from a fleet that has other devices: "Confirm on a device you trust." The screen shows this device's mark and waits for the fleet's answer (Photon's bind). Nothing is timed; the screen stays until the answer or the person leaves it.

Each step is the same screen with different words under the mark. A person who has done this once never sees steps 1, 2 or 4 again unless they ask (the device circle's back).

## WiFi connect: the driver's requests (built 2026-11-09)

pcie1 serves `wlan` from the moment its chip is up, before any join. Beside 1 SEND, 2 RECV and 3 the MAC:

- **4 SCAN:** `[4, 1]` starts a scan unless one runs; `[4]` reads the list. The reply is `[scanning, count]`, then per network, strongest first: name length, name, signal (dBm, a signed byte), flags. The flags: bit 0 joinable here (WPA2-PSK outside 6 GHz), bit 1 open, bit 2 other security, bit 3 seen on 5 GHz, bit 4 the network being joined or kept. The page asks for one scan and polls; the last list stays until a new scan ends. While joined, no scan runs and the list is the one from before the join.
- **5 JOIN:** `[5, name length, name, passphrase]`. The driver keeps the credential, replies `[0, class]` (class 0 the person's, 1 the device's), and then joins. A name or passphrase that is not usable ASCII (name 1-32 bytes, passphrase 8-63) gets `[1]`, a vault that refuses gets `[2]`, and a JOIN while joined gets `[3]`. The page is not answered during the join itself (up to about 15 s); it reads the outcome from the corners' link state, then STATUS.
- **6 STATUS:** `[state, outcome, name length, name]`. The state is 0 waiting for a network, 1 scanning, 2 joined. The outcome of the last join is 0 none, 1 no WPA2-PSK network of that name in range, 2 refused (a wrong passphrase among the reasons), 3 timed out, 4 joined.

**The credential** is a VSF document (ferros_detail `network.rs`: section `network`, `name` and `passphrase` as ASCII text). It replaced the two-line text file; a stored value that is not this document is dropped. Decision 3 as built:
- **Who keeps it:** the person's `wifi.cred` when someone has attested; before that the device's `wifi.net` (device class, readable from boot). Moving `wifi.net` into the person's on attest is next.
- **A credential that fails:** a credential handed in that does not join is deleted again, so a mistyped passphrase is not kept. A kept one whose network is out of range is looked for again every 64 s.
- **The MAC:** the person's networks get their per-network MAC from the ira. A network joined before anyone attests gets a random MAC at each join (AUTH.md, rule 2).
- **Nothing on the air unasked:** with no credential, the radio scans only when the page asks.

`ferros-bridge wifi-cred <in.txt> <out.vsf>` makes the document from a host text file (name line, passphrase line), for the staged scripts' input file. tools/husky-wifi.sh and husky-net.sh do it themselves.

## Decisions

0. **The corners as proposed** (radios top right as dots, battery bottom left as a ring, bottom right empty)? Or the battery as a number in the device's base?
1. **The low-battery warn colour**: the one thing allowed to shout, or nothing?
2. **Language now:** (a) wait for the VSF type, or (b) the unsigned field until it exists?
3. **The credential's home before an identity:** built 2026-11-09 as the device's `wifi.net`, under a random MAC (above); the move into the identity on attest is still to build.
4. **The first-use order** as above (language, network, handle, pairing)?
5. **Cellular on the attest screen:** a dot only (registers by itself), or a way to choose a network?

## Build order, once decided

0. The link-state fact and the corner dots (display server + ip, bt, nfc: a word each). Verifiable from the bridge.
1. The battery ring.
2. The words table and the language setting (its mechanism per decision 2), the picker on the device's back.
3. The WiFi driver's SCAN, JOIN and STATUS, tested from the bridge (done 2026-11-09, on akita).
4. The first-use screens in ahau, in the order above.
