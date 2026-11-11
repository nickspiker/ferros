//! The static cache map (KERNEL.md, Execution Model): one identity translation table, written once at boot, so ferros's own working memory is cacheable.
//!
//! It translates nothing (virtual = physical) and isolates nothing — isolation is compiled masking plus EL0. It exists because on ARM, with the MMU off, every data access is Device memory: no data cache, exclusives (atomics) not guaranteed, unaligned accesses fault. Measured on husky with the MMU off: a 4 MiB copy in 157 ms, BLAKE3 at 1.6 MB/s.
//!
//! Layout (4 KiB granule, 39-bit VA, walk starts at level 1):
//! - Every 1 GiB block is Device-nGnRnE, exactly what the MMU-off world was, so nothing outside the window changes and nothing there is ever speculatively touched (secure carveouts stay safe).
//! - The GiB holding the window gets a level-2 table: the window's 2 MiB blocks are Normal write-back; the rest stay Device.
//! - The 2 MiB blocks holding the device DMA buffers (`.bss.dma`) get level-3 tables: those 4 KiB pages are Normal non-cacheable, so DMA never needs cache maintenance.
//! - DMA_POOL (cold only): 2 MiB of Normal non-cacheable memory a driver process reaches directly (EL0 read/write, never executable at any level) — the memory its device DMAs into (rings, buffers). It is granted like hardware (process.rs `DMA`), and the device's IOMMU/S2MPU window is meant to cover exactly it, so the device reaches nothing else.
//!
//! The window is ferros's image, stack, heap, staging slot and the heap's second region: 64 MiB from DRAM base to the display framebuffer on a cold boot, or the whole 12 MiB handoff carveout (0x9240_0000..0x9300_0000, tpu_fw_reserved after it).
//!
//! VHE (HCR_EL2.E2H + TGE, where the cores have it): the kernel runs as an EL2 host and processes run at EL0 under this same table, so a process gets cached memory and working atomics. The table then also carries one static split, never changed per process: EL0 may touch only the arc pool (POOL) and may execute only there; EL2 may not execute pool code (and architecturally can't anyway: privileged execution of EL0-writable memory is always a permission fault in the EL2&0 regime); the kernel, MMIO, DMA pages and the staging slot are EL2-only, so dev RUN payloads still execute at EL2 from the slot. Spawning a process copies its staged code into its arc in the pool. Isolation between processes stays compiled masking (KERNEL.md, Execution Model).
//! Switching E2H changes the layout of several EL2 registers — CPTR_EL2 then reads in CPACR form, where an unset FPEN traps FP/SIMD at EL2 and the kernel's own memcpy would fault — so the switch sets FPEN explicitly, and RELOAD restores the boot HCR_EL2/CPTR_EL2 before handing over.

use core::arch::asm;

#[repr(C, align(4096))]
struct Table([u64; 512]);

static mut L1: Table = Table([0; 512]);
static mut L2: Table = Table([0; 512]);
/// The level-2 table for the GiB above the window's (G#C000_0000-G#FFFF_FFFF): every 2 MiB block Device, as the 1 GiB block was, except the surfaces, sprites and canvas.
static mut L2_HI: Table = Table([0; 512]);
/// Level 2 for the first GiB (the MMIO below the window): Device blocks, EL2-only, except the USB controller's block, which EL0 may reach (`USB_REGS`).
static mut L2_LO: Table = Table([0; 512]);
static mut L3: [Table; 2] = [Table([0; 512]), Table([0; 512])];

/// The arcs' virtual windows (ARCS.md, 2026-11-11): every process's arc is a window of addresses here, its 2 MiB slots mapped to frames of the pool wherever they lie (frames.rs), so an arc grows by mapping more and moves by remapping, and no process ever sees an address change. 64 GiB from G#40_0000_0000: above every address the SoC decodes on either board (DRAM ends at G#B_0000_0000), inside the 39-bit space TTBR0 walks. Until 2026-11-11 arcs were the physical pools themselves, identity-mapped (POOL's and POOL_BIG's own level-3 tables, which this replaces).
/// Why windows and not a base register for every mode (ARCS.md proposed fixed and ring move by x28 alone): masked code keeps only the low bits of a data pointer, but its code pointers are whole addresses (return addresses on its stack, the vtables its relocations filled, function pointers), and a branch is not masked, so after a copy to a new base the first `ret` would jump into the old frames.
pub const ARC_VA: (usize, usize) = (0x40_0000_0000, 0x50_0000_0000);
/// Level 2 for each GiB of the windows: an entry per 2 MiB slot, pointing at the level-3 table of the frame mapped there (frames.rs `table`), or invalid.
static mut L2_ARCS: [Table; 64] = [const { Table([0; 512]) }; 64];
/// Level 2 for the GiBs from G#8_0000_0000 (husky's high bank) that hold frames: the frames Normal and the kernel's alone (it zeroes, copies and scrubs them by their own addresses), every other 2 MiB block Device, as the 1 GiB block was.
static mut L2_DRAM: [Table; 16] = [const { Table([0; 512]) }; 16];
/// The first GiB `L2_DRAM` covers.
const DRAM_GIB0: usize = 32;

unsafe extern "C" {
    static __dma_start: u8;
    static __dma_end: u8;
}

#[cfg(not(shim_handoff))]
const WINDOW: (usize, usize) = (0x8000_0000, 0x8400_0000);
/// Discard any stale lines over the heap's second region (left by the loader) before the heap uses it: `enable` leaves it out to keep its 26 MiB of line-by-line invalidation off the boot path. Nothing may have touched the region yet.
#[cfg(not(shim_handoff))]
pub fn heap2_prepare() {
    unsafe {
        asm!("dsb sy");
        let mut a = HEAP2.0;
        while a < HEAP2.1 {
            asm!("dc ivac, {}", in(reg) a);
            a += 64;
        }
        asm!("dsb sy");
    }
}

