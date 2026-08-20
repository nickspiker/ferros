# ferros — Claude Code Project Instructions

## What This Is
ferros is a mobile OS distilled down to what an OS actually is, in Rust on the Redox OS microkernel. Ring memory, hardware key storage, killswitch ready. See README.md for full overview.

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
- `ferros_hal_m1` — M1 HAL: DART IOMMU (T8020), DWC3 device-mode USB (full UsbBulk impl), framebuffer via DTB simplefb
- `ferros_pt` — Photon Transport (blast mode, per-chunk BLAKE3)
- `ferros_vault` — persistent object store (tract, plow, HAMT, spine)
- `ferros_ledger` — categorized event chain, VSF from entry zero
- `tools/ferros-bridge` — host-side USB bridge (diag, reload, reboot)
- `tools/ferros-mkimg` — ELF to boot.img
- `tools/ferros-install` — partition tool (resize, create, format, genesis, kernel, seed)

## Current Priority
Validate M1 DWC3 USB end-to-end: boot kernel via m1n1 proxy, verify USB enumeration, test PT transport with ferros-bridge. Then hot-reload.

## Do NOT
- Commit **unfiled** patent material from `Intellectual Property/`. As of 2026-08-20 **all patents are FILED** (ISOMEM provisional on file with USPTO, etc.), so that directory is public and committable (`.tex`/`.pdf`/`.ots`/figures; Latex build artifacts stay gitignored). This guard only re-arms if genuinely new, unfiled patent work ever appears — then ask before committing, since a public push is irreversible.
- Use 0x prefix for numbers
- Add cleanup code, destructors, or shutdown procedures in critical paths
- Touch FP5/QCM6490-specific code — it's dropped

---

# Macbook Air (M1) Setup

## Phase 1: macOS Setup (DONE — reinstalled 2026-04-07)
macOS 26.4 reinstalled. SIP disabled. Authenticated root disabled.
Tri-boot: macOS (83.6 GB) | Asahi (82 GB) | ferros partition (79.5 GB, GUID stamped) | Recovery.
Tools: Homebrew 5.1.5, Rust 1.94.1, aarch64-unknown-none + x86_64-unknown-none + riscv64gc-unknown-none-elf,
git, gh, llvm, cmake, ninja, nasm, qemu, just, blake3, gptfdisk, tree.
VS Code + DaVinci Resolve. No Apple account. Dvorak keyboard.
~90 launchd bloat services disabled (Siri, iCloud, analytics, iMessage, iOS bridging, etc).
Bloat apps removed from system volume (Music, TV, Maps, Mail, Siri, Photos, etc).
TCC nags killed for VSCode + Terminal + Resolve. Gatekeeper quarantine disabled.

For future reinstalls:
1. Complete macOS setup minimal
2. `echo 'eval "$(/opt/homebrew/bin/brew shellenv zsh)"' >> ~/.zprofile && eval "$(/opt/homebrew/bin/brew shellenv zsh)"`
3. `brew install gh git llvm cmake ninja nasm qemu just blake3 gptfdisk tree`
4. `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
5. `rustup target add aarch64-unknown-none x86_64-unknown-none riscv64gc-unknown-none-elf`
6. `gh auth login && gh repo clone nickspiker/ferros ~/code/ferros`
7. Run `~/disable-apple-bloat.sh` to kill launchd services
8. Recovery: `csrutil disable` + `csrutil authenticated-root disable`
9. Run `sudo ferros-install install` or `sudo ferros-install all` if partition exists

## Phase 2: m1n1 UEFI Boot Entry (DONE — reinstalled 2026-04-07)
Ran `curl https://alx.sh | sh`, selected "UEFI environment only (m1n1 + U-Boot + ESP)".
Named the entry "ferros". macOS is default boot. 82 GB allocated to Asahi/m1n1.
ferros data partition: 79.5 GB, GPT type GUID a0b51225-61c5-0f5a-ffe7-1b644f9ca954 (BLAKE3("ferros")).
Partition formatted with ring layout + genesis spine entry via `ferros-install all`.
Boot picker: hold power button → select "ferros" entry.

