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

## photon_core (no_std, host-tested)

The reusable core of a ferros-native Photon client: `wire.rs` (ping/pong/photon/ack/pt_ack frames, byte-shaped like the stock app) and `chain.rs` (the braid — rolling-chain photon encryption, ported from photon/src/crypto/chain.rs with the same ihi primitives). Round-trip tested; byte-compatibility with the stock app is still to be confirmed on device.

## Needs the owner (can't be finished solo)

- **Enrollment / pairing:** the ferros device's Photon device key (ira-derived) must be added to the owner's fleet by the desktop Photon; nothing can beam until then (photon_on_ferros_no_bootstrap). Peers come from pairing + LAN broadcast — no FGTW bootstrap, no seed.
- **Photon interop test:** confirm a ferros-built photon decrypts in the stock app (wire + chain byte-compatibility).
- **Remaining photon_core layers:** lanes, message packaging, presence state machine, identity/conversation-token derivation — entangled with enrollment and best done against the running app.

## Next, in rough order

1. Migrate the WiFi driver to the sandbox (streamed firmware, its DMA region inside its arc, masked code) — retires the last native exception on husky. Touches the S2MPU/cache map; do it attended.
2. Finish photon_core against the app; enroll a ferros device; send the first photon.
3. Display: the owner's eyes on 10 bpc black and the 10 bpc link; the shell as a process with live rows; AOD as a server state; cold power-up via ACPM.
4. Camera stack → the first beam.