/// The kernel heap's second region (cold only): the window's top above the staging slot (STAGE G#8208_0000 + its 4 MiB, to the next 2 MiB) up to the display framebuffer. The first 12 MiB heap sits between the image and the arc pool and cannot grow; storing a 2.78 MB value needs a vault working set several times its size, and ran the heap out (2026-10-26).
#[cfg(not(shim_handoff))]
pub const HEAP2: (usize, usize) = (0x8260_0000, 0x8300_0000);
#[cfg(shim_handoff)]
const WINDOW: (usize, usize) = (0x9240_0000, 0x9300_0000);
/// The first range of the frame pool (frames.rs; until 2026-11-11 the page pool of arcs, EL0-accessible): kernel-only Normal memory now, reached by processes only through their windows (ARC_VA). A power of two aligned to its size, clear of the kernel image/stack/heap and the staging slot. Cold: 16 MiB at G#8300_0000, the top of the window, taken from the heap's second region (2026-10-26: the 8 MiB at G#8100_0000 held four guarded 1 MiB arcs — display, map, WiFi, IP — and refused the fifth process; G#8100_0000-G#8180_0000 is free now). Was: 8 MiB at G#8100_0000 (heap ends below it; staging slot G#8208_0000) — big enough for a driver process carrying multi-MiB firmware. Handoff (oracle only): 2 MiB at G#92C0_0000 (slot G#92A0_0000, stub G#92E8_0000).
#[cfg(not(shim_handoff))]
pub const POOL: (usize, usize) = (0x8300_0000, 0x8400_0000);
#[cfg(shim_handoff)]
pub const POOL: (usize, usize) = (0x92C0_0000, 0x92E0_0000);

/// The second range of the frame pool (frames.rs; until 2026-11-11 the big arc pool, cold only): 320 MiB of Normal write-back DRAM at G#A000_0000, kernel-only 2 MiB blocks of the window's GiB table, processes reaching its frames through their windows (ARC_VA). Until then it held arcs of 16 MiB and up (a std process: Photon's heap, a fluor frame's scratch), where the 16 MiB page pool holds the small ones and the masked ones. Nothing in the device tree reserves G#9800_0000-G#C000_0000 (the TPU, GSA and modem carveouts sit below G#9740_0000 and from G#C000_0000; ferros-ref soc-gs dts, 2026-10-27); W^X lives in each frame's own level-3 table now.
#[cfg(not(shim_handoff))]
pub const POOL_BIG: (usize, usize) = (0xA000_0000, 0xB400_0000);

/// The cellular modem's shared memory, EL0-accessible Device memory for its driver process (process.rs DMA `modem-pktproc`, `modem-ipc`, `modem-msi`; zuma-rmem.dtsi carveouts): pktproc's rings and packet buffers (32 MiB), the IPC region (legacy rings, handover block, ROM image; 4 MiB), the MSI and boot-register page. Device, as the kernel's own view of them: the CP writes them over PCIe and no cache line may hide that. 2 MiB aligned, in the GiB above the window.
pub const MODEM_WINDOWS: [(usize, usize); 3] = [(0xE800_0000, 0xEA00_0000), (0xEA40_0000, 0xEA80_0000), (0xF620_0000, 0xF640_0000)];
/// The USB controller's registers (DWC3, G#1121_0000, 64 KiB), EL0 Device memory for the USB driver process (USB.md: USB out of the kernel), its 2 MiB block mapped as the modem's windows are: nothing else lives in G#1120_0000-G#1140_0000 (zuma device tree). Granted as `usb-regs`.
pub const USB_REGS: (usize, usize) = (0x1120_0000, 0x1140_0000);
/// The USB driver's DMA memory (cold only): 8 MiB of EL0 read/write memory, 8 MiB aligned: the controller's event ring, TRBs and buffers (ferros_hal::usb::UsbMem, the run buffer) in the first 2 MiB, Normal non-cacheable (`USB_DMA_NC`); a PT command assembled in the 6 MiB after (payloads/usb), cacheable. No reserved-memory node of the device tree lies below G#9000_0000, so this is ferros's own: above the camera's diagnostic buffers (which end at G#8B10_0000), up to the display's dither phases (G#8C00_0000). Granted as `usb-dma`.
#[cfg(not(shim_handoff))]
pub const USB_DMA: (usize, usize) = (0x8B80_0000, 0x8C00_0000);
/// How much of `USB_DMA`, from its start, is non-cacheable: the controller's block and the run buffer, which the controller reads and writes. The rest is the driver's command area, which only the CPU touches (the process, and the kernel copying a command out), so it is Normal cacheable, EL0, never executable: decoding 2 MiB of packets straight out of non-cacheable memory took the process 2.2 s of CPU (2026-10-31).
pub const USB_DMA_NC: usize = 2 << 20;
/// The AoC's IPC control block and its services' rings (aoc.rs IPC: the carve-out at G#9400_0000 plus G#1C0_0000), EL0 Device memory for the sensors process (KERNEL-OUT.md move 4): com.google.usf's rings lie in its first G#85000 bytes. The rest of the carve-out (the firmware, its heap) stays the kernel's. Granted as `aoc-ipc`.
pub const AOC_IPC: (usize, usize) = (0x95C0_0000, 0x95E0_0000);
#[cfg(shim_handoff)]
pub const USB_DMA: (usize, usize) = (0, 0);
#[cfg(shim_handoff)]
pub const POOL_BIG: (usize, usize) = (0, 0);

