//! Ferros Seed — Trust Anchor
//!
//! The first code that runs after ABL hands off control.
//! Self-verifies, finds the current kernel via the kernel ring,
//! verifies the kernel, and jumps. Nothing else.
//!
//! No alloc, no interrupts, no USB, no capability system.
//! Sub-second runtime. Linear execution.
//!
//! ## Boot sequence
//!
//! 1. Zero BSS, set up stack
//! 2. Self-verify: BLAKE3(own binary with sig zeroed) + Ed25519 verify
//! 3. Scan kernel ring on UFS (8 reads via binary search)
//! 4. Read kernel binary from UFS to staging area
//! 5. Verify: BLAKE3(kernel) + Ed25519 verify against ring entry
//! 6. Cache flush + jump to kernel
//!
//! ## Fallback
//!
//! If kernel verification fails: try other device at same generation,
//! then decrement generation and repeat. Halt only if all 256 exhausted.

#![no_std]
#![no_main]

use ferros_layout::{
    KERNEL_RING_BASE, KERNEL_RING_SIZE, KERNEL_RING_DEPTH,
    SPLASH_FB_BASE, BLOCK_SIZE,
};

// ---------------------------------------------------------------------------
// Linker symbols
// ---------------------------------------------------------------------------

unsafe extern "C" {
    static _start: u8;
    static __seed_end: u8;
    static __bss_start: u8;
    static __bss_end: u8;
    static __stack_top: u8;
}

// ---------------------------------------------------------------------------
// Ed25519 public key (32 bytes, baked at build time)
// ---------------------------------------------------------------------------

#[unsafe(link_section = ".rodata.pubkey")]
#[used]
static PUBKEY: [u8; 32] = [0u8; 32]; // placeholder — patched by mkimg sign

// ---------------------------------------------------------------------------
// Ed25519 signature of seed binary (64 bytes, patched by mkimg sign)
// ---------------------------------------------------------------------------

#[unsafe(link_section = ".rodata.sig")]
#[used]
static SEED_SIG: [u8; 64] = [0u8; 64]; // zeroed = unsigned dev build

// ---------------------------------------------------------------------------
// Boot entry (asm)
// ---------------------------------------------------------------------------

