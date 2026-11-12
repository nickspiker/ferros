# Radio controls

Every link's numbers, names and knobs, from its driver to its page in settings (owner, 2026-11-07: "Be able to turn every knob our radio has, frequency lock, tech, etc. carrier stats, all the goodness. same with the wifi's, gps's, clock, USB. All the things.").

The console's circles stay orbs without words (they draw the measures); a link's page in settings is the only place with numbers and knobs, reached by dragging its circle toward the centre (or tapping a circle that is no switch: USB, the clock, the battery), or from Connections. Every pixel of the dash is some circle's (QUICK.md "The dash's zones").

## Three ways out of a driver, one way in

A driver says what it knows three ways, each kept by the kernel, which reads none of it:

- **Measures** (8 a link, `SYS_STATUS` slots 0-7, facts 33-80): tagged numbers, the kind in bits 48-63 and a signed value in its unit (`ferros_sys::measure`). No capability to read: they carry nothing that locates or names the person. The orbs draw these.
- **Tracks** (64 a link, `SYS_STATUS` slots 8-71, read with `SYS_TRACKS` 31): more tagged words of the same form, as many as the link has. The GPS driver's satellites (`measure::SAT`), and every other link's further measures and counters. No capability, the same rule as the measures.
- **The detail** (`SYS_DETAIL` 34): what is text or an identifier, never smuggled into a number: a complete VSF document (header, Eagle time, provenance hash) with one section named for the link and a typed field each: ASCII text `a` for names, `ni`/`nj` for IPv4/IPv6 addresses, `nm` for hardware addresses, unsigned for the rest. At most 1 KiB a link. The schemas live in one place, the crate `ferros_detail` (kept out of ferros_sys so a process without a heap never links alloc), shared by the driver that writes and the page that reads; the reader verifies the provenance hash and refuses unknown fields. **Reading it takes the Settings capability**: a network's name, an access point's address, the line's IP addresses say where the person is and who they are, so only the settings app (the map) sees them, never any process that asks.

And one way in:

- **The controls** (16 words a link, `SYS_CONTROL` 32 to set, `SYS_CONTROLS` 33 to read). Word 0 is the block's generation, counted up by every set; words 1-15 are knobs, all ones meaning "unset: the driver's own default". Setting one takes the Settings capability (the map holds it; init's grants page is full, so no new capability was minted). Reading takes none: a driver reads its own block by its link number and acts when the generation moves, and the page shows the values. Two kinds of knob:
  - **A value** (a mode, a preference): kept in the device's settings document in the vault (SETTINGS.md), as every device setting is (AGENT.md: every change hits disk), loaded at boot before any driver starts. The kernel's `settings::KNOBS` names each kept one.
  - **An action** (do this now): a counter the page counts up; the driver acts once when it sees it change. Never kept: a count means nothing across a boot.
- **Link 6 is the clock**, whose driver is the kernel: its zone is the settings document's `tz`, applied the moment it is set.

The kernel checks only the capability and the slot; whether a value means anything is the driver's (it owns the hardware), and a value it cannot use it ignores and shows as it is.

## Per link

What each driver reports and what can be turned, as of 2026-11-07. "Next" is what the hardware or firmware could do but the driver does not yet know how to ask safely: never guessed (owner's rule for the modem: decoded against Android's own readings, never guessed).

### Cellular (payloads/modem, Shannon 5400 over SIT)

