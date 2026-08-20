# FP5 Boot Image Format (CONFIRMED WORKING)

## Boot image header
- **Header version: 3** (v3 GKI format, matches stock)
- Page size: 4096 (implicit in v3)
- No kernel_addr/ramdisk_addr fields in v3 — ABL uses ARM64 Image header
- header_size: 1580 (0x62C)
- Stock kernel is UNCOMPRESSED (PE/COFF with MZ header)
- **v0, v1, v2 formats DO NOT WORK** (ABL rejects)

## PE/COFF Headers (REQUIRED — ABL is UEFI-based)
- MZ magic: `0x91005A4D` (valid ARM64: `add x13, x18, #0x16`)
  - MUST be valid ARM64 instruction! `0x00005A4D` is INVALID and may cause issues
- PE offset (e_lfanew) at ARM64 header byte 0x3C: 0x40
- Machine: 0xAA64, NumberOfSections: 1+, SizeOfOptionalHeader: 160
- COFF Characteristics: 0x0206
- PE32+ magic: 0x20B
- AddressOfEntryPoint: 0x1000 (RVA to _entry, page-aligned)
- SectionAlignment: 0x1000, FileAlignment: 0x200
- SizeOfImage: __kernel_size (total virtual size)
- SizeOfHeaders: 0x1000
- Subsystem: 10 (EFI Application)
- NumberOfRvaAndSizes: 6 (all empty, matches stock)

### Section Table — CRITICAL BUG FIX
- VirtualSize: `__kernel_size - 0x1000` (includes BSS, virtual extent)
- VirtualAddress: 0x1000
- **SizeOfRawData: `__bss_start - _start - 0x1000`** (FILE-BACKED ONLY!)
  - **BUG THAT BLOCKED BOOT FOR DAYS**: Using `__kernel_size` here caused ABL to
    read past end of kernel buffer (buffer overread), crashing PE loader before
    jumping to entry point. Behavior was identical to garbage image: FAIRPHONE hang.
- PointerToRawData: 0x1000
- Characteristics: 0xE0000020 (CODE|EXECUTE|READ|WRITE)

## ARM64 Image header
- text_offset: 0x80000 (matches stock)
- image_size: __kernel_size
- flags: 0x0A (LE, 4K pages, anywhere)
- ARM64 magic: "ARM\x64" at offset 0x38

## Confirmed Working
- PSCI SYSTEM_RESET via SMC (0x84000009) — reboots phone
- DRAM writes to splash FB (0xE1000000) — briefly visible during boot
- Code executes (confirmed by boot cycling + visible red framebuffer)
- GENI UART at 0x994000 (from stock cmdline, not yet tested from our code)

## Boot chain
- ABL loads kernel from boot partition (no decompression for PE/COFF)
- Parses PE/COFF, jumps to AddressOfEntryPoint (our _entry at RVA 0x1000)
- Linux kernel boots at EL2 or EL1 (VHE on stock)

## Display
- Panel: RM692E5 BOE AMOLED, DSI **command mode**
- Resolution: 1224x2700, XRGB8888, stride=4896
- Splash framebuffer: 0xE1000000 (reserved within System RAM)
- **Command mode: DPU must explicitly trigger frame transfers**
- MDSS/DPU base: 0x0AE00000

## System RAM layout (from /proc/iomem)
- 80600000-806fffff
- 80894000-808fefff
- 83600000-861fffff
- 8b71c000-8b7fffff
- 9c700000-b96fffff
- c5100000-d07fffff
- d8000000-df6fffff
- dff00000-1ffffffff (includes splash at e1000000-e33fffff)
- 200000000-27fffffff (upper DRAM)

## Device info
- Bootloader: FP5.UT2P.B.122.20250422
- A/B slots (slot-count: 2), Unlocked, QCM UFS variant
- Stock kernel: Linux 5.4.292-qgki

## Flashing
- `fastboot boot` does NOT work
- Must use `fastboot flash boot_a` + `fastboot set_active a` + reboot
- Recovery: `fastboot set_active b && fastboot reboot`

## Unsigned boot warning
- Shows on every boot (~10 seconds) with unlocked bootloader