// ========================================================================
// PE/COFF header + ARM64 Image header + boot stub
//
// ABL (UEFI-based) requires PE/COFF format. Same structure as the kernel.
// Layout:
//   0x000: DOS/ARM64 header (MZ + branch + ARM64 Image fields + e_lfanew)
//   0x040: PE header (PE\0\0 + COFF + Optional + Section table)
//   0x1000: _entry (page-aligned .text section start)
// ========================================================================
core::arch::global_asm!(
    ".section .text.boot",
    ".globl _start",

    "_start:",
    // Offset 0x00: MZ magic — valid ARM64: add x13, x18, #0x16
    "    .long   0x91005A4D",
    // Offset 0x04: branch to _entry (past all headers)
    "    b       _entry",

    // Offset 0x08: text_offset (ARM64 Image header)
    "    .quad   0x80000",
    // Offset 0x10: image_size
    "    .quad   __seed_size",
    // Offset 0x18: flags — LE, 4K pages
    "    .quad   0x0A",
    // Offset 0x20-0x37: reserved
    "    .quad   0",
    "    .quad   0",
    "    .quad   0",
    // Offset 0x38: ARM64 magic
    "    .ascii  \"ARM\\x64\"",
    // Offset 0x3C: PE header offset
    "    .long   .Lpe_header - _start",

    // ---------- PE/COFF Header (at offset 0x40) ----------
    ".balign 4",
    ".Lpe_header:",
    "    .ascii  \"PE\\0\\0\"",

    // COFF header (20 bytes)
    "    .short  0xAA64",               // Machine: ARM64
    "    .short  1",                     // NumberOfSections
    "    .long   0",                     // TimeDateStamp
    "    .long   0",                     // PointerToSymbolTable
    "    .long   0",                     // NumberOfSymbols
    "    .short  .Lsection_table - .Loptional_header",  // SizeOfOptionalHeader
    "    .short  0x206",                 // Characteristics

    ".Loptional_header:",
    "    .short  0x20B",                 // Magic: PE32+
    "    .byte   0",                     // MajorLinkerVersion
    "    .byte   0",                     // MinorLinkerVersion
    "    .long   __seed_size - 0x1000",  // SizeOfCode
    "    .long   0",                     // SizeOfInitializedData
    "    .long   0",                     // SizeOfUninitializedData
    "    .long   0x1000",               // AddressOfEntryPoint
    "    .long   0x1000",               // BaseOfCode

    // PE32+ fields
    "    .quad   0",                     // ImageBase
    "    .long   0x1000",               // SectionAlignment
    "    .long   0x200",                 // FileAlignment
    "    .short  0",                     // MajorOperatingSystemVersion
    "    .short  0",                     // MinorOperatingSystemVersion
    "    .short  0",                     // MajorImageVersion
    "    .short  0",                     // MinorImageVersion
    "    .short  0",                     // MajorSubsystemVersion
    "    .short  0",                     // MinorSubsystemVersion
    "    .long   0",                     // Win32VersionValue
    "    .long   __seed_size",           // SizeOfImage
    "    .long   0x1000",               // SizeOfHeaders
    "    .long   0",                     // CheckSum
    "    .short  10",                    // Subsystem: EFI Application
    "    .short  0",                     // DllCharacteristics
    "    .quad   0",                     // SizeOfStackReserve
    "    .quad   0",                     // SizeOfStackCommit
    "    .quad   0",                     // SizeOfHeapReserve
    "    .quad   0",                     // SizeOfHeapCommit
    "    .long   0",                     // LoaderFlags
    "    .long   6",                     // NumberOfRvaAndSizes

    // Data directories (6 entries, all empty)
    "    .quad   0",                     // Export Table
    "    .quad   0",                     // Import Table
    "    .quad   0",                     // Resource Table
    "    .quad   0",                     // Exception Table
    "    .quad   0",                     // Certificate Table
    "    .quad   0",                     // Base Relocation Table

    ".Lsection_table:",
    "    .ascii  \".text\\0\\0\\0\"",    // Name
    "    .long   __seed_size - 0x1000",  // VirtualSize
    "    .long   0x1000",               // VirtualAddress
    "    .long   __bss_start - _start - 0x1000",  // SizeOfRawData
    "    .long   0x1000",               // PointerToRawData
    "    .long   0",                     // PointerToRelocations
    "    .long   0",                     // PointerToLinenumbers
    "    .short  0",                     // NumberOfRelocations
    "    .short  0",                     // NumberOfLinenumbers
    "    .long   0xE0000020",            // Characteristics

    // ---------- Actual seed entry (page-aligned at RVA 0x1000) ----------
    ".balign 0x1000",
    "_entry:",
    // Save DTB pointer, mask interrupts
    "    mov x19, x0",
    "    msr daifset, #0xF",
    // Leave MMU + caches ON — ABL sets up identity-mapped page tables
    // that cover DRAM + MMIO. The kernel expects this state on entry.
    // Set up exception vector so crashes are visible
    "    adr x2, .Lseed_vectors",
    "    msr vbar_el1, x2",
    "    isb",
    // Zero BSS
    "    adrp x1, __bss_start",
    "    add  x1, x1, :lo12:__bss_start",
    "    adrp x2, __bss_end",
    "    add  x2, x2, :lo12:__bss_end",
    "0:  cmp x1, x2",
    "    b.ge 1f",
    "    str xzr, [x1], #8",
    "    b 0b",
    "1:",
    // Set up stack
    "    adrp x1, __stack_top",
    "    add  x1, x1, :lo12:__stack_top",
    "    mov sp, x1",
    // Call Rust entry with DTB pointer
    "    mov x0, x19",
    "    bl seed_main",
    // Halt
    "2:  wfe",
    "    b 2b",

    // Exception vector table — crash handler fills FB with magenta (G#FF00FF)
    ".balign 0x800",
    ".Lseed_vectors:",
    "    b .Lseed_crash",   // EL1t sync
    ".balign 0x80",
    "    b .Lseed_crash",   // EL1t IRQ
    ".balign 0x80",
    "    b .Lseed_crash",   // EL1t FIQ
    ".balign 0x80",
    "    b .Lseed_crash",   // EL1t SError
    ".balign 0x80",
    "    b .Lseed_crash",   // EL1h sync
    ".balign 0x80",
    "    b .Lseed_crash",   // EL1h IRQ
    ".balign 0x80",
    "    b .Lseed_crash",   // EL1h FIQ
    ".balign 0x80",
    "    b .Lseed_crash",   // EL1h SError
    ".balign 0x80",
    "    b .Lseed_crash",   // Lower sync
    ".balign 0x80",
    "    b .Lseed_crash",   // Lower IRQ
    ".balign 0x80",
    "    b .Lseed_crash",   // Lower FIQ
    ".balign 0x80",
    "    b .Lseed_crash",   // Lower SError
    ".balign 0x80",
    "    b .Lseed_crash",
    ".balign 0x80",
    "    b .Lseed_crash",
    ".balign 0x80",
    "    b .Lseed_crash",
    ".balign 0x80",
    "    b .Lseed_crash",

    // Crash handler: fill top 64 rows with magenta (G#FF00FF)
    ".Lseed_crash:",
    "    movz x8, #0x0000",
    "    movk x8, #0xE100, lsl #16",  // splash FB base
    "    movz w9, #0xFF00",
    "    movk w9, #0xFFFF, lsl #16",  // FFFF_FF00 = magenta-ish
    "    movz x10, #0x31E0",
    "    movk x10, #0x1, lsl #16",    // 0x131E0 = 78304 ≈ 1224*64
    "3:  str  w9, [x8], #4",
    "    subs x10, x10, #1",
    "    b.ne 3b",
    "4:  wfe",
    "    b 4b",
);