- **Measures:** state (connected with a data call, on otherwise), technology (the data stack's RIL radio technology, LTE 14 / NR 20), RSSI, RSRP, RSRQ, RS-SNR. They come from SIT_GET_SIGNAL_STRENGTH (G#0900, no parameters), asked on the data stack's channel every 2 s while it is registered, and from IND_SIGNAL_STRENGTH (G#0906) when the CP sends one; the reply's parameters after its 12-byte header are the indication's after its 8 (ProtocolSignalStrengthAdapter). SIT_SET_SCREEN_STATE (G#0902, u32 1) goes once at start. The parameters are a 16-bit version (4 on the Pro), then 50 words: the LTE ones at word 15 (signal in ASU, RSRP and RSRQ as positive dB, RS-SNR in tenths, CQI table, CQI, timing advance: FillLteSignalStrengthV4's order), inferred and range-checked, not yet checked against Android. On the Pro's EN-DC link (2026-11-09) every word past 20 is zero, the NR ones included: the anchor's LTE numbers are what the CP reports, so NR fields are not parsed until a reading shows where they are.
- **Tracks:** the data stack's registration state (the reply's first byte: 0 not registered, 1 home, 2 searching, 3 denied, 5 roaming, the CP's own 20 an emergency attach), the call's MTU, packets down and up, packets dropped down (queue full or malformed), uplink refused (ring full), and the link's suspends.
- **Detail** (`cellular`): the carrier pack the driver loads (`carrier`, Verizon: fixed until cfg.db is read), the data APN, the attach APN, the call's IPv4 and IPv6 and their DNS servers (while a call is up).
- **Knobs:** *re-register* (action): the data stack's radio off and on again (SIT_SET_RADIO_POWER 1 then 2, as the bring-up's own cycle does); the call drops with it and comes back once the stack is home.
- **Next:**
  - *Technology* (LTE only, NR only, both): SIT_SET_ALLOW_NETWORK takes a u32 bitmap in the CP's own numbering (the boot mask G#4_03FE is the CP's report, not Android's RadioAccessFamily); which bit is which needs Android's SET_ALLOWED_NETWORK_TYPES_BITMAP captured for each choice.
  - *Band and frequency lock*: no SIT message for it is known; Android sets it through an OEM path to capture.
  - *Cell identity, band, EARFCN, PCI, neighbours*: IND_CELL_INFO_LIST (G#070F) and IND_PHYSICAL_CHANNEL_CONFIG (G#0742) arrive and are not decoded. The registration reply's area, cell ids, PCI, EARFCN, timing advance and NR cell are read since 2026-11-11 by the vendor reader's offsets (LOCATION.md "What the modem knows") and served only on `cell`, to the place service, never logged; not yet matched against a live reading. Location-bearing: if the page ever shows them, they go in the detail, not tracks.
  - *Operator name, MNC*: the network's name, not decoded. Its MCC is (IND_MCC, G#072D, and NITZ's network field), for the zone only (docs/TIME.md).
  - *Network time*: NITZ (G#0905) is decoded for the zone (its offset, daylight flag and MCC go to the kernel, docs/TIME.md); its time is logged, not used.
  - *APN choice, roaming on/off, data on/off*: the profiles are Verizon's table today.

### WiFi (payloads/pcie1, BCM4398 msgbuf)

- **Measures:** state, RSSI, noise floor, SNR, centre frequency, channel width, rate, transmit loss over two seconds.
- **Tracks:** the channel number, transmit power (`qtxpower`, quarter dBm), the power-save mode the firmware reports (WLC_GET_PM), packets received and sent, receive errors and transmit failures (WLC_GET_PKTCNTS, cumulative).
- **Detail** (`wifi`): the network's name (from the person's credential), the access point's address (WLC_GET_BSSID), this device's address on that network (per network, keyed from the ira), and the lease (address, router, DNS server, length) when this driver ran DHCP itself; in service the `ip` process runs DHCP and its lease is not here yet (next: ip publishes it, or hands it to the driver).
- **Knobs:**
  - *Power save* (kept): off, on (PM1, the radio sleeps between beacons), fast (PM2, sleeps after a quiet spell), with the screen (the default since 2026-11-10: PM2 while the screen is off, off while it is on; 150 mW less with the screen off, POWER.md). Applied at once (WLC_SET_PM), and again at every screen change.
  - *Band* (kept): any (5 GHz whenever the network has it), 5 GHz only, 2.4 GHz only. Applied at the next join: switch WiFi off and on. No 2.4 GHz join has held under ferros yet (WIFI.md; the antenna is shared with Bluetooth and nothing coordinates them).
- **Next:** a scan on demand with its results (names in the detail, signals in tracks), forget the network (the credential is the identity's, `wifi.cred`), roaming (the driver joins with `roam_off`), join a chosen access point, the MCS and spatial streams (`rate_info` layout unchecked).

### GPS (payloads/gps, BCM4776 on-chip engine)

- **Measures:** state (on, or holding a fix), the fix's error estimate, satellites tracked and in the fix, the best carrier to noise; and with a fix, the altitude, speed and heading.
- **Tracks:** each tracked signal (`measure::SAT`): system, band, number, used, elevation, azimuth, C/N0 and how fast it moves across the sky. The page lists them, the orb draws the sky.
- **Knobs:** none. The chip runs a replayed gpsd session; turning systems or bands on or off means changing that session, whose messages are not decoded.
- **Next:** the position on the page (the fix is the `gps` endpoint's; location is the person's, so it would go in the detail, behind Settings), the chip's time as a clock source, systems on and off.

### Bluetooth (payloads/bt), NFC (payloads/nfc)

- **Bluetooth:** state, and the beacon's transmit power while it goes out. Next: the address (random each boot), the beacon on and off, pairings (the identity's).
- **NFC:** state (a reader's field on us is connected), the carrier while listening. Next: tag reads, card emulation.

### USB (payloads/usb, DWC3 device mode)

- **Measures:** state (configured is connected), the signalling rate, and the link's power state (U0 working, U1/U2 light sleep, U3 suspended).
- **Knobs:** none. The port is a device to the development host; the role is fixed.

### Clock (the kernel's rtc.rs; link 6 of the controls)

- **Shown:** the local time and UTC in the device's base, the zone by name with its standard and legal offsets and where it came from (the cellular network, GPS, by hand, a fixed offset), where the time comes from (the PMIC's clock, read at boot and every hour after), and the network's zone as heard.
- **Knobs:** the zone (docs/TIME.md, `SYS_ZONE`): "follow my identity", automatic or by hand, a picker by region then place; a fixed offset a quarter hour either way per tap (kept as the settings document's `tz`), only with no zone. The number base is the Numbers page's.
- **Next:** network time (NITZ, above) and GPS time as sources, and setting the PMIC's clock from them.

## Kernel calls

- `SYS_CONTROL` 32: x0 the Settings capability's token offset, x1 the link (0 cellular, 1 WiFi, 2 Bluetooth, 3 NFC, 4 GPS, 5 USB, 6 the clock), x2 the slot (1-15), x3 the word. The generation counts up; a kept knob writes the settings document (refused, and the old value back, when the vault does not take it). → 0 or ERR.
- `SYS_CONTROLS` 33: x0 the link, x1/x2 a buffer and its length in words. → the words written (at most 16: the generation, then slots 1-15), or ERR. Link 6 slot 1 reads the zone.
- `SYS_DETAIL` 34: x0 the op. Op 0 (a driver): x1/x2 the document and its length (at most 1 KiB), stored as its link's (by its process name, as `SYS_STATUS`). Op 1 (the settings): x1 the Settings capability's token offset, x2 the link, x3/x4 a buffer and its length → the document's whole length (0 when the driver has written none), its first bytes copied.
- `SYS_BELL` 38 (2026-11-10): x0 the source, 0-5 a link (its measures, tracks and detail document), 6 the facts a settings page shows (the battery's share and whether it charges, the processes alive, the vault's keys, the hottest part of the chip and its throttle); x1 a word in the caller's arc. The kernel writes the source's change count there now and at every change, waking the caller's threads that wait on it. A link counts only a word or document set to something other than it held, and its emptying as its driver ends; the facts count only when one of those shown changes (the gauge's two-second sample, the thermal read, the process table), never on a timer. No capability, as the measures and the facts. The claims' word (`SYS_CLAIM` op 4) is the same bell behind Settings. → 0, ERR for no such source.

The drivers find their link by their process name (modem, pcie1, bt, nfc, gps, usb), exactly as `SYS_STATUS` does, so no capability or grant slot is needed for a driver's own block.

## The pages

A link's page (payloads/map `draw_page`): its switch and state, every measure from its slots and tracks grouped (signal, channel, link, counters, motion) in the device's base, the satellites one a line, its names and addresses, its knobs as rows a tap turns. Addresses are identifiers, not quantities: they are written in hexadecimal grouped as their standards group them (IPv4 by byte with dots, IPv6 by sixteen bits with colons, hardware addresses by byte with colons), in every base. A link's page and Connections draw again when its bell rings (`SYS_BELL`: a measure, a track or the detail changed, or the driver ended), and when its switch moves; the GPS page also at its driver's doorbell (docs/GPS.md); the clock as each second of the wall clock turns, its rest ending on the turn (2026-11-10, owner: "1s poll can desync too"; until then all of them once a second).

## State (2026-11-07)

Built, committed, nothing flashed or deployed. The kernel calls need the kernel flashed; until then the drivers and the map degrade to today's behaviour (an unknown call returns ERR: the control blocks read as zeros, which mean power save off and any band; the detail is refused; a knob tap says on the console that the kernel did not take it). The driver capsules (modem, pcie1, gps, usb) and the map are to be stored and restarted with the flashed kernel.
