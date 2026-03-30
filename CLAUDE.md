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
- **Pixel 8** (G9BQD, Tensor G3) — PRIMARY. ARMv9, EL2 via `fastboot oem pkvm disable`. Boxed for now.
- **M1 MacBook Air** — ACTIVE. EL2 via m1n1. macOS 26.4, Asahi/m1n1 UEFI entry installed (120 GB free). Fedora x86 box is the dev host connected via USB-C.
- **Fairphone 5** (QCM6490) — DROPPED. Bricked + Qualcomm security restrictions. Do not touch.

## Architecture
- `ferros_seed` — trust anchor (self-verify, kernel ring scan, jump)
- `ferros_kernel` — bare-metal aarch64 kernel (no_std); feature flags: `pixel8` (default), `m1`
- `ferros_hal` — HAL trait definitions (`UsbBulk`, `UsbEvent`) + shared utils
- `ferros_hal_m1` — M1 HAL stub (USB not yet implemented; framebuffer via DTB simplefb)
- `ferros_pt` — Photon Transport (blast mode, per-chunk BLAKE3)
- `ferros_vault` — persistent object store (tract, plow, HAMT, spine)
- `ferros_ledger` — categorized event chain, VSF from entry zero
- `tools/ferros-bridge` — host-side USB bridge (diag, reload, reboot)
- `tools/ferros-mkimg` — ELF to boot.img

## Current Priority
Get ferros booting on M1 via m1n1 proxy. Fedora box is the proxy host (USB-C). First goal: framebuffer console output on MacBook screen. Then: Apple USB controller for PT transport.

## Do NOT
- Commit anything from `Intellectual Property/` — contains patent filings
- Commit `CLAUDE MEMORY.md` or `memory/` — local project memory
- Use 0x prefix for numbers
- Add cleanup code, destructors, or shutdown procedures in critical paths
- Touch FP5/QCM6490-specific code — it's dropped

---

# Macbook Air (M1) Setup

## Phase 1: macOS Setup (DONE — 2026-03-30)
macOS 26.4 installed via DFU. Tools: Xcode CLT, Homebrew 5.1.2, Rust 1.94.1, aarch64-unknown-none, git, gh, iTerm2, VS Code.
Disk: 125 GB macOS, 120 GB free for ferros.

For future reinstalls:
1. Complete macOS setup minimal
2. `echo 'eval "$(/opt/homebrew/bin/brew shellenv zsh)"' >> ~/.zprofile && eval "$(/opt/homebrew/bin/brew shellenv zsh)"`
3. `brew install --cask iterm2 && brew install gh git && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
4. `rustup target add aarch64-unknown-none`

## Phase 2: m1n1 UEFI Boot Entry (DONE — 2026-03-30)
Ran `curl https://alx.sh | sh`, selected "UEFI environment only (m1n1 + U-Boot + ESP)".
Named the entry "ferros". macOS is default boot. 120 GB allocated to ferros partition.
Boot picker: hold power button → select "ferros" entry.

## Phase 3: First Boot via m1n1 Proxy (CURRENT)
The Fedora x86 box is the dev host. Steps:

**On Fedora (once):**
```bash
git clone https://github.com/AsahiLinux/m1n1
pip3 install pyserial
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
rustup target add aarch64-unknown-none
git clone https://github.com/nickspiker/ferros
cd ferros
cargo build -p ferros_kernel --target aarch64-unknown-none --release --no-default-features --features m1
```

**Each dev iteration:**
1. Boot MacBook → hold power → select "ferros" → m1n1 loads, MacBook shows up as `/dev/ttyACM0` on Fedora
2. On Fedora: `M1N1DEVICE=/dev/ttyACM0 python3 m1n1/proxyclient/tools/run_guest.py ferros/target/aarch64-unknown-none/release/ferros_kernel`
3. ferros boots → framebuffer console appears on MacBook screen

## Phase 4: Apple USB for PT Transport (NEXT AFTER PHASE 3)
Once booting confirmed, implement Apple USB controller in `ferros_hal_m1/src/usb.rs`.
Then ferros-bridge on Fedora can connect to running ferros kernel for hot-reload, diag, etc.

## Phase 5: Persistent ferros Boot Entry (LATER)
- Only after ferros boots stably
- iBoot requires stub macOS APFS container per boot entry (Asahi handles this)
- **NEVER set ferros as default boot — this is how we bricked it last time**
- **NEVER touch the macOS recovery partition** — it's per-install, lose it = no recovery
- macOS must always remain default. Use boot picker to select ferros.

## Important M1 Notes
- SecureROM (EL3) — silicon ROM, not flashable ever
- iBoot — signed by Apple, don't touch
- m1n1 gets EL2 — full hardware access, no TrustZone cage
- SEP owns Touch ID — never accessible from AP
- LocalPolicy (SEP-signed) controls boot security — changeable via `bputil` in recoveryOS
- Three security levels: Full / Reduced / Permissive