/// The GPU's registers (Mali-G715 at G#1F00_0000, 16 MiB: zuma-gpu.dtsi `reg`; its doorbell pages inside, from G#1F08_0000), EL0 Device memory for the GPU driver process (GPU.md), mapped as the USB controller's block is: nothing else lives in G#1F00_0000-G#2000_0000. Granted as `gpu-regs`.
pub const GPU_REGS: (usize, usize) = (0x1F00_0000, 0x2000_0000);
/// The GPU's memory (cold only): 32 MiB of Normal non-cacheable, EL0 read/write memory the GPU driver process and the GPU share — the MCU's firmware sections, the GPU's page tables, the shared interface, rings and buffers. Non-cacheable because the GPU does not snoop the CPU's caches (GPU.md). The top 32 MiB of what was the big arc pool, just below the secure DRAM carveout at G#B600_0000 (cbff939; it is in no reserved-memory node, so the device tree does not show it: the first gpu-mem, at G#B800_0000 inside it, took writes silently and aborted every read, 2026-11-04). The GPU's S2MPU lets it reach this and nothing else (s2mpu::fence_gpu). Granted as `gpu-mem`.
#[cfg(not(shim_handoff))]
pub const GPU_MEM: (usize, usize) = (0xB400_0000, 0xB600_0000);
/// Its last 2 MiB is Normal write-back on the CPU's side, the rest non-cacheable: where the GPU's I/O coherency (ACE-Lite, COHERENCY_FEATURES G#1) is tried, the CPU writing through its caches and the GPU snooping them (GPU.md "Coherency"). The GPU driver's allocator stays below it.
#[cfg(not(shim_handoff))]
pub const GPU_SNOOP: usize = GPU_MEM.1 - (2 << 20);
#[cfg(shim_handoff)]
pub const GPU_SNOOP: usize = 0;
/// The layer arena (cold only; GPU.md "Layers", owner 2026-11-05): 64 MiB of Normal write-back, EL0 read/write, never executable memory where the GPU writes compute-dense layers (the Mandelbrot, glows) and the CPU composes them, or the CPU writes and the GPU reads, with no clean and no copy: the GPU maps it coherent and snoops the CPU's caches (GPU.md "Coherency"). In the GPU's fence beside gpu-mem. Free DRAM: the device tree reserves nothing in G#9800_0000-G#A000_0000 (USB's 4 MiB end at G#9740_0000). Granted as `layers`.
#[cfg(not(shim_handoff))]
pub const LAYERS: (usize, usize) = (0x9C00_0000, 0xA000_0000);
#[cfg(shim_handoff)]
pub const LAYERS: (usize, usize) = (0, 0);
#[cfg(shim_handoff)]
pub const GPU_MEM: (usize, usize) = (0, 0);

/// Device DMA memory for driver processes (see the module doc): cold G#8180_0000, the 2 MiB above the pool, below the reload stub (G#8207_0000). None on the handoff path.
#[cfg(not(shim_handoff))]
pub const DMA_POOL: (usize, usize) = (0x8180_0000, 0x81A0_0000);
#[cfg(shim_handoff)]
pub const DMA_POOL: (usize, usize) = (0, 0);

/// The display's framebuffers (cold only): 32 MiB of Normal non-cacheable, EL0 read/write memory above the window (two WQHD BGRX8888 surfaces of 15.3 MiB), the DPU's DMA target once ferros stops painting on ABL's splash surface. Granted as the DMA region `display-fb`. Free DRAM on a cold boot: Linux's image would be here, and the first reserved carveout (zuma-rmem.dtsi) is at G#9000_0000.
#[cfg(not(shim_handoff))]
pub const DISPLAY_FB: (usize, usize) = (0x8400_0000, 0x8600_0000);
#[cfg(shim_handoff)]
pub const DISPLAY_FB: (usize, usize) = (0, 0);

/// A cacheable heap for the display server (cold only): 32 MiB of Normal write-back, EL0 read/write, never executable DRAM above the framebuffers — a process heap outside its arc, so the 8 MiB arc pool doesn't have to carry it and the non-cacheable framebuffer region doesn't slow it (a typeface parse over non-cacheable memory ran for minutes, 2026-10-16). Its top 16 MiB is the display server's back buffer, where frames are composited before one write-only copy to the framebuffer. It ends where the camera's buffers begin (G#8800_0000). Granted as the region `display-heap`; nothing DMAs into it.
#[cfg(not(shim_handoff))]
pub const DISPLAY_HEAP: (usize, usize) = (0x8600_0000, 0x8800_0000);
#[cfg(shim_handoff)]
pub const DISPLAY_HEAP: (usize, usize) = (0, 0);

/// The display's temporal dither phases (cold only): 64 MiB of Normal non-cacheable, EL0 read/write memory for four 16 MiB frames the display server writes once per content change and the layer flips between every 100 ms (DISPLAY-CALIB.md, the present stage). Free DRAM on a cold boot: above the camera's diagnostic buffers (which end at G#8B10_0000) and below the first firmware carveout (G#9000_0000). Granted as the DMA region `display-phases`.
#[cfg(not(shim_handoff))]
pub const DISPLAY_PHASES: (usize, usize) = (0x8C00_0000, 0x9000_0000);
#[cfg(shim_handoff)]
pub const DISPLAY_PHASES: (usize, usize) = (0, 0);