// ---------------------------------------------------------------------------
// Rust entry point
// ---------------------------------------------------------------------------

/// Staging address for kernel binary. 512MB into DRAM, well away from the
/// seed at DRAM_BASE + G#80000 = G#80080000. The kernel uses PC-relative
/// addressing (adrp) so it runs correctly from any DRAM address.
const KERNEL_STAGE: usize = 0x8200_0000;

/// Seed timing record — written to DRAM for the kernel to read.
/// Located just below KERNEL_STAGE so the kernel read doesn't clobber it.
const SEED_TIMING_ADDR: usize = 0x81FF_FFE0;

fn qtimer() -> u64 {
    let val: u64;
    unsafe { core::arch::asm!("mrs {}, CNTPCT_EL0", out(reg) val) };
    val
}

fn write_seed_timing(start: u64, end: u64) {
    unsafe {
        let p = SEED_TIMING_ADDR as *mut u64;
        core::ptr::write_volatile(p, start);
        core::ptr::write_volatile(p.add(1), end);
        // Magic: "SEEDTIME"
        core::ptr::write_volatile(p.add(2), 0x454D_4954_4445_4553);
    }
}

#[unsafe(no_mangle)]
extern "C" fn seed_main(dtb_addr: usize) -> ! {
    let t_start = qtimer();

    // Self-verify FIRST — before any memory modifications.
    // progress() modifies PROGRESS_X (.data), which is in the hashed range.
    // Stack frames are excluded by hashing only _start..__bss_start.
    if !self_verify() {
        error("SEED INTEGRITY FAILURE");
        halt();
    }
    progress(0xFF_00FF00); // green = self-verify passed
    progress(0xFF_FFFF00); // yellow = proceeding

    // ---- Step 2: Init UFS (resume from ABL state) ----
    let ufs = ferros_hal::ufs::UfsController::resume();
    progress(0xFF_00FFFF); // cyan = UFS resumed

    // ---- Step 3: Scan kernel ring ----
    let found = scan_kernel_ring(&ufs);

    match found {
        Some(entry) => {
            progress(0xFF_FF00FF); // magenta = ring entry found

            // ---- Step 4: Read kernel to staging area ----
            let kernel_size = entry.kernel_size as usize;
            if kernel_size < 0x1000 || kernel_size > 4 * 1024 * 1024 {
                error("KERNEL SIZE INVALID");
                halt();
            }

            let ufs = ferros_hal::ufs::UfsController::resume();
            let stage_dst = KERNEL_STAGE as *mut u8;
            let blocks = (kernel_size + BLOCK_SIZE - 1) / BLOCK_SIZE;

            for i in 0..blocks {
                let lba = entry.kernel_lba + i as u32;
                let ocs = ufs.read_block(lba);
                if ocs != 0 {
                    error("KERNEL READ FAILED");
                    halt();
                }
                let src = ufs.data_buffer();
                let offset = i * BLOCK_SIZE;
                let copy_len = core::cmp::min(BLOCK_SIZE, kernel_size - offset);
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        src.as_ptr(),
                        stage_dst.add(offset),
                        copy_len,
                    );
                }
            }

            progress(0xFF_FF8000); // orange = kernel read complete

            // ---- Step 5: Verify kernel ----
            let kernel_slice = unsafe {
                core::slice::from_raw_parts(stage_dst, kernel_size)
            };
            let hash = blake3::hash(kernel_slice);

            if hash.as_bytes() != &entry.kernel_hash {
                error("KERNEL HASH MISMATCH");
                halt();
            }

            let pubkey = read_pubkey();
            if pubkey != [0u8; 32] {
                if !ed25519_verify_key(&entry.kernel_sig, hash.as_bytes(), &pubkey) {
                    error("KERNEL SIGNATURE INVALID");
                    halt();
                }
            }

            progress(0xFF_0000FF); // blue = verification passed

            // ---- Step 6: Cache flush + jump ----
            // ABL's MMU + caches are on. Flush staged kernel from D-cache
            // to DRAM, then invalidate I-cache so CPU fetches fresh code.
            unsafe {
                let mut addr = KERNEL_STAGE;
                let end = KERNEL_STAGE + kernel_size;
                while addr < end {
                    core::arch::asm!("dc cvau, {}", in(reg) addr);
                    addr += 64;
                }
                core::arch::asm!("dsb ish");
                addr = KERNEL_STAGE;
                while addr < end {
                    core::arch::asm!("ic ivau, {}", in(reg) addr);
                    addr += 64;
                }
                core::arch::asm!("dsb ish");
                core::arch::asm!("isb");
            }

            progress(0xFF_FFFFFF); // white = jumping to kernel

            let t_end = qtimer();
            write_seed_timing(t_start, t_end);

            unsafe {
                core::arch::asm!(
                    "br {entry}",
                    entry = in(reg) KERNEL_STAGE,
                    in("x0") dtb_addr,
                    options(noreturn),
                );
            }
        }
        None => {
            error("NO VALID KERNEL FOUND");
            halt();
        }
    }
}

