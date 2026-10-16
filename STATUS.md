# ferros — status

A snapshot of what runs on the Pixel 8 Pro (husky) today and what's next. Area docs hold the detail: BOOT.md, KERNEL.md, SANDBOX.md, WIFI.md, CPU-FEATURES.md, KOHI-SOURCES.md, DISPLAY.md, KILLSWITCH.md.

## Runs on husky, cold boot (slot b, no Linux)

- **Boot:** ABL → ferros at G#8008_0000. Killswitch armed first (Vol Up + Vol Down → SYSTEM_OFF, interrupt-driven, owner-tested). Static cache map + VHE. "Ready?" gate (dev: 1 s auto-press, a volume key cancels it; production waits for power). Boot checklist on ABL's splash surface.
- **Identity/entropy:** KOHI gathers the session wairua from the SoC TRNG (via EL3) and CPU timing jitter, both health-tested (RCT/APT/GF(2)); target is four independent sources (KOHI-SOURCES.md). ira v2 from on-die sources. Vault opens and verifies; devinfo slot marked.
- **Processes:** EL0 processes in arcs, capability IPC (call/recv/reply, mmio, entropy, dma, input), 10 ms slices, signed capsules through the oracle gate, ihi per process. Boot spawns the vault's `init`.
- **Sandbox (SANDBOX.md), complete and proven:** masked capsules pass an install-time instruction checker (`ferros_sandbox`) over their declared `.text`, are sealed with an OS-only keyed hash in the vault, spawn only if sealed (no re-check), run in guarded 2S arcs with W^X (code read-only+executable, everything else non-executable). `payloads/wxtest` proves an accepted-but-malicious payload faults instead of escaping. Native `aarch64` capsules remain a dev-only path a production build refuses.
- **WiFi + network (WIFI.md), from an EL0 driver process:** PCIe ch1 up, BCM4398 firmware downloaded and booted, WPA2 join (firmware supplicant), DHCP + IPv6 SLAAC, UDP over IPv4 and IPv6. Three processes over capabilities: the driver serving Ethernet frames on `wlan`, `payloads/ip` (ARP/IPv4/IPv6/UDP) serving `udp`, `payloads/nettest` echoing. Ping and UDP echo work from the LAN. The HSI2 S2MPU fences every DMA master to ferros's own memory.
- **Toolchain:** `aarch64-unknown-none` with x24-x28 reserved and the CPU's real feature set enabled (CPU-FEATURES.md). `tools/ferros-masked-build.sh` builds a sandboxed image; `tools/husky-cold.sh`, `tools/husky-wifi.sh`, `tools/husky-net.sh` drive the device.

## photon_core (no_std, host-tested)

The reusable core of a ferros-native Photon client: `wire.rs` (ping/pong/photon/ack/pt_ack frames, byte-shaped like the stock app) and `chain.rs` (the braid — rolling-chain photon encryption, ported from photon/src/crypto/chain.rs with the same ihi primitives). Round-trip tested; byte-compatibility with the stock app is still to be confirmed on device.

## Needs the owner (can't be finished solo)

- **Enrollment / pairing:** the ferros device's Photon device key (ira-derived) must be added to the owner's fleet by the desktop Photon; nothing can beam until then (photon_on_ferros_no_bootstrap). Peers come from pairing + LAN broadcast — no FGTW bootstrap, no seed.
- **Photon interop test:** confirm a ferros-built photon decrypts in the stock app (wire + chain byte-compatibility).
- **Remaining photon_core layers:** lanes, message packaging, presence state machine, identity/conversation-token derivation — entangled with enrollment and best done against the running app.

## Next, in rough order

1. Migrate the WiFi driver to the sandbox (streamed firmware, its DMA region inside its arc, masked code) — retires the last native exception on husky. Touches the S2MPU/cache map; do it attended.
2. Finish photon_core against the app; enroll a ferros device; send the first photon.
3. Display driver (knobs inventory + design first, per the owner).
4. Camera stack → the first beam.
