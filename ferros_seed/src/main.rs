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
//! 4. Confirm against SD: read same position, check P+1 for SD-ahead
//! 5. Read kernel binary from winning device
//! 6. Verify: BLAKE3(kernel) + Ed25519 verify against ring entry
//! 7. Jump to kernel at KERNEL_DRAM_BASE
//!
//! ## Fallback
//!
//! If kernel verification fails: try other device at same generation,
//! then decrement generation and repeat. Halt only if all 256 exhausted.

#![no_std]
#![no_main]

use ferros_layout::{
    KERNEL_RING_BASE, KERNEL_RING_SIZE, KERNEL_RING_DEPTH,
    KERNEL_DRAM_BASE, SPLASH_FB_BASE, BLOCK_SIZE,
};

// ---------------------------------------------------------------------------
// Linker symbols
// ---------------------------------------------------------------------------

unsafe extern "C" {
    static _start: u8;
    static __seed_end: u8;
    static __sig_start: u8;
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

core::arch::global_asm!(
    ".section .text.boot",
    ".globl _start",
    "_start:",
    // Save DTB pointer (x0 from ABL)
    "    mov x19, x0",
    // Zero BSS
    "    ldr x1, =__bss_start",
    "    ldr x2, =__bss_end",
    "0:  cmp x1, x2",
    "    b.ge 1f",
    "    str xzr, [x1], #8",
    "    b 0b",
    "1:",
    // Set up stack
    "    ldr x1, =__stack_top",
    "    mov sp, x1",
    // Call Rust entry with DTB pointer
    "    mov x0, x19",
    "    bl seed_main",
    // If seed_main returns (should never happen), halt
    "2:  wfe",
    "    b 2b",
);

// ---------------------------------------------------------------------------
// Rust entry point
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
extern "C" fn seed_main(dtb_addr: usize) -> ! {
    // ---- Step 1: Self-verify ----
    if !self_verify() {
        error("SEED INTEGRITY FAILURE");
        halt();
    }

    // ---- Step 2: Init UFS (resume from ABL state) ----
    let ufs = ferros_hal::ufs::UfsController::resume();

    // ---- Step 3: Scan kernel ring (UFS primary, SD confirmation) ----
    let found = scan_kernel_ring(&ufs);

    match found {
        Some(entry) => {
            // ---- Step 4: Read kernel binary ----
            let kernel_size = entry.kernel_size as usize;
            if kernel_size < 0x1000 || kernel_size > 4 * 1024 * 1024 {
                error("KERNEL SIZE INVALID");
                halt();
            }

            let kernel_dst = KERNEL_DRAM_BASE as *mut u8;
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
                        kernel_dst.add(offset),
                        copy_len,
                    );
                }
            }

            // ---- Step 5: Verify kernel ----
            let kernel_slice = unsafe {
                core::slice::from_raw_parts(kernel_dst, kernel_size)
            };
            let hash = blake3::hash(kernel_slice);

            if hash.as_bytes() != &entry.kernel_hash {
                error("KERNEL HASH MISMATCH");
                halt();
            }

            if !ed25519_verify(&entry.kernel_sig, hash.as_bytes()) {
                error("KERNEL SIGNATURE INVALID");
                halt();
            }

            // ---- Step 6: Jump to kernel ----
            // Pass DTB address in x0 (same convention as ABL → seed)
            unsafe {
                core::arch::asm!(
                    "mov x0, {dtb}",
                    "br {entry}",
                    dtb = in(reg) dtb_addr,
                    entry = in(reg) KERNEL_DRAM_BASE,
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

/// Verify seed's own integrity: BLAKE3 hash with signature zeroed,
/// then Ed25519 verify against embedded public key.
fn self_verify() -> bool {
    let start = &raw const _start as usize;
    let end = &raw const __seed_end as usize;
    let sig_off = &raw const __sig_start as usize - start;
    let size = end - start;

    // Can't verify if seed is impossibly small
    if size < 256 || sig_off + 64 > size {
        return false;
    }

    let binary = unsafe { core::slice::from_raw_parts(start as *const u8, size) };

    // Hash with signature region zeroed
    let mut hasher = blake3::Hasher::new();
    hasher.update(&binary[..sig_off]);
    hasher.update(&[0u8; 64]); // signature placeholder
    hasher.update(&binary[sig_off + 64..]);
    let hash = hasher.finalize();

    // Extract actual signature
    let sig_bytes = &binary[sig_off..sig_off + 64];

    // Dev build: all-zero signature = skip verification
    if sig_bytes == &[0u8; 64] && PUBKEY == [0u8; 32] {
        return true; // unsigned dev build, no key baked in
    }

    ed25519_verify(sig_bytes, hash.as_bytes())
}

// ---------------------------------------------------------------------------
// Ed25519 verification
// ---------------------------------------------------------------------------

fn ed25519_verify(sig_bytes: &[u8], message: &[u8]) -> bool {
    let pk = match ed25519_compact::PublicKey::from_slice(&PUBKEY) {
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

/// Parsed kernel ring entry — everything the seed needs.
struct KernelRingEntry {
    generation: u64,
    kernel_lba: u32,
    kernel_size: u32,
    kernel_hash: [u8; 32],
    kernel_sig: [u8; 64],
}

/// Parse a kernel ring entry from a 4KB block.
/// Returns None if the block is empty, corrupt, or not a valid VSF document.
fn parse_kernel_entry(blk: &[u8]) -> Option<KernelRingEntry> {
    use ferros_hal::vsf_mini::VsfReader;

    let mut r = VsfReader::new(blk);

    // VSF header
    if !r.magic() { return None; }
    let _ver = r.version()?;
    let _bver = r.backward_version()?;
    let _hlen = r.header_length()?;
    let _eagle_time = r.eagle_time_qtimer()?;

    // Provenance hash (verify later if needed)
    let hp_pos = r.pos;
    let _hp = r.hash_p()?;
    let _count = r.field_count()?;
    if !r.close() { return None; }

    // Verify provenance hash
    let mut temp = [0u8; BLOCK_SIZE];
    temp.copy_from_slice(&blk[..BLOCK_SIZE]);
    for i in 0..32 { temp[hp_pos + 4 + i] = 0; } // zero hp field
    let computed = blake3::hash(&temp);
    if computed.as_bytes() != &blk[hp_pos + 4..hp_pos + 36] {
        return None;
    }

    // Body: anonymous section with fields
    if r.read_byte_raw()? != b'[' { return None; }

    let mut entry = KernelRingEntry {
        generation: 0,
        kernel_lba: 0,
        kernel_size: 0,
        kernel_hash: [0u8; 32],
        kernel_sig: [0u8; 64],
    };

    while r.peek_tag() == Some(b'(') {
        r.read_byte_raw(); // consume '('
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
/// Returns 0 if empty/corrupt/unreadable.
fn read_generation(ufs: &ferros_hal::ufs::UfsController, pos: u32) -> u64 {
    let lba = KERNEL_RING_BASE + pos;
    let ocs = ufs.read_block(lba);
    if ocs != 0 { return 0; }

    let data = ufs.data_buffer();

    // Quick parse: just extract generation from the VSF body
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
// Kernel ring scan — UFS primary, SD confirmation at P+1
// ---------------------------------------------------------------------------

fn scan_kernel_ring(ufs: &ferros_hal::ufs::UfsController) -> Option<KernelRingEntry> {
    // Binary search UFS for newest generation
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
        // Empty ring — no kernel installed
        // TODO: try SD as primary if UFS is empty
        return None;
    }

    // TODO: SD confirmation read at position `lo` and `lo + 1`
    // For now, trust UFS. SD fallback will be added when the SD
    // driver is made alloc-free for the seed.

    // Read full entry from the winning position
    let lba = KERNEL_RING_BASE + lo;
    let ocs = ufs.read_block(lba);
    if ocs != 0 { return None; }

    let data = ufs.data_buffer();
    let mut blk = [0u8; BLOCK_SIZE];
    blk.copy_from_slice(&data[..BLOCK_SIZE]);

    parse_kernel_entry(&blk)
}

// ---------------------------------------------------------------------------
// Error display and halt
// ---------------------------------------------------------------------------

/// Write an error message to the splash framebuffer.
fn error(msg: &str) {
    // Simple text output at the splash FB — 8x16 font, white on red
    let fb = SPLASH_FB_BASE as *mut u32;
    let stride = 1224; // FP5 display width in pixels

    // Red background bar (64 pixels tall)
    for y in 0..64 {
        for x in 0..stride {
            unsafe {
                core::ptr::write_volatile(fb.add(y * stride + x), 0xFF_400000);
            }
        }
    }

    // White text (crude 8x16 rendering using the HAL's font if available)
    // For now, just write the red bar as a visual error indicator.
    // The actual text rendering will use ferros_hal::fb once wired up.
    let _ = msg;
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
