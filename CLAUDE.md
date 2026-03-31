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
sed -i '' 's|    usb_iodev_shutdown();|    //usb_iodev_shutdown();  // ferros: keep PHY alive|' src/main.c
sed -i '' 's|    mmu_shutdown();|    //mmu_shutdown();  // ferros: keep DMA coherent|' src/main.c

make clean && make
# Produces build/m1n1.bin (~1.1MB)

# 2. Find and replace m1n1 on the Asahi ESP
diskutil list   # find the Asahi/ferros EFI partition
sudo diskutil mount <partition-id>
find /Volumes -name "*.bin" -path "*/m1n1/*" 2>/dev/null
sudo cp ~/m1n1/build/m1n1.bin <path-to-current-m1n1-boot.bin>
```

**What this changes:** Skips `usb_iodev_shutdown()` and `mmu_shutdown()` before jumping.
Display, PMGR, everything else works normally. USB PHY stays powered and DMA stays
coherent so ferros can take over the DWC3 controller for PT transport.
m1n1's MMU uses identity mapping (phys=virt) so ferros code works unchanged.

After replacing, `sudo shutdown -h now`. Then hold power → boot picker → "ferros".

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
  `p.reload()` (P_VECTOR) goes through m1n1's full shutdown path, disabling SPRR before jumping.
- **run_guest.py is wrong tool:** It runs a hypervisor (EL1 guest). We need direct EL2 boot.
  Custom `m1n1-boot.py` uploads kernel + minimal DTB and uses `p.reload()`.
- **DTB:** m1n1's `kboot_boot()` doesn't auto-generate a DTB — need to build a minimal one with
  a `simple-framebuffer` node containing reg, width, height, stride, format from boot_args.
- **30bpp display:** M1 DCP uses 10:10:10:2 pixel format. Console colors use G#FFFFFFFC (white)
  and G#00000000 (black), not standard 8-bit ARGB.

## Phase 4: Apple USB for PT Transport (CURRENT)
Implement DWC3 device-mode USB on M1 for PT transport. Then bridge connects for hot-reload, diag, beam.

### Key discovery: M1 uses Synopsys DWC3 — same IP as the FP5 driver
The DWC3 core protocol (TRBs, events, endpoint commands, bulk I/O) is identical.
Only platform init differs: ATCPHY + PipeHandler + DART instead of QUSB2/SMMU.

### M1 USB register map (from m1n1 reverse engineering)
- **DWC3 core**: reg[0] of `/arm-io/usb-drd0` (need ADT read for actual address)
- **PipeHandler**: reg[3] of `/arm-io/usb-drd0` (MUX, AON_GEN, reset control)
- **ATCPHY**: reg[0] of `/arm-io/atc-phy0` (Apple Type-C PHY)
- **DART USB0**: 0x382f80000 (confirmed from boot log, T8020 variant)

### ATCPHY init sequence (from m1n1/src/usb.c)
```
write32(atc + 0x08, 0x01c1000f)
write32(atc + 0x04, 0x00000003)
write32(atc + 0x04, 0x00000000)
write32(atc + 0x1c, 0x008c0813)
write32(atc + 0x00, 0x00000002)
```

### PipeHandler init (from m1n1/src/usb.c)
```
write32(pipe + 0x0c, 0x22)    // MUX: dummy mode
write32(pipe + 0x1c, 0x01)    // AON_GEN: DWC3_RESET_N
write32(pipe + 0x20, 0x9332)  // NONSELECTED_OVERRIDE
```

### DART (IOMMU) setup — T8020 variant
- TTBR at offset 0x200, TCR at offset 0x100
- 2-level page table: L1 (16KB pages) → L2 (16KB pages)
- Must map: event buffer, TRB rings, bulk I/O buffers to IOVAs
- Stream command invalidate at 0x20, stream select at 0x34

### DWC3 core init (same as any DWC3, from m1n1/src/usb_dwc3.c)
1. Device soft reset (DCTL.CSFTRST)
2. Core + PHY soft reset (GCTL.CORESOFTRESET + GUSB2PHYCFG/GUSB3PIPECTL PHYSOFTRST)
3. Force HS mode (DCFG.SPEED = 0)
4. Event buffer setup (GEVNTADR/SIZ/COUNT)
5. Endpoint config (DEPSTARTCFG, SETEPCONFIG, SETTRANSFRESOURCE)
6. Enable EP0 (DALEPENA), start controller (DCTL.RUN_STOP)

### Implementation plan
1. `ferros_hal_m1/src/dart.rs` — minimal T8020 DART for USB DMA mapping
2. `ferros_hal_m1/src/usb.rs` — fresh clean DWC3 driver implementing `UsbBulk` trait
3. Wire up USB event loop in M1 `kernel_main` (port from FP5 kernel)
4. `ferros-bridge` connects via PT — all commands light up

### Next boot: read ADT addresses
Use m1n1 proxy to dump the actual register base addresses:
```python
# In m1n1 proxy shell:
u.adt["/arm-io/usb-drd0"].get_reg(0)   # DWC3 core base
u.adt["/arm-io/usb-drd0"].get_reg(3)   # PipeHandler base
u.adt["/arm-io/atc-phy0"].get_reg(0)   # ATCPHY base
```

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
