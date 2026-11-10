# ferros — status

A snapshot of what runs on the Pixel 8 Pro (husky) today and what's next. Area docs hold the detail: BOOT.md, KERNEL.md, SANDBOX.md, WIFI.md, CPU-FEATURES.md, KOHI-SOURCES.md, DISPLAY.md, KILLSWITCH.md.

## Runs on husky, cold boot (slot b, no Linux)

- **Boot:** ABL → ferros at G#8008_0000. Killswitch armed first (Vol Up + Vol Down → SYSTEM_OFF, interrupt-driven, owner-tested). Static cache map + VHE. "Ready?" gate (dev: 1 s auto-press, a volume key cancels it; production waits for power). Boot checklist on ABL's splash surface.
- **Identity/entropy:** KOHI gathers the session wairua from the SoC TRNG (via EL3) and CPU timing jitter, both health-tested (RCT/APT/GF(2)); target is four independent sources (KOHI-SOURCES.md). ira v2 from on-die sources. Vault opens and verifies; devinfo slot marked.
- **Processes:** EL0 processes in arcs, capability IPC (call/recv/reply, mmio, entropy, dma, input), 10 ms slices, signed capsules through the oracle gate, ihi per process. Boot spawns the vault's `init`.
- **Sandbox (SANDBOX.md), complete and proven:** masked capsules pass an install-time instruction checker (`ferros_sandbox`) over their declared `.text`, are sealed with an OS-only keyed hash in the vault, spawn only if sealed (no re-check), run in guarded 2S arcs with W^X (code read-only+executable, everything else non-executable). `payloads/wxtest` proves an accepted-but-malicious payload faults instead of escaping. Native `aarch64` capsules remain a dev-only path a production build refuses.
- **WiFi + network (WIFI.md), from an EL0 driver process:** PCIe ch1 up, BCM4398 firmware downloaded and booted, WPA2 join (firmware supplicant), DHCP + IPv6 SLAAC, UDP over IPv4 and IPv6. Three processes over capabilities: the driver serving Ethernet frames on `wlan`, `payloads/ip` (ARP/IPv4/IPv6/UDP) serving `udp`, `payloads/nettest` echoing. Ping and UDP echo work from the LAN. The HSI2 S2MPU fences every DMA master to ferros's own memory.
- **Toolchain:** `aarch64-unknown-none` with x24-x28 reserved and the CPU's real feature set enabled (CPU-FEATURES.md). `tools/ferros-masked-build.sh` builds a sandboxed image; `tools/husky-cold.sh`, `tools/husky-wifi.sh`, `tools/husky-net.sh` drive the device.

## Display (DISPLAY.md, DISPLAY-DESIGN.md, DISPLAY-POWER.md) — 2026-10-16

