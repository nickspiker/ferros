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
# Pixel 8 (default)
cargo build -p ferros_kernel --target aarch64-unknown-none --release

# M1 MacBook Air
cargo build -p ferros_kernel --target aarch64-unknown-none --release --no-default-features --features m1

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

## Phase 1: Clean macOS Install (DONE — 2026-03-30)
macOS 26.4 installed via DFU (Apple Configurator 2). Tools installed:
- Xcode CLT, Homebrew 5.1.2, Rust 1.94.1, aarch64-unknown-none target, git, gh, iTerm2, VS Code
- Bloatware: macOS 26 fresh install has none in /Applications — system apps live in /System/Applications (SIP-protected, not worth removing)
- Note: Photon Messenger was installed to ~/Applications during setup (ferros comms tool)
- Disk: 245 GB total, ~26 GB used by macOS, ~219 GB free for Asahi + ferros

For future reinstalls:
1. Complete macOS setup (minimal — skip Apple ID if possible, or use throwaway)
2. Add brew to PATH: `echo 'eval "$(/opt/homebrew/bin/brew shellenv zsh)"' >> ~/.zprofile && eval "$(/opt/homebrew/bin/brew shellenv zsh)"`
3. Install tools: `brew install --cask iterm2 && brew install gh git && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
4. Add Rust target: `rustup target add aarch64-unknown-none`

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
- **NEVER set ferros as default boot — this is how we bricked it last time**
- **NEVER overwrite or share the macOS recovery partition** — each macOS install has its own recovery; touching it = no recovery path
- macOS must always remain the default boot. Use boot picker (hold power) to select ferros/Asahi

## Important M1 Notes
- SecureROM (EL3) is not flashable, ever — silicon ROM
- iBoot is updatable but signed, don't mess with it
- m1n1 gets EL2 — full hardware access, no TrustZone cage
- SEP owns Touch ID, never accessible from AP at any privilege level
- FaceTime camera is USB internally (not CSI-2)
- LocalPolicy (SEP-signed) controls boot security — changeable via `bputil` in recoveryOS
- Three security levels: Full / Reduced / Permissive