// ---------------------------------------------------------------------------
// Self-verification
// ---------------------------------------------------------------------------

/// Read PUBKEY from memory via volatile — prevents the compiler from
/// constant-folding the [0u8; 32] initializer (mkimg patches the bytes).
fn read_pubkey() -> [u8; 32] {
    unsafe { core::ptr::read_volatile(&raw const PUBKEY as *const [u8; 32]) }
}

/// Verify seed's own integrity: BLAKE3 hash with signature zeroed,
/// then Ed25519 verify against embedded public key.
///
/// Hashes _start..__bss_start (file-backed data only). This excludes
/// BSS and stack, which are modified at runtime before this runs.
/// MUST be called before any .data modifications (e.g. progress()).
fn self_verify() -> bool {
    let start = &raw const _start as usize;
    let end = &raw const __bss_start as usize;
    let sig_off = SEED_SIG.as_ptr() as usize - start;
    let size = end - start;

    if size < 256 || sig_off + 64 > size {
        return false;
    }

    let binary = unsafe { core::slice::from_raw_parts(start as *const u8, size) };

    // Hash with signature region zeroed
    let mut hasher = blake3::Hasher::new();
    hasher.update(&binary[..sig_off]);
    hasher.update(&[0u8; 64]);
    hasher.update(&binary[sig_off + 64..]);
    let hash = hasher.finalize();

    // Read sig and pubkey from memory (not the static — compiler may fold it)
    let sig_bytes = &binary[sig_off..sig_off + 64];
    let pubkey = read_pubkey();

    // Dev build: all-zero signature + pubkey = skip verification
    if sig_bytes == &[0u8; 64] && pubkey == [0u8; 32] {
        return true;
    }

    ed25519_verify_key(sig_bytes, hash.as_bytes(), &pubkey)
}