Disk layout (disk0):
```
| ISC (524MB) | macOS (83.6GB) | Asahi stub+EFI+Linux (82GB) | ferros (79.5GB) | Recovery (5.4GB) |
| disk0s1     | disk0s2        | disk0s3-s6                  | disk0s9         | disk0s7          |
```

## Phase 3: First Boot via m1n1 Proxy (DONE — 2026-03-30)
ferros boots on the M1 MacBook Air via m1n1 proxy. Framebuffer console output confirmed.

### Setup (already done)
- **MacBook:** Asahi m1n1 replaced with BRINGUP build (see below) on the ESP.
- **Fedora:** m1n1 cloned at `/mnt/Octopus/Code/m1n1`, pyserial + construct installed.
- **udev rule:** `/etc/udev/rules.d/99-m1n1.rules` auto-loads `cdc_acm` and sets `dialout` group.

### MacBook Claude: Install patched m1n1 (DO THIS — replaces broken BRINGUP build)
The BRINGUP build was wrong — it disables ALL peripherals (no display, no USB, no MMU).
We need a normal m1n1 build with ONLY `usb_iodev_shutdown()` commented out.

**DO NOT use `make EXTRA_CFLAGS="-DBRINGUP"`.** That breaks everything.

```bash
# 1. Build patched m1n1 (normal build, USB shutdown commented out)
cd ~/m1n1
git pull
git submodule update --init --recursive
rustup target add aarch64-unknown-none-softfloat

# Apply the patch: comment out usb_iodev_shutdown() in src/main.c line 195
# Change: usb_iodev_shutdown();
# To:     //usb_iodev_shutdown();
# (This is the ONLY change. Everything else stays normal.)
# Apply ALL patches to src/main.c in the #ifndef BRINGUP block before mmu_shutdown():
# 1. After usb_iodev_shutdown() add: { extern int usb_phy_bringup(u32 idx); usb_phy_bringup(0); }
# 2. Comment out display_shutdown(DCP_SLEEP_IF_EXTERNAL);
# 3. Comment out fb_shutdown(next_stage.restore_logo);
# The result should be:
#     usb_iodev_shutdown();
#     { extern int usb_phy_bringup(u32 idx); usb_phy_bringup(0); }
#     //display_shutdown(DCP_SLEEP_IF_EXTERNAL);
#     //fb_shutdown(next_stage.restore_logo);
#     mmu_shutdown();
# Add usb_phy_bringup(0) BEFORE mmu_shutdown() (needs MMU for PMGR register access).
# Everything else stays stock. This is the ONLY change to src/main.c.
python3 -c "
with open('src/main.c') as f: s = f.read()
s = s.replace('    mmu_shutdown();',
    '    { extern int usb_phy_bringup(u32 idx); usb_phy_bringup(0); }\n    mmu_shutdown();')
with open('src/main.c','w') as f: f.write(s)
"

make clean && make
# Produces build/m1n1.bin (~1.1MB)

# 2. Find and replace m1n1 on the Asahi ESP
diskutil list   # find the Asahi/ferros EFI partition
sudo diskutil mount <partition-id>
find /Volumes -name "*.bin" -path "*/m1n1/*" 2>/dev/null
sudo cp ~/m1n1/build/m1n1.bin <path-to-current-m1n1-boot.bin>
```

**What this changes:** Adds ONE line before `mmu_shutdown()`: `usb_phy_bringup(0)`.
Must be before MMU shutdown because PMGR register access needs device memory mappings.
All other shutdown steps run normally — identical to Phase 3.

After replacing, `sudo shutdown -h now`. Then hold power → boot picker → "ferros".

### MacBook Claude: Restore stock m1n1 (DO THIS IF PATCHED)
If the m1n1 on the ESP has any ferros patches, restore it to stock:
```bash
cd ~/m1n1
git checkout src/main.c
make clean && make
# Then mount ESP and copy as above
```

### Boot procedure (each dev iteration)
```bash
# 1. Build kernel + flat binary
cargo build -p ferros_kernel --target aarch64-unknown-none --release --no-default-features --features m1
aarch64-linux-gnu-objcopy -O binary target/aarch64-unknown-none/release/ferros_kernel \
    target/aarch64-unknown-none/release/ferros_kernel.bin

# 2. MacBook: shut down → hold power → boot picker → "ferros"
#    m1n1 loads, drops to proxy mode, MacBook appears as /dev/ttyACM0

# 3. Fedora: push kernel via proxy
M1N1DEVICE=/dev/ttyACM0 python3 tools/m1n1-boot.py
```

