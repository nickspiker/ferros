# The attest screen, laid out — a proposal (2026-11-02, for the owner's decision)

What the glass shows between power-on and the map: the attest screen (payloads/ahau), the corners (the display server's chrome), and the two things a device with no identity yet must be able to do before anyone attests: pick a language and reach a network.
Nothing here is built yet beyond "what exists". The decisions are at the end.

## What exists

- **The attest screen** (ahau): the mark (tapuwae, 900 px), the handle line under it (typed in the clear, the bar's left key clears), the honeycomb keyboard in the device's layout. A first entry is not a claim: the Photon client looks the handle up (the fleet's devices nearby first, then fgtw.org); minting asks for the handle twice, visibly. Optional unlock-at-boot stores the seed.
- **The corners**: the four corners outside the app circle belong to the system (WINDOWS.md). Only the clock is drawn, top left, in the device's base, redrawn once a minute. Signal and navigation are noted as "later".
- **Facts** a process can read (SYS_FACTS): boot-to-home time, battery (%, temperature, voltage, current and its sign), USB link, processes alive, keys in the vault, the wall clock and zone, the hottest thermal zone and whether throttled, gravity, the spectral and proximity samples, how the boot began. **Not there:** the radios' link state.
- **WiFi**: the driver joins at boot from the vault's `wifi.cred` (two lines: SSID, passphrase) and serves Ethernet frames on `wlan`. It scans to find the credential's network. It has **no runtime op** for scanning on request, joining a new network, or reporting its state.
- **Settings** (SETTINGS.md): base, labels, brightness, dither, theme, prox_wake, scope, layout, unlock_at_boot, tz. **The language is not a field yet:** it joins as its own VSF type (ISO 639-3 packed, with script and region), which vsf does not have today (its text encoding knows scripts, not a language tag).
- **First boot** (SHELL.md): "pairs with a device you trust before anything else. On the map that is the one circle there is."

## The screen

One screen, no modes, nothing animated or timed. Top to bottom:

1. **The corners** (the chrome, drawn by the display server, present on every screen):
   - Top left: the time, as now.
   - Top right: the radios, as four dots in a row: WiFi, cellular, Bluetooth, NFC. A dot is lit (the theme's "changed" colour, the map's only colour) when its link is up, the text colour when the radio is on but not linked, faint when off. No bars, no numbers: a dot says up, on, or off, and nothing shouts. After a jump from Android the radios are off, and the dots say so.
   - Bottom left: the battery, a ring swept like the clock's rings: the fill is the charge, the ring's colour the text colour, the lit colour while charging (current positive). Below a sixteenth it is drawn in the warn colour; that is the one exception to "nothing shouts", and it is the owner's call.
   - Bottom right: empty. Navigation is not built, and this proposal leaves it so.
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

## WiFi connect: what the driver needs

Three requests on `wlan`, beside the two it serves now (1 SEND, 2 RECV):

- **3 SCAN:** start a scan; the reply is the list so far (name, strength, security, known), repeatable until the scan ends. The driver already walks escan results to find its credential's network; this returns them instead.
- **4 JOIN:** a name and a passphrase; the driver writes the credential to its vault scope and joins. The reply is the join's outcome as the driver already decodes it (associated, keyed, failed with the reason).
- **5 STATUS:** the link's state, the network's name, the strength.

Where the credential lives is a decision (below): the device's scope before an identity exists; the identity's once one attests (WiFi pairings move with the identity, the auth-flow decisions of 2026-10-31).

## Decisions

0. **The corners as proposed** (radios top right as dots, battery bottom left as a ring, bottom right empty)? Or the battery as a number in the device's base?
1. **The low-battery warn colour**: the one thing allowed to shout, or nothing?
2. **Language now:** (a) wait for the VSF type, or (b) the unsigned field until it exists?
3. **The credential's home before an identity:** the device's `wifi.cred` (as now), moved into the identity on attest?
4. **The first-use order** as above (language, network, handle, pairing)?
5. **Cellular on the attest screen:** a dot only (registers by itself), or a way to choose a network?

## Build order, once decided

0. The link-state fact and the corner dots (display server + ip, bt, nfc: a word each). Verifiable from the bridge.
1. The battery ring.
2. The words table and the language setting (its mechanism per decision 2), the picker on the device's back.
3. The WiFi driver's SCAN, JOIN and STATUS, tested from the bridge.
4. The first-use screens in ahau, in the order above.