// ---------------------------------------------------------------------------
// Ed25519 verification
// ---------------------------------------------------------------------------

fn ed25519_verify_key(sig_bytes: &[u8], message: &[u8], pubkey: &[u8; 32]) -> bool {
    let pk = match ed25519_compact::PublicKey::from_slice(pubkey) {
        Ok(pk) => pk,
        Err(_) => return false,
    };
    let sig = match ed25519_compact::Signature::from_slice(sig_bytes) {
        Ok(sig) => sig,
        Err(_) => return false,
    };
    pk.verify(message, &sig).is_ok()
}

// ---------------------------------------------------------------------------
// Kernel ring entry
// ---------------------------------------------------------------------------

struct KernelRingEntry {
    generation: u64,
    kernel_lba: u32,
    kernel_size: u32,
    kernel_hash: [u8; 32],
    kernel_sig: [u8; 64],
}

/// Parse a kernel ring entry from a 4KB block.
fn parse_kernel_entry(blk: &[u8]) -> Option<KernelRingEntry> {
    use ferros_hal::vsf_mini::VsfReader;

    let mut r = VsfReader::new(blk);

    if !r.magic() { return None; }
    let _ver = r.version()?;
    let _bver = r.backward_version()?;
    let _hlen = r.header_length()?;
    let _eagle_time = r.eagle_time_qtimer()?;

    let hp_pos = r.pos;
    let _hp = r.hash_p()?;
    let _count = r.field_count()?;
    if !r.close() { return None; }

    // Verify provenance hash
    let mut temp = [0u8; BLOCK_SIZE];
    temp.copy_from_slice(&blk[..BLOCK_SIZE]);
    for i in 0..32 { temp[hp_pos + 4 + i] = 0; }
    let computed = blake3::hash(&temp);
    if computed.as_bytes() != &blk[hp_pos + 4..hp_pos + 36] {
        return None;
    }

    if r.read_byte_raw()? != b'[' { return None; }

    let mut entry = KernelRingEntry {
        generation: 0,
        kernel_lba: 0,
        kernel_size: 0,
        kernel_hash: [0u8; 32],
        kernel_sig: [0u8; 64],
    };

    while r.peek_tag() == Some(b'(') {
        r.read_byte_raw();
        let fname = r.dict_key_str()?;
        if r.read_byte_raw()? != b':' { return None; }

        match fname {
            "generation" => { entry.generation = r.uint()?; }
            "kernel_lba" => { entry.kernel_lba = r.uint()? as u32; }
            "kernel_size" => { entry.kernel_size = r.uint()? as u32; }
            "kernel_hash" => {
                let h = r.hash_p()?;
                entry.kernel_hash.copy_from_slice(h);
            }
            "kernel_sig" => {
                let sig = r.signature()?;
                entry.kernel_sig.copy_from_slice(sig);
            }
            _ => { r.skip_field(); }
        }

        if r.read_byte_raw()? != b')' { return None; }
    }

    if entry.generation == 0 { return None; }
    Some(entry)
}