### Key learnings
- **ELF vs flat binary:** m1n1 proxy loads raw bytes — must use `objcopy -O binary`, not the ELF directly.
  File offsets in ELF don't match VMA offsets (`.text` starts at file offset G#10000, not G#0).
- **_m1_entry:** M1 needs a separate entry point that skips the Pixel 8's cache/MMU/SCTLR teardown.
  m1n1's `mmu_shutdown()` already handles this before jumping. The standard `_entry` code's
  cache clean + MMU disable sequence crashes on M1 (likely interacts badly with Apple SPRR/GXF).
- **p.reload() not p.call():** `p.call()` runs under SPRR which blocks execute on heap memory.
  `p.reload()` (P_VECTOR) goes thru m1n1's full shutdown path, disabling SPRR before jumping.
- **run_guest.py is wrong tool:** It runs a hypervisor (EL1 guest). We need direct EL2 boot.
  Custom `m1n1-boot.py` uploads kernel + minimal DTB and uses `p.reload()`.
- **DTB:** m1n1's `kboot_boot()` doesn't auto-generate a DTB — need to build a minimal one with
  a `simple-framebuffer` node containing reg, width, height, stride, format from boot_args.
- **30bpp display:** M1 DCP uses 10:10:10:2 pixel format. Console colors use G#FFFFFFFC (white)
  and G#00000000 (black), not standard 8-bit ARGB.

## Phase 4: Apple USB for PT Transport (CURRENT — code complete, needs hardware test)

### Implementation status
All code written, never run on hardware. Next step is boot + validate.

- `ferros_hal_m1/src/dart.rs` (346 LOC) — DONE. T8020 DART, dual register banks, L1/L2 16KB pages, 128MB IOVA.
- `ferros_hal_m1/src/usb.rs` (966 LOC) — DONE. Full DWC3 device-mode, all 13 UsbBulk trait methods.
  EP0 control (SET_ADDRESS, GET_DESCRIPTOR, SET_CONFIG), EP1 bulk IN/OUT, TRB-based I/O.
  USB descriptors: VID=G#1209, PID=G#4665 (pid.codes, pending PR #1208 merge), "ferros M1". 28 diagnostic fields.
- `ferros_kernel/src/main.rs` M1 path — DONE. DART setup, DWC3 init, USB event loop, PT dispatch stub.

### Hardcoded M1 register addresses (from m1n1 ADT dump)
- DWC3 core:    G#3_8228_0000
- PipeHandler:  G#3_82A8_4000
- ATCPHY:       G#3_82A9_0000
- DART USB0 bank 0: G#3_82F8_0000
- DART USB0 bank 1: G#3_82F0_0000

### Test plan (Phase 4a: validate boot-to-USB)
```bash
# Fedora box:
cd /mnt/Octopus/Code/ferros
git pull
cargo build -p ferros_kernel --target aarch64-unknown-none --release --no-default-features --features m1
aarch64-linux-gnu-objcopy -O binary target/aarch64-unknown-none/release/ferros_kernel \
    target/aarch64-unknown-none/release/ferros_kernel.bin

# MacBook: shut down → hold power → boot picker → "ferros" → m1n1 proxy mode
# Fedora:
M1N1DEVICE=/dev/ttyACM0 python3 tools/m1n1-boot.py

# Watch MacBook framebuffer for DWC3 probe + DART init output
# Then from Fedora:
cargo run -p ferros-bridge -- status    # should see VID=G#1209 PID=G#4665
cargo run -p ferros-bridge -- diag      # PT DIAG command
```

### Known limitations (OK for Phase 4, fix later)
- Hardcoded register addresses (should read ADT)
- Polling only, no GIC interrupts
- Single-TRB model (no TRB rings, no UPDATETRANSFER)
- No error recovery from stalled transfers
- ATCPHY/PipeHandler init not called (m1n1 pre-inits them)
- DART limited to 128MB IOVA (4 pre-allocated L2 tables)
- Cache coherence requires explicit clean/invalidate around DMA

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
