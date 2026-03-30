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
The Fedora x86 box (leviathan) is the dev host. MacBook is the target.
m1n1 is cloned at `/mnt/Octopus/Code/m1n1` on the Fedora box. pyserial is installed.

### Problem: Asahi m1n1 has U-Boot embedded
The Asahi installer concatenated U-Boot into m1n1.bin as a payload. When m1n1 finds a payload,
it chainloads immediately instead of waiting for proxy. We need a proxy-only m1n1 (no payload).

### MacBook Claude: Install proxy-only m1n1 (DO THIS FIRST)
The Asahi-installed m1n1 chainloads U-Boot. Replace it with a proxy-only build.

```bash
# 1. Clone and build m1n1 (native aarch64 — no cross-compile needed)
git clone https://github.com/AsahiLinux/m1n1 ~/m1n1
cd ~/m1n1
git submodule update --init --recursive
rustup target add aarch64-unknown-none-softfloat
make   # native build, produces build/m1n1.bin (~1.1MB, proxy-only)

# 2. Find the Asahi EFI System Partition
diskutil list   # look for the Asahi/ferros EFI partition (likely ~500MB, type "EFI")
# It will NOT be the macOS EFI — look for the one Asahi created

# 3. Mount the ESP and find the current m1n1
# The path is typically: /Volumes/<ESP>/m1n1/boot.bin
# Asahi uses a custom boot.bin path, not the standard EFI boot path
sudo diskutil mount <partition-id>   # e.g. disk0s4 or similar
find /Volumes -name "*.bin" -path "*/m1n1/*" 2>/dev/null
ls -la /Volumes/*/m1n1/   # find the current m1n1 boot binary

# 4. Replace with proxy-only build
sudo cp ~/m1n1/build/m1n1.bin <path-to-current-m1n1-boot.bin>
```

**CRITICAL:** Do NOT touch the macOS EFI partition. Only modify the Asahi/ferros ESP.
After replacing, `sudo shutdown -h now`. User holds power → boot picker → "ferros" → m1n1 proxy mode.

### Fedora side (DONE)
```bash
# m1n1 cloned + built at /mnt/Octopus/Code/m1n1
# ferros M1 kernel built:
cargo build -p ferros_kernel --target aarch64-unknown-none --release --no-default-features --features m1
```

### To boot ferros (after proxy-only m1n1 is installed)
1. On MacBook: shut down → hold power button → boot picker → select **"ferros"**
   - m1n1 loads, finds no payload, drops to proxy mode
   - MacBook appears as `/dev/ttyACM0` on Fedora (USB-C cable must be connected)
2. On Fedora: `M1N1DEVICE=/dev/ttyACM0 python3 /mnt/Octopus/Code/m1n1/proxyclient/tools/run_guest.py /mnt/Octopus/Code/ferros/target/aarch64-unknown-none/release/ferros_kernel`
3. ferros boots at EL2 → framebuffer console shows "ferros on M1" on MacBook screen

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