/// The apps' surfaces (cold only): 128 MiB of Normal write-back, EL0 read/write, never executable memory, four full-panel surfaces of 32 MiB (1344 × 2992 pixels of 64 bits each). Each app is given one at spawn and draws there as it wishes; the display server holds all four and composes the windows frontmost first (WINDOWS.md). Free per the device tree: above the display's canvas (G#CA00_0000) and below the debug snapshot (G#D810_0000; the modem's carveouts are G#E800_0000 up). Replaced the single `app-surface` at G#C000_0000 (2026-10-27), which overlapped the modem image ABL loads there.
#[cfg(not(shim_handoff))]
pub const SURFACES: (usize, usize) = (0xCA00_0000, 0xD200_0000);
#[cfg(shim_handoff)]
pub const SURFACES: (usize, usize) = (0, 0);
/// One surface's size: a power of two that holds a full panel.
pub const SURFACE_SIZE: usize = 32 << 20;
/// The first 64 bytes of every surface are a porch nobody draws in (SANDBOX.md, "Handles"): a surface's handle spans its whole 32 MiB, and an access through it at the last offset reads up to its width less one byte past the end (63 for a structure load), into the next surface. With the porch that lands in the next surface's porch, never in its pixels. A process is told its surface 64 bytes in (SYS_DMA), so pixel 0 is at the porch's end for native pointers and handle offsets alike; a screen's pixels (1344 x 2992 x 8 bytes) fit the rest.
pub const SURFACE_PORCH: usize = 64;

/// The display's sprite cache (cold only): 32 MiB of Normal write-back, EL0 read/write, never executable memory in the same GiB, 32 MiB clear of the apps' surface (in case the bootloader's modem image runs past it): every circle's drawn pixels, kept while its size and turn stay, so a pan blits rather than redraws. A screen packed with circles needs ~20 MB of square sprites; the 6 MiB the display's heap could spare thrashed at 200 circles (2026-10-24: every icon redrawn every frame, 11 ms). Free per the device tree (the first carveout in this GiB is G#E800_0000). Granted as the region `display-sprites`.
#[cfg(not(shim_handoff))]
pub const DISPLAY_SPRITES: (usize, usize) = (0xC400_0000, 0xC800_0000);
#[cfg(shim_handoff)]
pub const DISPLAY_SPRITES: (usize, usize) = (0, 0);

/// The display's canvas (cold only): 32 MiB of Normal write-back, EL0 read/write, never executable memory right above the sprites — the back buffer where frames are composited, 16 bits a channel (a WQHD frame of 64-bit pixels is 30.7 MiB; the 8-bit canvas lived in the top half of the display heap). Granted as the region `display-canvas`. 2026-10-24: the sprite region doubled with it to 64 MiB (sprites are 64-bit pixels too).
#[cfg(not(shim_handoff))]
pub const DISPLAY_CANVAS: (usize, usize) = (0xC800_0000, 0xCA00_0000);
#[cfg(shim_handoff)]
pub const DISPLAY_CANVAS: (usize, usize) = (0, 0);

/// HCR_EL2 / CPTR_EL2 as the loader left them, restored before RELOAD hands over.
static mut BOOT_HCR: u64 = 0;
static mut BOOT_CPTR: u64 = 0;
static mut VHE: bool = false;
const HCR_TGE: u64 = 1 << 27;
const HCR_E2H: u64 = 1 << 34;

const GIB: usize = 1 << 30;
const MIB2: usize = 1 << 21;
const PAGE: usize = 1 << 12;

// MAIR_EL2: Attr0 Device-nGnRnE (G#00), Attr1 Normal write-back read/write-allocate (G#FF), Attr2 Normal non-cacheable (G#44).
const MAIR: u64 = 0x00 | (0xFF << 8) | (0x44 << 16);
const ATTR_DEVICE: u64 = 0 << 2;
const ATTR_NORMAL: u64 = 1 << 2;
const ATTR_NC: u64 = 2 << 2;
// Descriptor bits. Non-VHE EL2 regime: AP[1] (bit 6) is RES1, one XN (bit 54). VHE (EL2&0 regime): AP[1] = EL0 access, UXN (bit 54) / PXN (bit 53). SH inner shareable (bits 9:8), AF (bit 10).
const AP_EL2: u64 = 1 << 6;
const AP_EL0: u64 = 1 << 6;
/// AP[2]: read-only (for either EL in the VHE regime).
const AP_RO: u64 = 1 << 7;
const SH_INNER: u64 = 3 << 8;
const AF: u64 = 1 << 10;
const XN: u64 = 1 << 54;
const UXN: u64 = 1 << 54;
const PXN: u64 = 1 << 53;
const BLOCK: u64 = 0b01;
const TABLE_OR_PAGE: u64 = 0b11;