- **Every mode from ferros, verified on the panel's own registers and the TE pin:** WQHD ⇄ FHD by the full modeset (DECON + DSIM stopped, re-initialised, started; the panel's PPS and raster), 60/120 Hz, NS 60, the 30/10/1 Hz auto-rate floors, AOD in and out, HBM and dimming, the 1346/1368 Mbps link rates, 8 and 10 bpc blend (10 bpc + DQE dither is the default; costs nothing). DECON, DSIM and DPHY match Android's dumps word for word at 1346 / 10 bpc. The 10 bpc *link* is prepared and waits for eyes.
- **Power, measured from the battery** (the charger's buck off for each sample, the gauge over hsi2c_11): white costs 2.44 W over black at 140 nits; the panel's floor itself moves 0.8 W between 2 and 140 nits; 1000 nits 8.3 W, HBM 12.9 W; AOD 2.1 W whole-phone; refresh rate is worth 100–200 mW.
- **The display server** (`payloads/display`, an EL0 process): owns a 32 MiB framebuffer region and the base layer, paints with fluor's no_std canvas in Oxanium, presents on TE, serves the `display` endpoint (draw ops, brightness, refresh, idle floor, DCS through `ferros_dsi`), asks the panel for the 10 Hz floor between frames. `payloads/shelltest` draws SHELL.md's home screen through it. `tools/husky-display.sh` brings it up.
- **Kernel fixes along the way:** a `sleep` syscall and no slices during PT transfers; no idle exit while a process lives; PIE-linked payloads relocated at entry (`ferros_sys::relocate`); the stall exit named its culprit and the DIAG handlers' `continue` skipping the transfer close was fixed — every "unexplained" PT-stall exit.

## Peripherals brought up from a cold boot (2026-10-17 → 20)

- **Touch (docs/oracle/TOUCH.md) — reports contacts, finger-verified 2026-10-20.** The ST FTM5 on SPI, cold: rails over ACPM, reset, controller ready in 12 ms, chip G#3640 (SALIXP). Contacts arrive with position, an eight-bit pressure, and the contact ellipse's major and minor axes; 1344 events in 8192 ms tracking a finger. Three things each made the chip sense without reporting, and all three are easy to reintroduce, so they are written down: `setScanMode` is *two* writes (a zero write to G#2000_0000 precedes the mode command), an event's identifier is the high nibble of byte zero alone, and the queue must be drained in bursts off byte seven's depth. Sixteen of the forty capacitance frame layers the driver defines are live on this panel, readable raw. Diagnostics: `diag touch up/watch/info/map/raw/decay/gamma/off`.
- **Audio — control side only.** Both CS35L41 speaker amplifiers answer from cold (DEVID G#35A40, rev B2) over the shared SPI. No sound has been played yet.
- **Camera (docs/oracle/CAMERA.md) — main camera takes pictures (2026-10-21).** 4080 x 3072 RAW10 from a cold boot: the front end's clock set as Linux's firmware sets it (465 MHz, found by an hv live-register snapshot), its local channels enabled (G#1C32_00A0), frames written to memory every frame. Lens focus from ferros (sweep peaks at G#3800 for 1 m), exposure ~68.8 ms at ~14.5 fps (counted), colour through the main camera's Absolute IDT into VSF (`tools/camera-raw2vsf`), a live viewfinder on the panel (`cam pdma view=<s>`, ~0.8 fps, fixed point), many runs per boot. Ultrawide (Sony IMX787) has a replay-grade init capture; front answers G#08D5; tele not reached yet.
- **Modem (docs/oracle/MODEM-BOOT-RECON.md) — ROM boots, boot channel stalls after one frame.** PCIe ch0 trains to Gen3 x2 (the drop must also switch the PHY to the oscillator clock and pulse SOFT_PWR_RESET), the CP answers config and the ROM boots. It answers the *first* boot-channel frame (A110 → C110) and then consumes every later frame silently. Ruled out: the per-frame mask and doorbell, the FMT ring, RC MSI status, the unprompted C11B and the `ap2cp_united_status` value. The CP repositions the AP's tx cursors itself as part of emitting C11B; what makes it do so is the open question. The per-unit NV files are read from their f2fs partitions by `ferros_f2fs` (2026-10-25, matching Android's reads); they never get logged or committed.

## Storage and the boot gate (2026-10-20)

- **The kernel vault runs on manifestus** (`ferros_kernel/src/kvault.rs`), the owner's own store, rather than a kernel copy of it: two mirrored UFS regions, sealed values, a sealed name index, proven across reboots. Reserved names stay out of listings. The in-tree `ferros_vault` is still referenced by `shim.rs`, `ira.rs` and the wairua nonce source, and comes out next.
- **An append-only text log on the flash** (`flashlog.rs`, `diag log`): a bus stall loses the PT response, so anything being bisected names each step on the flash before it takes it. This is what found the camera read that stalls the bus.
- **The boot gate in the hv** (`ferros_hv/src/gate.rs`): one pre-assembled slot-a image, `[hv][Linux][ferros]`, showing "Ready?" before Linux — power (or 1 s on dev) boots ferros, volume down continues to Android, volume up holds. Android on slot a stays the escape hatch and the register oracle.
- **The wedged USB port recovers itself** (`tools/usb-kick.sh`): the host port, not the phone, is what gives up after a -71 storm; the scripts kick it through its sysfs switch instead of asking for a replug.

## Photon and FGTW on ferros (corrected 2026-11-05)

**Photon is one full, self-supporting app on every platform, ferros included** (owner, 2026-11-05: "None of this 'Photon stripped copy' bullshit"):
- **Photon:** `std/photon` builds the real photon-messenger crate for the ferros std target ([STD-TARGET.md](STD-TARGET.md); `tools/photon-build.sh`). It has run on the device since 2026-10-27, drawing through fluor's ferros host.
- **FGTW:** attest, identity and the OS's basic sync.
- **`photon_core`** (no_std) was a reimplementation of Photon's wire, PT, join, chain and beam code, written 2026-10-26 before the real app ran here. It is superseded, and nothing more is built on it. `payloads/fgtw`, `payloads/beamtest` and `tools/photon-interop` still depend on it; retiring them is the owner's call.

## The night of 2026-11-04

- **Every radio's DMA fenced (AUTH.md "Radios while locked").**
  - On a cold boot the modem's S2MPU (HSI1) and BLK_MISC's were open, and the modem driver had been switching its own protection off.
  - The kernel now fences the modem to its device-tree carve-outs and denies BLK_MISC everything. No process may write any S2MPU.
  - Verified on a cold boot: registration, the data call, an HTTP fetch over cellular, no faults.
- **Location is the person's (GPS.md).** `gps.last` is identity class, and the replays' captured position is zeroed. Owner rule: no GPS while unattested, locate excepted (designed, not built).
- **Nothing acts on the radios while locked.** SYS_FACTS 83 says whether a person is present. `ip` serves applications only their addresses while nobody is, `bt` refuses the beacon, `nfc` refuses the tap secret.
- **The GPU is online (GPU.md "Online").**
  - `payloads/gpu` boots the CSF microcontroller from the device's own firmware (115 us) and answers a ping (15 us).
  - It runs one group and queue, with jobs waited for on the GPU's job interrupt: SYNC_SET32 and STORE_MULTIPLE + WAIT confirmed against Mesa's v11 encodings.
  - **The shader cores compute:** a hand-encoded Valhall compute shader, 16 invocations each writing G#C0FF_EE00 + its id, then 16384 invocations; then **the Mandelbrot set**, a 26-instruction kernel with a divergent float loop, 64 x 64 in 708 us, bit-exact against the CPU (GPU.md "A real kernel").
  - The kernel holds its fence (to `gpu-mem`) and leaves it powered. The secure DRAM carveout at G#B600_0000 is now written down twice.
- **Kernel out.** The GPS bring-up tools left. The rest of the list needs owner decisions (KERNEL-OUT.md 3, 5, 7-10).
- **Attest keys:** the press highlights work again (broken since the sandbox's own frame buffer), plus a bevel.

## Needs the owner (can't be finished solo)

- **Enrollment:** the real Photon on the device (`std/photon`) joins the owner's fleet the way any Photon device does: the device-to-device bind over the LAN, built in Photon on 2026-10-26 (photon docs/pairing-v2.md, "The ceremony over the mesh"). It needs the owner at the desktop Photon. Until then the desktop drops pings from a device key it does not know.

## Next, in rough order

**Hardware bring-up, the night of 2026-10-24/25 (each with its diag, doc and commit):**
- **Sensors (SENSORS.md):** 39 of the AoC's 41 sensors stream under ferros from every boot (rails, firmware, registry push with the board CDT, the display's state and TE2 for the under-display light/proximity sensor). Gravity turns the clock; proximity carries its raw counts; the proximity wake is built, off until tuned (`device.prox_wake`).
- **Haptics:** the CS40L26 answers (rev A1, ROM firmware); `diag haptic play <n>` queues a ROM waveform — not yet felt.
- **GPS (GPS.md):** the BCM4776 powers and answers its wake handshake and SPI framing; since 2026-10-25 it gives fixes (below).
- **Modem (docs/oracle/MODEM-BOOT-RECON.md):** the boot channel runs through — TOC, MAIN (98 MB, CRC accepted), VSS, APM; stopped on purpose before the per-unit NV and START.
- **GPU (GPU.md):** Mali-G715 r1p2, 7 cores, answers; the CSF bring-up plan written (after toka).
- **TPU (TPU.md):** firmware loaded through GSA, started through Trusty, its first KCI exchange answered (changelist 836291931). Next: the in-kernel VII path (a captured model replayed).
- Fixed along the way: the flash log raced vault reads from other cores (touch failed to start at boot); `bridge reboot fastboot` resets again (vault writes kept petting the watchdog); the AoC no longer takes the touch bus.

**2026-10-25 (day and night):**
- **Power (POWER.md):** screen on at rest 3.5 → **1.21–1.26 W** (Android 1.16), screen off cable in **0.58 W**: the clock policy (rest / active / dark operating points, every request acked before the next), the UFS link hibernating after 16 ms idle (0.65 W), the unused SoC power domains gated at boot through EL3 (0.35 W), the kernel's event stream and idle backstop, the worker cores off while dark (35 mW). The PMICs' rail meters read from ferros (`diag odpm`) and compared with Android's own rail by rail; BB_HLDO's 324 mW is the meter's, not a load.
- **USB:** the replug wedge was the phone's TCPC USB switch left open after a detach; the re-init closes it.
- **GPS (GPS.md): fixes under ferros.** The chip's transport decoded from captures of lhd (tools/gtrace); ferros powers it, downloads its patch and replays gpsd's on-chip session from the vault with the time aiding rewritten from the RTC; the chip's own engine reports a fix a second, the first 11–18 s after power-up, settling at ±8 m. `diag gps boot [s]`, `fix`, `off`. Open: an indoor offset of ~150 m against Android's host engine (the same engine under Android agrees with ferros), and independence from the captured aiding.
- **Modem (docs/oracle/MODEM-BOOT-RECON.md):** `ferros_f2fs` reads the per-unit NV files (efs, modem_userdata; hashes match Android's own reads); the boot channel now carries TOC, MAIN, VSS, APM and the three NV sections, every frame acked. START (G#A400) waits for the owner.


**The owner's list, 2026-11-07:**
- **The time zone from the network (NITZ).** The modem gets SIT `IND_NITZ_TIME_RECEIVED` (G#0905) when it camps; its payload after the 8-byte header reads `01`, year−2000, month, day, hour, minute, second (UTC), the zone in signed quarter hours (the whole current offset, daylight saving included: G#E0 = −8 h, Pacific standard), the daylight-saving adjustment in hours, then the network (MCC/MNC as ASCII, "311480"). To build: the modem process reads the zone on every NITZ and hands it to the kernel through a narrow right of its own (not the settings right), which sets `rtc::TZ` and saves `device.tz`. The modem logs each NITZ whole since 2026-11-07.
- **The status circles** (the corners' link measures) stay above everything, pinned at the top of the screen, outside the home screen's transform.
- **The cellular circle's signal is asked for (2026-11-09).** The CP stopped sending `IND_SIGNAL_STRENGTH` (G#0906) on its own after 2026-11-03. The modem now asks with SIT_GET_SIGNAL_STRENGTH (G#0900) every 2 s while registered, and says the screen is on (SIT_SET_SCREEN_STATE G#0902) once. On the Pro the circle shows RSRP, RS-SNR, RSRQ and RSSI (−110 dBm, 8 dB, −7 dB, −95 dBm on the first look). Not yet checked against Android's readings of the same moment.
- **Fleet: removal has to travel as joining does.** The departure request goes only to siblings heard on the LAN; ferros never announces itself in the phonebook, so a sibling elsewhere (Brittany's phone, 2026-11-07) never hears it, and WiFi carries no multicast beacons. Next: ferros announces its address in the phonebook and sends sibling frames the way Photon does (then removal reaches every sibling, and the device shows in their lists). Then: device names on the Fleet page instead of key prefixes (the chain carries only keys), adding a device and approving another's removal from the page, and "save first" as a copy to a sibling (Photon's copy-then-release), never to fgtw.org.
- **The jump:** after a jump the panel shows static (the layer DMA stuck on Android's in-flight read, so DPP0's shadows never latch: quiesce the DPU's DMA before the freeze), and one jump died after the worker cores started (crumb G#CC).
**The owner's order (2026-10-24):** the clock upright by gravity (the accelerometer through the AoC's USF) and the 16-bit canvas; then the proximity-inhibited wake (designed with the owner once the proximity sensor's specs are known); then the spectrometer, GPS, the cellular radio; then the keyboard, the IME and OS housekeeping (a task manager and the like); then Photon.


1. **The touch server:** ~~an EL0 process taking the controller's interrupt (gpn0-0) instead of polling, serving a `touch` endpoint~~ — done (payloads/touch waits on its interrupt; checked 2026-10-29); still open — raw eight-bit pressure straight through to the app, with mm/px and px/RU published. The display server is the pattern to copy. Then fluor grows the `Event::Touch` arm its event doc already reserves (pressure, the contact ellipse, touch id, contact kind), pressure threads through `PointerArbiter` into `Context`, and `theme`'s `HELD` becomes a ramp so a control brightens under a harder press. Mouse hosts report full pressure on press, so nothing on desktop changes. Then `shelltest` answers a real tap.
2. Migrate the WiFi driver to the sandbox (streamed firmware, its DMA region inside its arc, masked code) — retires the last native exception on husky. Touches the S2MPU/cache map; do it attended.
3. The real Photon (`std/photon`) on the device: enroll it in the owner's fleet and send the first photon. Then the next pieces of the app on ferros: kete's native vault backend, fluor's host over the app surface, tokio over the `ip` server, input ([STD-TARGET.md](STD-TARGET.md)).
4. Display: the owner's eyes on 10 bpc black and the 10 bpc link; the shell as a process with live rows; AOD as a server state; cold power-up via ACPM.
5. Camera: cacheable frame buffers + NEON for a faster viewfinder; the ultrawide from its capture (link, PHY and PDMA captures still needed); the gain → ISO mapping for absolute IDT comparison; the screen calibration with the owner.
6. Cellular: **done through IP (2026-10-29)** — the modem is its own driver process (payloads/modem: cold bring-up, registration on Verizon LTE/NR, the internet data call), serving `rmnet` to `ip`, which routes WiFi first per family and cellular otherwise; httptest fetches over cellular IPv6 and IPv4 (docs/oracle/MODEM-BOOT-RECON.md). Next: start it at boot (init), re-establish a dropped call, then data-only polish (the MVNO is data-only: mvno_data_only).
7. **The owner's full reach through the portal** (PORTAL.md §5: owned, unlocked members hold full authority for their identity): the USB PT dispatch checks who is asking — a request signed by a device the fleet chain resolves to the identity — and the development capabilities (MEM read/poke, RELOAD, RUN, SPAWN, DIAG, DISPLAY, STORE) become portal verbs granted in full to an identity-matched request in production builds, as the passless RustDesk fork works today; anyone else gets nothing. Until then production builds compile MEM out (`ferros_production`, service.rs), a stopgap, not the design.
