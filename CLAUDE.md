# ferros — Claude Code Project Instructions

## What This Is
ferros is a mobile OS built from first principles in Rust on the Redox OS microkernel. Ring memory, hardware key storage, killswitch ready. See README.md for full overview.

## Conventions
- All numeric output uses `[base36]#[number]` format (G# for hex, A# for decimal) — NEVER use 0x prefix
- Pure Rust, no C dependencies. `unsafe` only at audited hardware boundaries
- Binary OCD: powers of 2 for counts, timeouts, loops
- EWE encoding on disk — no fixed-width integers on persistent storage, ever
- Write-verify-then-mirror protocol for all persistent writes
- Dozenal versioning: Zil(0), Zila(1), Zilor(2), Ter(3)
- USB comms use serialized VSF + Photon Transport (not CDC-ACM)

## Build
```bash
cargo build -p ferros_kernel --target aarch64-unknown-none --release
cargo run -p ferros-mkimg -- boot target/aarch64-unknown-none/release/ferros_kernel -o ferros.img
cargo run -p ferros-bridge -- reload target/aarch64-unknown-none/release/ferros_kernel
```

## Target Hardware
- **Pixel 8** (G9BQD, Tensor G3) — PRIMARY. ARMv9, EL2 via `fastboot oem pkvm disable`
- **M1 MacBook Air** — SECONDARY. EL2 via m1n1, proxy mode for live hardware exploration
- **Fairphone 5** (QCM6490) — BRICKED. EDL mode, waiting on Fairphone for firehose programmer. Do not invest time here.

## Architecture
- `ferros_seed` — trust anchor (self-verify, kernel ring scan, jump)
- `ferros_kernel` — bare-metal aarch64 kernel (no_std)
- `ferros_hal` — HAL trait definitions (being split from implementations)
- `ferros_pt` — Photon Transport (blast mode, per-chunk BLAKE3)
- `ferros_vault` — persistent object store (tract, plow, HAMT, spine)
- `ferros_ledger` — categorized event chain, VSF from entry zero
- `tools/ferros-bridge` — host-side USB bridge (diag, reload, reboot)
- `tools/ferros-mkimg` — ELF to boot.img

## Current Priority
Comms first: Photon Transport + ferros-bridge over USB. Then HAL trait split (`ferros_hal` traits, `ferros_hal_pixel8` and `ferros_hal_m1` implementations). Start with `UsbBulk` trait.

## Do NOT
- Commit anything from `Intellectual Property/` — contains patent filings
- Commit `CLAUDE MEMORY.md` or `memory/` — local project memory
- Use 0x prefix for numbers
- Add cleanup code, destructors, or shutdown procedures in critical paths
- Touch FP5-specific code unless explicitly asked

---

# Macbook Air (M1) Setup Tasks

These are instructions for setting up the M1 MacBook Air as a ferros development target. Run these when working on the macbook directly.

## Phase 1: Clean macOS Install (CURRENT)
macOS was just restored via DFU (Apple Configurator 2). Device is at Hello screen.

1. Complete macOS setup (minimal — skip Apple ID if possible, or use throwaway)
2. Run `diskutil list` and save the partition layout
3. Remove bloatware apps where possible:
   - `sudo rm -rf /Applications/FaceTime.app`
   - `sudo rm -rf /Applications/TV.app`
   - `sudo rm -rf "/Applications/Apple TV.app"`
   - `sudo rm -rf /Applications/News.app`
   - `sudo rm -rf /Applications/Stocks.app`
   - `sudo rm -rf /Applications/Chess.app`
   - `sudo rm -rf /Applications/Maps.app`
   - Note: SIP may block some of these. `csrutil disable` from recoveryOS if needed, then re-enable after.
4. Install essential tools:
   - Xcode Command Line Tools: `xcode-select --install`
   - Homebrew: `/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"`
   - Rust: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
   - `rustup target add aarch64-unknown-none`
   - `brew install git`

## Phase 2: Asahi Linux
1. Install Asahi Linux: `curl https://alx.sh | sh`
   - It handles its own partitioning safely
   - Let it resize macOS APFS container
   - Note how much space it takes and what's left
2. Boot into Asahi, verify everything works
3. Install Rust toolchain in Asahi too

## Phase 3: ferros Development via m1n1
1. m1n1 proxy is the primary dev interface — loads payloads over USB-C to RAM
   - No partition needed, no flash, no boot entry modification
   - Built by Hector Martin (marcan), Asahi Linux project
2. Clone ferros repo, build for aarch64-unknown-none
3. UART addresses are hardcoded to QCM6490 — will need updating for M1
4. First goal: get PT transport working over USB between macbook and dev box
5. macOS stays DEFAULT boot — hold power button for boot picker to select Asahi/ferros

## Phase 4: Persistent ferros Boot Entry (LATER)
- Only after ferros actually boots on M1
- iBoot requires a stub macOS APFS container per boot entry
- Model after how Asahi creates its boot entry
- ferros entry chainloads: iBoot -> m1n1 -> ferros
- NEVER set ferros as default boot (this is how we bricked it last time)

## Important M1 Notes
- SecureROM (EL3) is not flashable, ever — silicon ROM
- iBoot is updatable but signed, don't mess with it
- m1n1 gets EL2 — full hardware access, no TrustZone cage
- SEP owns Touch ID, never accessible from AP at any privilege level
- FaceTime camera is USB internally (not CSI-2)
- LocalPolicy (SEP-signed) controls boot security — changeable via `bputil` in recoveryOS
- Three security levels: Full / Reduced / Permissive