fn device(vhe: bool, addr: usize, kind: u64) -> u64 {
    addr as u64 | kind | ATTR_DEVICE | AF | if vhe { UXN | PXN } else { AP_EL2 | XN }
}
/// Device memory EL0 may read and write (VHE), never executable: the modem's shared memory (`MODEM_WINDOWS`).
fn device_el0(vhe: bool, addr: usize, kind: u64) -> u64 {
    addr as u64 | kind | ATTR_DEVICE | AF | if vhe { AP_EL0 | UXN | PXN } else { AP_EL2 | XN }
}
fn normal(vhe: bool, addr: usize, kind: u64) -> u64 {
    addr as u64 | kind | ATTR_NORMAL | SH_INNER | AF | if vhe { UXN } else { AP_EL2 }
}
fn pool(vhe: bool, addr: usize, kind: u64) -> u64 {
    if vhe { addr as u64 | kind | ATTR_NORMAL | SH_INNER | AF | AP_EL0 | PXN } else { normal(false, addr, kind) }
}
fn dma_pool(vhe: bool, addr: usize, kind: u64) -> u64 {
    addr as u64 | kind | ATTR_NC | SH_INNER | AF | if vhe { AP_EL0 | UXN | PXN } else { AP_EL2 | XN }
}
/// A pool page (VHE EL0): Normal cacheable, EL0-accessible, never executable at EL2 (PXN). `writable` clears AP[2]; `exec` clears UXN so EL0 may execute it. Default (the state a freed/fresh arc page is left in) is writable, non-executable — data.
fn pool_page(vhe: bool, addr: usize, writable: bool, exec: bool) -> u64 {
    if !vhe {
        return normal(false, addr, TABLE_OR_PAGE);
    }
    let mut d = addr as u64 | TABLE_OR_PAGE | ATTR_NORMAL | SH_INNER | AF | AP_EL0 | PXN;
    if !writable {
        d |= AP_RO;
    }
    if !exec {
        d |= UXN;
    }
    d
}
fn uncached(vhe: bool, addr: usize, kind: u64) -> u64 {
    addr as u64 | kind | ATTR_NC | SH_INNER | AF | if vhe { UXN | PXN } else { AP_EL2 | XN }
}
/// A frame of the pool at its own address (frames.rs): Normal write-back, the kernel's alone (no EL0 access), never executable.
fn frame_block(vhe: bool, addr: usize) -> u64 {
    addr as u64 | BLOCK | ATTR_NORMAL | SH_INNER | AF | if vhe { UXN | PXN } else { AP_EL2 | XN }
}
/// Whether the 2 MiB block at `a` is a frame of the pool.
fn in_frames(a: usize) -> bool {
    crate::frames::map().iter().any(|&(lo, hi)| a >= lo && a + MIB2 <= hi)
}
/// A page the kernel may read and write and no process may touch: an arc's guard, the page under its first stack, the slack of its last frame past its size.
fn kernel_page(addr: usize) -> u64 {
    addr as u64 | TABLE_OR_PAGE | ATTR_NORMAL | SH_INNER | AF | PXN | UXN
}
/// The output address bits of a descriptor (47:12).
const OA: u64 = 0x0000_FFFF_FFFF_F000;

/// Whether the cores implement VHE (ID_AA64MMFR1_EL1.VH, bits 11:8).
fn has_vhe() -> bool {
    let v: u64;
    unsafe { asm!("mrs {}, ID_AA64MMFR1_EL1", out(reg) v) };
    (v >> 8) & 0xF != 0
}

/// Whether ferros switched to VHE (processes can run at EL0).
pub fn vhe() -> bool {
    unsafe { VHE }
}

fn dma_range() -> (usize, usize) {
    unsafe { (&raw const __dma_start as usize, &raw const __dma_end as usize) }
}