/// Read the generation number from a kernel ring position.
fn read_generation(ufs: &ferros_hal::ufs::UfsController, pos: u32) -> u64 {
    let lba = KERNEL_RING_BASE + pos;
    let ocs = ufs.read_block(lba);
    if ocs != 0 { return 0; }

    let data = ufs.data_buffer();

    use ferros_hal::vsf_mini::VsfReader;
    let mut r = VsfReader::new(data);
    if !r.magic() { return 0; }
    if r.version().is_none() { return 0; }
    if r.backward_version().is_none() { return 0; }
    if r.header_length().is_none() { return 0; }
    if r.eagle_time_qtimer().is_none() { return 0; }
    if r.hash_p().is_none() { return 0; }
    if r.field_count().is_none() { return 0; }
    if !r.close() { return 0; }

    if r.read_byte_raw() != Some(b'[') { return 0; }
    if r.read_byte_raw() != Some(b'(') { return 0; }
    if r.dict_key_str().is_none() { return 0; }
    if r.read_byte_raw() != Some(b':') { return 0; }
    r.uint().unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Kernel ring scan
// ---------------------------------------------------------------------------

fn scan_kernel_ring(ufs: &ferros_hal::ufs::UfsController) -> Option<KernelRingEntry> {
    let mut lo: u32 = 0;
    let mut size: u32 = KERNEL_RING_SIZE;

    for _ in 0..KERNEL_RING_DEPTH {
        let half = size >> 1;
        let mid = (lo + half) % KERNEL_RING_SIZE;
        let gen_lo = read_generation(ufs, lo);
        let gen_mid = read_generation(ufs, mid);

        if gen_mid > gen_lo {
            lo = mid;
        }
        size = half;
    }

    let ufs_gen = read_generation(ufs, lo);
    if ufs_gen == 0 {
        return None;
    }

    let lba = KERNEL_RING_BASE + lo;
    let ocs = ufs.read_block(lba);
    if ocs != 0 { return None; }

    let data = ufs.data_buffer();
    let mut blk = [0u8; BLOCK_SIZE];
    blk.copy_from_slice(&data[..BLOCK_SIZE]);

    parse_kernel_entry(&blk)
}

// ---------------------------------------------------------------------------
// Progress display
// ---------------------------------------------------------------------------

/// Write a colored dot (16x16 px) at the next progress slot on the splash FB.
/// Row 100, spaced 20px apart — visible below ABL's yellow bar.
/// Flushes D-cache so the DPU sees the write immediately.
static mut PROGRESS_X: usize = 16;
fn progress(color: u32) {
    let fb = SPLASH_FB_BASE as *mut u32;
    let stride = 1224;
    let base_row = 100;
    let x0 = unsafe { PROGRESS_X };

    for y in 0..16 {
        for x in 0..16 {
            unsafe {
                let addr = fb.add((base_row + y) * stride + x0 + x);
                core::ptr::write_volatile(addr, color);
            }
        }
    }

    // Flush the written cache lines so DPU sees them
    unsafe {
        for y in 0..16 {
            let line_addr = fb as usize + ((base_row + y) * stride + x0) * 4;
            core::arch::asm!("dc cvau, {}", in(reg) line_addr);
        }
        core::arch::asm!("dsb ish");
    }

    unsafe { PROGRESS_X += 20; }
}

// ---------------------------------------------------------------------------
// Error display and halt
// ---------------------------------------------------------------------------

/// Color-coded error bar on splash FB (rows 0-63, full width).
fn error(msg: &str) {
    let b0 = msg.as_bytes().get(0).copied().unwrap_or(0);
    let b7 = msg.as_bytes().get(7).copied().unwrap_or(0);
    let b9 = msg.as_bytes().get(9).copied().unwrap_or(0);
    let color: u32 = match b0 {
        b'S' => 0xFF_400000, // SEED INTEGRITY = red
        b'N' => 0xFF_FF8000, // NO VALID KERNEL = orange
        b'K' => match (b7, b9) {
            (b'S', b'Z') => 0xFF_FFFF00, // KERNEL SIZE = yellow
            (b'S', b'G') => 0xFF_00FFFF, // KERNEL SIGNATURE = cyan
            (b'R', _)    => 0xFF_800080, // KERNEL READ = purple
            (b'H', _)    => 0xFF_0000FF, // KERNEL HASH = blue
            _            => 0xFF_FFFFFF,
        }
        _ => 0xFF_FFFFFF,
    };

    let fb = SPLASH_FB_BASE as *mut u32;
    let stride = 1224;
    for y in 0..64 {
        for x in 0..stride {
            unsafe {
                core::ptr::write_volatile(fb.add(y * stride + x), color);
            }
        }
    }

    // Flush error bar to display
    unsafe {
        for y in 0..64 {
            let mut addr = fb as usize + y * stride * 4;
            let end = addr + stride * 4;
            while addr < end {
                core::arch::asm!("dc cvau, {}", in(reg) addr);
                addr += 64;
            }
        }
        core::arch::asm!("dsb ish");
    }
}

fn halt() -> ! {
    loop {
        unsafe { core::arch::asm!("wfe"); }
    }
}

// ---------------------------------------------------------------------------
// Panic handler
// ---------------------------------------------------------------------------

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    error("SEED PANIC");
    halt();
}
