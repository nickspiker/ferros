# Ferros Project Memory

## Target Hardware — PRIMARY: Pixel 8 + M1 MacBook Air (FP5 on ice)
- **Pixel 8** (G9BQD, Obsidian) — EL2 available (pKVM disableable via fastboot), official unlock, in transit as of 2026-03-23
- **M1 MacBook Air** — EL2 from day one via m1n1, no signing ceremony, proxy mode for live HW exploration
- **Fairphone 5** (QCM6490 / SM7325) — BRICKED (EDL mode, all partitions erased), waiting on Fairphone support; see [project_target_hardware.md](project_target_hardware.md)
  - ARM64, Cortex-A78 + Cortex-A55, DRAM base: G#80000000
  - Display: RM692E5 BOE AMOLED 1224x2700, DSI command mode
  - Splash FB: G#E1000000 (DRAM writes visible briefly during boot)

## Boot Format — CONFIRMED WORKING
- See [boot-format.md](boot-format.md) for full details
- boot.img v3, header_size=1580, uncompressed kernel
- PE/COFF headers required (ABL is UEFI-based, loads as PE)
- `fastboot boot` DOES NOT WORK — must flash to partition + set_active
- Recovery: `fastboot set_active b && fastboot reboot`

## PE/COFF Critical Details
- MZ must be valid ARM64: `0x91005A4D` (add x13, x18, #0x16)
- **SizeOfRawData must be file-backed data only** (excl BSS/pagetables)
  - Use `__bss_start - _start - 0x1000` NOT `__kernel_size`
  - Previous bug: SizeOfRawData = __kernel_size caused buffer overread, ABL crashed
- VirtualSize = `__kernel_size - 0x1000` (includes BSS, virtual extent)
- SizeOfImage = `__kernel_size` (total virtual)
- Stock uses: FileAlignment=0x200, NumRvaAndSizes=6, Subsystem=EFI_APPLICATION(10)

## Confirmed Working
- PSCI SYSTEM_RESET via SMC (0x84000009) — warm reboot preserving DRAM
- DRAM writes to splash FB (0xE1000000) — stable display output
- FB console with 8x16 VGA font, 2x scaled, margins for notch/corners
- GENI UART TX at 0x994000 (QUP1 SE5) — FW_REV=0x204, TX completes
- Pstore/ramoops log pipeline — see [pstore.md](pstore.md)
- SPMI read/write to PM8350C (flash LED), PM7325 (PON)
- DPU register reads (VIG0 SRC_ADDR confirms splash FB at 0xE1000000)
- USB enumeration as "Fairphone 5" (VID 0x1838, PID 0xFE01)
- **PT blast mode** — 20/20 multi-session diag, per-chunk BLAKE3 verification
  - No outbound SPEC ACK (response direction implicit)
  - No outbound FIN (receiver knows count from SPEC)
  - DWC3 ISP_IMI for short bulk IN packets
  - Idle-poll pump for outbound DATA, deferred from ep3 handler
- **Hot-reload over USB** — `ferros-bridge reload <kernel>` sends 72KB in ~15s
  - 1ms pacing between OUT sends (DWC3 single-TRB re-arm limitation)
  - Bridge pads all OUT to 512 bytes (short packets break DWC3 TRB chains)
  - ENDTRANSFER before each STARTTRANSFER for multi-packet inbound
- **SD card R/W** — 4-bit bus, 400KHz, multi-block read (CMD23+CMD18)
  - 1TB SanDisk SDXC, CID: SanDisk DSR01, RCA=0xAAAA
  - CSD v2: 953GB capacity, 25MHz max, 512-byte blocks
  - GCC SDC2 clock configured, RPMh LDO C9/C6 power enabled
  - 8-block write + multi-block read + verify: PASS
- **UFS R/W** — UFSHCI v3.0 at 0x1D84000, ABL leaves controller enabled
  - NOP OUT ping: OK
  - SCSI READ(10)/WRITE(10): verified with write-read-back at LBA 0x100000
  - Geometry: 232GB LUN0, 4KB blocks, 60.8M blocks, 4MB erase units
  - Query descriptors: device, geometry, unit (per-LUN)
- **Boot state ring on UFS** — proper VSF documents, binary search
  - vsf_mini.rs: no_std VSF encoder/decoder (~350 lines)
  - 65536 entries (256MB) at blocks 0x2000-0x11FFF
  - Each entry: RÅ< z(7) y(7) b() e() hp() n(1) > [(fields)]
  - hp = BLAKE3 of entire 4KB block with hp field zeroed
  - Binary search: 16 reads to find newest generation
  - Write-verify: write to UFS, read back, byte compare
  - Genesis + multi-generation cycle tested across hot-reloads
  - Anonymous sections (< 1MB from header, name in TOC only)

## Pstore / Ramoops — CONFIRMED WORKING
- Full pipeline: ferros writes → PSCI warm reboot → Android → adb pstore read
- ABL relocates reserved-memory between boots
- Must parse DTB each boot for current ramoops address
- persistent_ram_buffer header: sig=0x43474244 ("DBGC"), start(u32), size(u32), data[]

## Not Yet Working — Blocked by TZ/Qualcomm Lockdown
- **GIC**: GICD at G#17A00000 is TZ-protected. MMIO access causes exception → crash.
  - GIC driver written (gic.rs) but cannot be activated from EL1
  - WFI needs GIC to route DWC3 SPI 133 interrupt
  - **WORKAROUND FOUND**: ICC_PMR_EL1 + ICC_IGRPEN1_EL1 (EL1 registers) + WFI
  - ABL already configured GIC + DWC3 SPI 133 — we just open CPU interrupt mask
  - WFI works: CPU sleeps, wakes on USB events. Dramatic power reduction.
- **SPMI arbiter**: blocks most PMIC access
  - PON, SDAM, charger, RTC PIDs not in HLOS APID map
  - Charger on PM7250B SID 8 — locked to coprocessor EE
  - RTC (PID 0x61) completely absent — no wall clock
  - Battery status needs pmic-glink mailbox
- **Reboot-to-fastboot**: SDAM write "succeeds" but value doesn't persist
- **SD 25MHz clock**: needs DLL calibration in sdhci-msm vendor regs
- **UFS hot-reload**: SDHCI reset fails after hot reload (controller state dirty)
- **USB multi-session**: stale endpoint after failed transfer needs power cycle

## Hypervisor Approach (EL2) — BLOCKED
Probed QHEE: old-style closed hypervisor, no Gunyah API.
- HVC #0x8000 (Gunyah identify) → faults (exc=2)
- QCOM vendor HVC → faults
- Boot EL confirmed = 1 (QHEE owns EL2)
- Custom hyp flash = HIGH brick risk (secure boot chain)
- pKVM not used on Qualcomm
- **Alternative: pmic-glink** — mailbox to charger coprocessor
  - Gets RTC, charger, battery without EL2
  - Uses SMEM shared memory + IPCC doorbell
  - Research in progress

## Key Storage (Dev Target)
- **PAC registers**: 5 × 128-bit (640 bits total) — APIAKey, APIBKey, APDAKey, APDBKey, APGAKey
  - Writable at EL1, never touch RAM/cache
  - Repurposed from pointer auth (which ring memory eliminates)
  - Allocation: 256-bit ChaCha20 key + 128-bit nonce + 256-bit derived/temp
- **RPMB**: UFS hardware-backed, SHA256-HMAC, write-once key, can't be read back
- **Glyph target**: BLAKE3 silicon oracle with optical link

## Specs Written
- SEED.md — trust anchor (verify self + kernel, jump, <512 lines)
- BOOT.md — kernel boot sequence (8 stages, <500ms)
- KERNEL.md — running kernel (5 responsibilities, formal verification strategy)
- RING.md — ring mechanics, binary search, mirror protocol, stem/spine instances
- VAULT.md — persistent object store: tract, plow, HAMT, spine, object modes
- LEDGER.md — categorized event chain, VSF from entry zero
- SECURITY_CHAIN.md — seed→kernel→spine trust model, Ed25519 signatures
- ARCHITECTURE.md — why ferros eliminates Linux vulnerability classes
- HAMT.md — hash array mapped trie, COW, v_u0 bitmap, lone/direct/chained leaves

## Vault Architecture Terminology
- **Tract**: log-structured gravity ring, ~230GB, plow-managed
- **Plow**: sole write head for the tract, advances and wraps
- **Stem**: kernel ring (256 entries at G#C00), scanned by seed
- **Spine**: vault root ring (65536 entries at G#2000), commit log
- **Lone**: inline object in HAMT leaf (< ~3.9KB)
- **Direct**: furrow LBAs in HAMT leaf (< ~4MB)
- **Chained**: extent chain for large objects (> ~4MB)
- **Furrow**: extent data block, minimal VSF: m(index) + hp + hb + v(payload)
- **Deletion**: zero VSF magic on both disks, lazy HAMT cleanup during plow rotation
- **Batch commits**: multiple vault writes per spine entry, 1s timer or buffer-full trigger

## Partition Layout (unified 4KB blocks, same on UFS + SD)
- G#000-G#3FF: Reserved (ABL/GPT)
- G#400: Seed A, G#800: Seed B (>1MB apart)
- G#C00-G#CFF: Stem (256 entries, scanned by seed)
- G#2000-G#11FFF: Spine (256MB, 65536 entries)
- G#40000-G#7FFFF: State ring (1GB — procs, caps, display)
- G#80000-G#BFFFF: Ledger ring (1GB — categorized events)
- G#C0000+: Tract (~230GB, plow-managed)

## Project Structure
- ferros_layout: shared constants (partition layout, ring sizes, DRAM addresses, HW bases)
- ferros_seed: trust anchor (self-verify, kernel ring scan, kernel verify, jump)
- ferros_kernel: bare-metal aarch64 kernel (no_std)
- ferros_hal: MMIO, UART, FB, DTB, SD/MMC, DPU, SPMI, USB, GCC, RPMh, UFS drivers
  - `alloc` feature gates: sdmmc, console, usb, pmic_glink (seed uses HAL without alloc)
- ferros_pt: Photon Transport — blast mode, per-chunk BLAKE3, cap-addressed commands
- ferros_vault: persistent object store — mesh, anchors, caps
- ferros_ledger: EWE encoding, event types, preboot buffer
- tools/ferros-bridge: host-side USB bridge (diag, read, reload, reboot, status)
- tools/mkimg: ELF → flat binary → boot.img
- Build: `cargo build -p ferros_kernel --target aarch64-unknown-none --release`
- Image: `cargo run -p ferros-mkimg -- boot target/.../ferros_kernel -o ferros.img`
- Reload: `cargo run -p ferros-bridge -- reload target/.../ferros_kernel`

## User Preferences
- Pure Rust, no C dependencies
- Prefers direct hardware access over abstraction layers
- All numeric output uses [base36]#[number] format (G# for hex, A# for decimal) — NEVER 0x
- USB comms use serialized VSF + Photon Transport (not standard CDC-ACM)
- Binary OCD: use powers of 2 for counts, timeouts, loops — single bit taps, no comparators
- No quick fixes — do things right or don't do them
- Dozenal versioning: Zil(0), Zila(1), Zilor(2), Ter(3), ...
- EWE in kernel from day zero — no fixed-width integers on disk, ever
- Write-verify-then-mirror protocol for all persistent writes

## Memory Index
- [project_sd_card_progress.md](project_sd_card_progress.md) — SD card init fixes and status
- [project_fastboot_reboot.md](project_fastboot_reboot.md) — all fastboot reboot attempts and findings
- [project_pt_usb_review.md](project_pt_usb_review.md) — PT over USB status, DWC3 quirks, hot-reload
- [project_seed_fallbacks.md](project_seed_fallbacks.md) — seed SD fallback design, MUST implement before production
- [project_usb_auth_design.md](project_usb_auth_design.md) — USB auth: Ed25519 challenge + X25519 DH + ChaCha20 session
- [feedback_flash_cycle.md](feedback_flash_cycle.md) — build/flash/diag workflow and timing
- [feedback_number_format.md](feedback_number_format.md) — number format convention: [base36]#[number], never 0x
- [research_vault_architecture.md](research_vault_architecture.md) — vault design: RedoxFS analysis, HAMT+VSF, buddy allocator, mirroring, caps, encryption, names
- [feedback_boot_read_path.md](feedback_boot_read_path.md) — boot is all UFS reads, SD never touched unless BLAKE3 fails
- [project_target_hardware.md](project_target_hardware.md) — hardware pivot: Pixel 8 + M1 primary, FP5 bricked/deprioritized