/// Build the table and turn the MMU and data cache on. Call once, first thing in kernel_main (after the killswitch). Returns false (and changes nothing) if the layout doesn't fit.
pub fn enable() -> bool {
    let (w0, w1) = WINDOW;
    let (d0, d1) = dma_range();
    let (p0, p1) = POOL;
    let (q0, q1) = DMA_POOL;
    let (f0, f1) = DISPLAY_FB;
    let (h0, h1) = DISPLAY_HEAP;
    let (t0, t1) = DISPLAY_PHASES;
    let dma_ok = q0 == q1 || (q0 % MIB2 == 0 && q1 % MIB2 == 0 && q0 >= w0 && q1 <= w1 && (q1 <= p0 || q0 >= p1) && (q1 <= d0 || q0 >= d1));
    // The framebuffers and the display heap sit outside the window but in its GiB (so the level-2 table covers them), 2 MiB aligned, apart from each other.
    let outside = |a: usize, b: usize| a == b || (a % MIB2 == 0 && b % MIB2 == 0 && a / GIB == w0 / GIB && (b - 1) / GIB == w0 / GIB && (b <= w0 || a >= w1));
    let fb_ok = outside(f0, f1) && outside(h0, h1) && (f1 <= h0 || h1 <= f0 || f0 == f1 || h0 == h1) && outside(t0, t1) && (t0 == t1 || (t0 >= f1.max(h1)));
    if w0 / GIB != (w1 - 1) / GIB || d0 < w0 || d1 > w1 || (d1 - 1) / MIB2 - d0 / MIB2 >= 2 || p0 < w0 || p1 > w1 || (d0 < p1 && p0 < d1) || !dma_ok || !fb_ok {
        return false;
    }
    let vhe = has_vhe();
    unsafe {
        let l1 = &mut *(&raw mut L1);
        let l2 = &mut *(&raw mut L2);
        let l3 = &mut *(&raw mut L3);
        for (i, e) in l1.0.iter_mut().enumerate() {
            *e = device(vhe, i * GIB, BLOCK);
        }
        // The first GiB at 2 MiB granularity, so the USB controller's block can be EL0's and every other MMIO block stays the kernel's.
        let l2lo = &mut *(&raw mut L2_LO);
        for (j, e) in l2lo.0.iter_mut().enumerate() {
            let a = j * MIB2;
            *e = if (a >= USB_REGS.0 && a + MIB2 <= USB_REGS.1) || (a >= GPU_REGS.0 && a + MIB2 <= GPU_REGS.1) { device_el0(vhe, a, BLOCK) } else { device(vhe, a, BLOCK) };
        }
        l1.0[0] = &raw const L2_LO as u64 | TABLE_OR_PAGE;
        let gib = w0 / GIB * GIB;
        l1.0[w0 / GIB] = &raw const L2 as u64 | TABLE_OR_PAGE;
        let mut next_l3 = 0;
        for (j, e) in l2.0.iter_mut().enumerate() {
            let a = gib + j * MIB2;
            *e = if (f0 != f1 && a >= f0 && a + MIB2 <= f1) || (t0 != t1 && a >= t0 && a + MIB2 <= t1) || (USB_DMA.0 != USB_DMA.1 && a >= USB_DMA.0 && a + MIB2 <= USB_DMA.0 + USB_DMA_NC) || (GPU_MEM.0 != GPU_MEM.1 && a >= GPU_MEM.0 && a + MIB2 <= GPU_SNOOP) {
                dma_pool(vhe, a, BLOCK)
            } else if a >= AOC_IPC.0 && a + MIB2 <= AOC_IPC.1 {
                device_el0(vhe, a, BLOCK)
            } else if (h0 != h1 && a >= h0 && a + MIB2 <= h1) || (USB_DMA.0 != USB_DMA.1 && a >= USB_DMA.0 + USB_DMA_NC && a + MIB2 <= USB_DMA.1) || (GPU_MEM.0 != GPU_MEM.1 && a == GPU_SNOOP) || (LAYERS.0 != LAYERS.1 && a >= LAYERS.0 && a + MIB2 <= LAYERS.1) {
                pool(vhe, a, BLOCK) | UXN
            } else if in_frames(a) {
                // A frame of the pool (the page pool's and the big pool's ranges until 2026-11-11): the kernel's, by its own address; a process reaches it only through its window.
                frame_block(vhe, a)
            } else if a + MIB2 <= w0 || a >= w1 {
                device(vhe, a, BLOCK)
            } else if a >= q0 && a + MIB2 <= q1 {
                dma_pool(vhe, a, BLOCK)
            } else if a + MIB2 > d0 && a < d1 {
                let t = &mut l3[next_l3];
                for (k, p) in t.0.iter_mut().enumerate() {
                    let pa = a + k * PAGE;
                    *p = if pa >= d0 && pa < d1 { uncached(vhe, pa, TABLE_OR_PAGE) } else { normal(vhe, pa, TABLE_OR_PAGE) };
                }
                let d = &raw const t.0 as u64 | TABLE_OR_PAGE;
                next_l3 += 1;
                d
            } else {
                normal(vhe, a, BLOCK)
            };
        }
        // The GiB above: Device blocks as before (the ramoops block, ABL's splash, the carveouts all live there), the apps' surfaces and the display's sprites and canvas Normal.
        let (s0, s1) = SURFACES;
        let (p0, p1) = DISPLAY_SPRITES;
        let (q0, q1) = DISPLAY_CANVAS;
        if s0 != s1 && s0 / GIB == w0 / GIB + 1 && s0 % MIB2 == 0 && s1 % MIB2 == 0 && (s1 - 1) / GIB == s0 / GIB {
            let l2h = &mut *(&raw mut L2_HI);
            let base = s0 / GIB * GIB;
            for (j, e) in l2h.0.iter_mut().enumerate() {
                let a = base + j * MIB2;
                let normal_here = (a >= s0 && a + MIB2 <= s1) || (p0 != p1 && a >= p0 && a + MIB2 <= p1) || (q0 != q1 && a >= q0 && a + MIB2 <= q1);
                let modem_here = MODEM_WINDOWS.iter().any(|w| a >= w.0 && a + MIB2 <= w.1);
                *e = if normal_here { pool(vhe, a, BLOCK) | UXN } else if modem_here { device_el0(vhe, a, BLOCK) } else { device(vhe, a, BLOCK) };
            }
            l1.0[s0 / GIB] = &raw const L2_HI as u64 | TABLE_OR_PAGE;
        }
        // The high bank's GiBs that hold frames (frames.rs: husky's 9 GiB from G#8_8000_0000): the frames Normal and the kernel's, every other block Device as the GiB was.
        let l2d = &mut *(&raw mut L2_DRAM);
        for (g, t) in l2d.iter_mut().enumerate() {
            let base = (DRAM_GIB0 + g) * GIB;
            if !crate::frames::map().iter().any(|&(lo, hi)| lo < base + GIB && hi > base) {
                continue;
            }
            for (j, e) in t.0.iter_mut().enumerate() {
                let a = base + j * MIB2;
                *e = if in_frames(a) { frame_block(vhe, a) } else { device(vhe, a, BLOCK) };
            }
            l1.0[DRAM_GIB0 + g] = &raw const t.0 as u64 | TABLE_OR_PAGE;
        }
        // The arcs' windows: every GiB's level-2 table in place, every slot invalid until an arc maps a frame there.
        for (g, t) in (*(&raw const L2_ARCS)).iter().enumerate() {
            l1.0[ARC_VA.0 / GIB + g] = &raw const t.0 as u64 | TABLE_OR_PAGE;
        }
        // The tables went to DRAM with the MMU off. Discard any stale lines over the window (left by the loader; ours bypassed the cache) before the window becomes cacheable — including the table walker's view of the tables. Not the heap's second region (cold): it is discarded later, off the boot path, before the heap takes it (`heap2_prepare`; its 26 MiB of lines cost the boot ~235 ms here, 2026-10-26).
        asm!("dsb sy");
        let mut a = w0;
        #[cfg(not(shim_handoff))]
        let w1 = HEAP2.0;
        while a < w1 {
            asm!("dc ivac, {}", in(reg) a);
            a += 64;
        }
        // Record the boot HCR/CPTR, then (VHE) switch E2H + TGE with FP/SIMD explicitly untrapped (CPTR_EL2 in CPACR form: FPEN bits 21:20 = 0b11).
        let hcr: u64;
        let cptr: u64;
        asm!("mrs {}, hcr_el2", "mrs {}, cptr_el2", out(reg) hcr, out(reg) cptr);
        BOOT_HCR = hcr;
        BOOT_CPTR = cptr;
        if vhe {
            asm!(
                "msr hcr_el2, {h}",
                "isb",
                "msr cptr_el2, {c}",
                "isb",
                h = in(reg) hcr | HCR_E2H | HCR_TGE,
                c = in(reg) 3u64 << 20,
            );
            VHE = true;
            // EL0 may read the counters (CNTHCTL_EL2 in its E2H layout: EL0PCTEN b0, EL0VCTEN b1), so processes can time their own delays.
            asm!("mrs {t}, cnthctl_el2", "orr {t}, {t}, #3", "msr cnthctl_el2, {t}", "isb", t = out(reg) _);
            crate::pmu::el0_on();
        }
        // TCR_EL2. Non-VHE: T0SZ 25, inner/outer write-back walks, inner shareable, 4 KiB granule, 40-bit PA, RES1 bits 23 and 31. VHE (TCR_EL1 layout): the same for TTBR0, TTBR1 walks disabled (EPD1, TG1 = 4 KiB), IPS 40-bit.
        let tcr: u64 = if vhe {
            25 | (1 << 8) | (1 << 10) | (3 << 12) | (25 << 16) | (1 << 23) | (2 << 30) | (2 << 32)
        } else {
            25 | (1 << 8) | (1 << 10) | (3 << 12) | (2 << 16) | (1 << 23) | (1 << 31)
        };
        asm!(
            "dsb sy",
            "msr mair_el2, {mair}",
            "msr tcr_el2, {tcr}",
            "msr ttbr0_el2, {ttbr}",
            "isb",
            "tlbi alle2",
            "dsb sy",
            "isb",
            "mrs {t}, sctlr_el2",
            "orr {t}, {t}, #(1 << 0)",
            "orr {t}, {t}, #(1 << 2)",
            "orr {t}, {t}, #(1 << 12)",
            // VHE meanings (RES1/RES0 otherwise): SPAN (23) = 1 so taking an exception never sets PAN (the kernel reads arcs); UMA (9) = 0 so EL0 cannot mask interrupts (the killswitch); UCI (26), DZE (14), UCT (15) = 0.
            "orr {t}, {t}, #(1 << 23)",
            "bic {t}, {t}, #(1 << 9)",
            "bic {t}, {t}, #(1 << 26)",
            "bic {t}, {t}, #(1 << 14)",
            "bic {t}, {t}, #(1 << 15)",
            "msr sctlr_el2, {t}",
            "isb",
            mair = in(reg) MAIR,
            tcr = in(reg) tcr,
            ttbr = in(reg) &raw const L1 as u64,
            t = out(reg) _,
        );
        // BTI for EL0 (SCTLR_EL2.BT0 under VHE): enforced only on guarded pages, which only BTI capsules' code gets (set_arc_wx), so nothing else changes.
        if vhe {
            asm!("mrs {t}, sctlr_el2", "orr {t}, {t}, #(1 << 35)", "msr sctlr_el2, {t}", "isb", t = out(reg) _);
        }
    }
    true
}

/// Whether the MMU is on (SCTLR_EL2.M).
pub fn is_on() -> bool {
    let s: u64;
    unsafe { asm!("mrs {}, sctlr_el2", out(reg) s) };
    s & 1 != 0
}

/// Before RELOAD's copy stub: write every dirty line in the window back to DRAM and drop it, then turn the MMU and caches off, so the stub copies the staged image with the MMU off (as the next image expects) and no stale line can later be written back over it. IRQs must already be masked.
pub fn off_for_handover() {
    if !is_on() {
        return;
    }
    let (w0, w1) = WINDOW;
    unsafe {
        asm!("dsb sy");
        let mut a = w0;
        while a < w1 {
            asm!("dc civac, {}", in(reg) a);
            a += 64;
        }
        asm!(
            "dsb sy",
            "mrs {t}, sctlr_el2",
            "bic {t}, {t}, #(1 << 0)",
            "bic {t}, {t}, #(1 << 2)",
            "msr sctlr_el2, {t}",
            "isb",
            "tlbi alle2",
            "ic ialluis",
            "dsb sy",
            "isb",
            // Back to the loader's register layout (non-VHE), as the next image expects.
            "msr hcr_el2, {h}",
            "isb",
            "msr cptr_el2, {c}",
            "isb",
            t = out(reg) _,
            h = in(reg) BOOT_HCR,
            c = in(reg) BOOT_CPTR,
        );
        VHE = false;
    }
}

/// Set the W^X attributes of an arc's pages (SANDBOX.md), then invalidate the TLB for them. A masked process marks `[exec_start, exec_end)` read-only + executable (its checked `.text`) and leaves every other page writable + non-executable (grants, read-only data, input, stack). A native dev process (`native`) gets the whole arc RWX. Call after writing the code, before entering. Each page keeps the frame it maps; a freed arc's frames go back with fresh tables (frames.rs `give`), so nothing resets them here.
pub fn set_arc_wx(base: usize, size: usize, exec_start: usize, exec_end: usize, native: bool, guarded: bool) {
    if !vhe() {
        return;
    }
    if page_entry(base).is_none() || page_entry(base + size - PAGE).is_none() {
        return;
    }
    unsafe {
        let mut a = base;
        while a < base + size {
            let exec = native || (a >= exec_start && a < exec_end);
            let writable = native || !exec;
            let e = page_entry(a).unwrap();
            // A BTI capsule's code on guarded pages (GP, bit 50): every indirect branch into them must land on a `bti` instruction (SCTLR_EL2.BT0, set at boot), so no jump enters a multi-instruction check midway.
            *e = pool_page(true, (*e & OA) as usize, writable, exec) | if guarded && exec && !native { 1 << 50 } else { 0 };
            a += PAGE;
        }
        asm!("dsb ish");
        let mut a = base;
        while a < base + size {
            // Inner shareable: every core's TLB, not this one's — the workers run processes too (smp.rs).
            asm!("tlbi vae1is, {x}", x = in(reg) (a as u64) >> 12);
            a += PAGE;
        }
        asm!("dsb ish", "isb");
    }
}

/// Make freshly written code at [addr, addr + len) executable: clean it to the point of unification, then invalidate the I-cache.
pub fn sync_code(addr: usize, len: usize) {
    unsafe {
        let mut a = addr & !63;
        while a < addr + len {
            asm!("dc cvau, {}", in(reg) a);
            a += 64;
        }
        asm!("dsb ish", "ic ialluis", "dsb ish", "isb");
    }
}

/// The grants page (SANDBOX.md, "Handles"): the arc's first 4 KiB, the handle table, the tokens, the preferences and the ihi. Read-only to everyone once the kernel has written it (the map has no "kernel writes, the process does not": a page is read-only at every level or at none), so a process cannot forge a handle; the kernel opens it to write a preference and closes it again. Non-executable always.
pub fn grants_page(base: usize, writable: bool) {
    if !vhe() {
        return;
    }
    let a = base;
    let Some(e) = page_entry(a) else { return };
    unsafe {
        *e = pool_page(true, (*e & OA) as usize, writable, false);
        asm!("dsb ish");
        asm!("tlbi vae1is, {x}", x = in(reg) (a as u64) >> 12);
        asm!("dsb ish", "isb");
    }
}

/// `len` bytes of an arc from `base` made kernel-only: the page under its first thread's stack (process.rs STACK_GUARD), so a stack run past its end faults with the process named. The kernel keeps its own access: a syscall buffer may name it, and the kernel's write there must not fault.
pub fn guard_pages(base: usize, len: usize) {
    if !vhe() {
        return;
    }
    unsafe {
        let mut a = base;
        while a < base + len {
            if let Some(e) = page_entry(a) {
                *e = kernel_page((*e & OA) as usize);
            }
            a += PAGE;
        }
        asm!("dsb ish");
        let mut a = base;
        while a < base + len {
            asm!("tlbi vae1is, {x}", x = in(reg) (a as u64) >> 12);
            a += PAGE;
        }
        asm!("dsb ish", "isb");
    }
}

/// Map frame `pa` at the window slot `va` (2 MiB aligned, inside ARC_VA) through the frame's own level-3 table at `table` (frames.rs): its first `usable` bytes as the process's data (writable, never executable; `set_arc_wx` makes code of them), the rest kernel-only (the slack of an arc smaller than its frames, which is a masked arc's guard). The slot must be unmapped. Call `mapped` once the arc's slots are in.
pub fn map_slot(va: usize, pa: usize, table: usize, usable: usize) {
    let t = unsafe { &mut *(table as *mut [u64; 512]) };
    for (k, e) in t.iter_mut().enumerate() {
        let a = pa + k * PAGE;
        *e = if k * PAGE < usable { pool_page(true, a, true, false) } else { kernel_page(a) };
    }
    *slot_entry(va) = table as u64 | TABLE_OR_PAGE;
}

/// The level-2 entry of window slot `va`. An address outside the windows is a bug in the caller: the index panics.
fn slot_entry(va: usize) -> &'static mut u64 {
    let g = (va - ARC_VA.0) / GIB;
    unsafe { &mut (*(&raw mut L2_ARCS))[g].0[(va / MIB2) % 512] }
}

/// The level-3 table mapped at window slot `va`, None while the slot is unmapped.
pub fn slot_table(va: usize) -> Option<usize> {
    let e = *slot_entry(va);
    (e & 3 == 3).then_some((e & OA) as usize)
}

/// Point window slot `va` at another level-3 table (a frame's, frames.rs `table`), or unmap it (0). Call `unmapped` for the slot before its old frame is used again.
pub fn set_slot(va: usize, table: usize) {
    *slot_entry(va) = if table == 0 { 0 } else { table as u64 | TABLE_OR_PAGE };
}

/// After mapping slots that were unmapped: the tables written before any core walks them. A slot going from invalid to valid needs no invalidation (no TLB holds an invalid entry).
pub fn mapped() {
    unsafe { asm!("dsb ish", "isb") };
}

/// After unmapping or remapping the window `[va, va + len)`: every core's TLB forgets it before its old frames are zeroed or handed on. Page by page up to 8 MiB; past that the whole TLB at once (one instruction, where a 200 MB arc would take 51,200).
pub fn unmapped(va: usize, len: usize) {
    unsafe {
        asm!("dsb ish");
        if len > 8 << 20 {
            asm!("tlbi vmalle1is");
        } else {
            let mut a = va;
            while a < va + len {
                asm!("tlbi vae1is, {x}", x = in(reg) (a as u64) >> 12);
                a += PAGE;
            }
        }
        asm!("dsb ish", "isb");
    }
}

/// The level-3 entry of the page at `a` in an arc's window; None outside the windows or in an unmapped slot.
fn page_entry(a: usize) -> Option<&'static mut u64> {
    if a < ARC_VA.0 || a >= ARC_VA.1 {
        return None;
    }
    let t = slot_table(a)?;
    Some(unsafe { &mut (*(t as *mut [u64; 512]))[(a / PAGE) % 512] })
}
