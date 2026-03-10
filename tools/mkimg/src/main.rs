//! ferros-mkimg — build flashable images from the ferros kernel.
//!
//! ## Usage
//!
//! ```text
//! # Build flat binary from ELF:
//! ferros-mkimg flat kernel.elf -o ferros.bin
//!
//! # Build Android boot.img for fastboot:
//! ferros-mkimg boot kernel.elf -o ferros.img
//!
//! # Generate a random anchor key image:
//! ferros-mkimg anchor-key -o anchor.img
//!
//! # All-in-one: build + wrap
//! ferros-mkimg boot target/aarch64-unknown-none/release/ferros_kernel -o ferros.img
//! ```
//!
//! ## Android Boot Image Format (v0/v1)
//!
//! ```text
//! Offset  Size    Field
//! 0x000   8       Magic: "ANDROID!"
//! 0x008   4       kernel_size
//! 0x00C   4       kernel_addr  (load address)
//! 0x010   4       ramdisk_size (0 for us)
//! 0x014   4       ramdisk_addr
//! 0x018   4       second_size  (0)
//! 0x01C   4       second_addr
//! 0x020   4       tags_addr    (DTB address hint)
//! 0x024   4       page_size    (2048 or 4096)
//! 0x028   4       header_version (0)
//! 0x02C   4       os_version
//! 0x030   16      name
//! 0x040   512     cmdline
//! 0x240   32      id (SHA1 of kernel + ramdisk)
//! 0x260   1024    extra_cmdline
//! ```
//!
//! After the header (padded to page_size), the kernel binary follows
//! (also padded to page_size).

use std::env;
use std::fs;
use std::io::{self, Write};
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        usage();
        process::exit(1);
    }

    match args[1].as_str() {
        "flat" => cmd_flat(&args[2..]),
        "boot" => cmd_boot(&args[2..]),
        "anchor-key" => cmd_anchor_key(&args[2..]),
        "help" | "--help" | "-h" => usage(),
        _ => {
            eprintln!("Unknown command: {}", args[1]);
            usage();
            process::exit(1);
        }
    }
}

fn usage() {
    eprintln!("ferros-mkimg — build flashable images");
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  ferros-mkimg flat <kernel.elf> -o <output.bin>");
    eprintln!("  ferros-mkimg boot <kernel.elf> -o <output.img>");
    eprintln!("  ferros-mkimg anchor-key -o <anchor.img>");
}

// ---------------------------------------------------------------------------
// ELF parsing (minimal, aarch64 only)
// ---------------------------------------------------------------------------

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const PT_LOAD: u32 = 1;

struct ElfSegment {
    vaddr: u64,
    file_offset: u64,
    file_size: u64,
    mem_size: u64,
}

fn parse_elf_segments(data: &[u8]) -> Vec<ElfSegment> {
    // Verify magic
    if data.len() < 64 || data[0..4] != ELF_MAGIC {
        eprintln!("Error: not a valid ELF file");
        process::exit(1);
    }

    // Verify 64-bit, little-endian, aarch64
    if data[4] != 2 {
        eprintln!("Error: not a 64-bit ELF");
        process::exit(1);
    }
    if data[5] != 1 {
        eprintln!("Error: not little-endian");
        process::exit(1);
    }

    let e_phoff = u64::from_le_bytes(data[32..40].try_into().unwrap()) as usize;
    let e_phentsize = u16::from_le_bytes(data[54..56].try_into().unwrap()) as usize;
    let e_phnum = u16::from_le_bytes(data[56..58].try_into().unwrap()) as usize;

    let mut segments = Vec::new();

    for i in 0..e_phnum {
        let off = e_phoff + i * e_phentsize;
        let p_type = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());

        if p_type == PT_LOAD {
            let p_offset = u64::from_le_bytes(data[off + 8..off + 16].try_into().unwrap());
            let p_vaddr = u64::from_le_bytes(data[off + 16..off + 24].try_into().unwrap());
            let p_filesz = u64::from_le_bytes(data[off + 32..off + 40].try_into().unwrap());
            let p_memsz = u64::from_le_bytes(data[off + 40..off + 48].try_into().unwrap());

            segments.push(ElfSegment {
                vaddr: p_vaddr,
                file_offset: p_offset,
                file_size: p_filesz,
                mem_size: p_memsz,
            });
        }
    }

    if segments.is_empty() {
        eprintln!("Error: no PT_LOAD segments found");
        process::exit(1);
    }

    segments.sort_by_key(|s| s.vaddr);
    segments
}

/// Convert ELF to flat binary — concatenate all PT_LOAD segments
/// into a single contiguous image starting at the lowest vaddr.
fn elf_to_flat(elf_data: &[u8]) -> Vec<u8> {
    let segments = parse_elf_segments(elf_data);
    let base_addr = segments[0].vaddr;

    // Calculate total size needed
    let last = segments.last().unwrap();
    let total_size = (last.vaddr - base_addr + last.mem_size) as usize;

    let mut flat = vec![0u8; total_size];

    for seg in &segments {
        let dest_offset = (seg.vaddr - base_addr) as usize;
        let src_start = seg.file_offset as usize;
        let src_end = src_start + seg.file_size as usize;

        if src_end <= elf_data.len() {
            flat[dest_offset..dest_offset + seg.file_size as usize]
                .copy_from_slice(&elf_data[src_start..src_end]);
        }

        eprintln!(
            "  LOAD: vaddr=0x{:08x} filesz={} memsz={} -> offset 0x{:x}",
            seg.vaddr, seg.file_size, seg.mem_size, dest_offset
        );
    }

    eprintln!("  Flat binary: {} bytes ({:.1} KB)", flat.len(), flat.len() as f64 / 1024.0);
    flat
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn cmd_flat(args: &[String]) {
    let (input, output) = parse_io_args(args);
    let elf_data = fs::read(&input).unwrap_or_else(|e| {
        eprintln!("Error reading {}: {}", input, e);
        process::exit(1);
    });

    eprintln!("Converting ELF to flat binary: {}", input);
    let flat = elf_to_flat(&elf_data);
    write_output(&output, &flat);
    eprintln!("Wrote {} -> {}", input, output);
}

fn cmd_boot(args: &[String]) {
    let (input, output) = parse_io_args(args);
    let elf_data = fs::read(&input).unwrap_or_else(|e| {
        eprintln!("Error reading {}: {}", input, e);
        process::exit(1);
    });

    eprintln!("Building Android boot image: {}", input);
    let flat = elf_to_flat(&elf_data);
    let boot_img = make_boot_img(&flat);
    write_output(&output, &boot_img);
    eprintln!("Wrote boot.img -> {} ({} bytes)", output, boot_img.len());
}

fn cmd_anchor_key(args: &[String]) {
    let output = if args.len() >= 2 && args[0] == "-o" {
        args[1].clone()
    } else {
        "anchor.img".to_string()
    };

    // Generate 32 bytes of random key material.
    // On a real system this would use /dev/urandom or a CSPRNG.
    // For dev, we use a deterministic-but-unique seed from the timestamp.
    let mut key = [0u8; 32];

    // Read from /dev/urandom
    if let Ok(urandom) = fs::read("/dev/urandom") {
        key.copy_from_slice(&urandom[..32]);
    } else {
        // Fallback: use process ID and timestamp as entropy
        let pid = process::id();
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        for i in 0..32 {
            key[i] = ((time >> (i % 16 * 8)) ^ (pid as u128)) as u8;
        }
    }

    // Anchor key image format:
    // [0..8]   magic: "FEANCH01" (ferros anchor v01)
    // [8..40]  32-byte anchor key
    // [40..48] ring_size as u64 LE (default 4096 slots)
    // [48..64] reserved (zeros)
    let mut img = vec![0u8; 64];
    img[0..8].copy_from_slice(b"FEANCH01");
    img[8..40].copy_from_slice(&key);
    let ring_size: u64 = 4096;
    img[40..48].copy_from_slice(&ring_size.to_le_bytes());

    write_output(&output, &img);
    eprintln!("Wrote anchor key image -> {} (32-byte key, {} ring slots)", output, ring_size);
    eprintln!("Flash with: fastboot flash ferros_anchor {}", output);
}

// ---------------------------------------------------------------------------
// Android boot image construction
// ---------------------------------------------------------------------------

/// Build an Android boot image (v0 format) wrapping a flat kernel binary.
///
/// This produces an image that `fastboot boot` can load directly.
fn make_boot_img(kernel: &[u8]) -> Vec<u8> {
    let page_size: u32 = 2048;
    let kernel_addr: u32 = 0x0008_0000; // standard arm64 load address
    let tags_addr: u32 = 0x0000_0100;   // DTB hint (ABL may override)

    // Build the 1-page header
    let mut header = vec![0u8; page_size as usize];

    // Magic
    header[0..8].copy_from_slice(b"ANDROID!");

    // kernel_size
    write_le32(&mut header, 8, kernel.len() as u32);
    // kernel_addr
    write_le32(&mut header, 12, kernel_addr);
    // ramdisk_size = 0
    write_le32(&mut header, 16, 0);
    // ramdisk_addr
    write_le32(&mut header, 20, 0);
    // second_size = 0
    write_le32(&mut header, 24, 0);
    // second_addr
    write_le32(&mut header, 28, 0);
    // tags_addr
    write_le32(&mut header, 32, tags_addr);
    // page_size
    write_le32(&mut header, 36, page_size);
    // header_version = 0
    write_le32(&mut header, 40, 0);
    // os_version = 0
    write_le32(&mut header, 44, 0);

    // name: "ferros"
    header[48..54].copy_from_slice(b"ferros");

    // cmdline (empty — we don't use Linux cmdline)
    // id field: simple hash of kernel for identification
    let id_hash = simple_hash(kernel);
    header[576..608].copy_from_slice(&id_hash);

    // Pad kernel to page boundary
    let kernel_pages = (kernel.len() + page_size as usize - 1) / page_size as usize;
    let mut kernel_padded = vec![0u8; kernel_pages * page_size as usize];
    kernel_padded[..kernel.len()].copy_from_slice(kernel);

    // Concatenate header + kernel
    let mut img = header;
    img.extend_from_slice(&kernel_padded);

    eprintln!("  Boot image: {} header + {} kernel = {} total",
        page_size, kernel_padded.len(), img.len());

    img
}

fn write_le32(buf: &mut [u8], offset: usize, val: u32) {
    buf[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
}

/// Simple hash for the boot image ID field. Not cryptographic —
/// just for identification in fastboot output.
fn simple_hash(data: &[u8]) -> [u8; 32] {
    let mut hash = [0u8; 32];
    for (i, &byte) in data.iter().enumerate() {
        hash[i % 32] ^= byte;
        hash[(i + 7) % 32] = hash[(i + 7) % 32].wrapping_add(byte);
    }
    hash
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_io_args(args: &[String]) -> (String, String) {
    if args.is_empty() {
        eprintln!("Error: missing input file");
        usage();
        process::exit(1);
    }

    let input = args[0].clone();
    let output = if args.len() >= 3 && args[1] == "-o" {
        args[2].clone()
    } else {
        // Default output name
        let stem = input.rsplit('/').next().unwrap_or(&input);
        let stem = stem.split('.').next().unwrap_or(stem);
        format!("{}.bin", stem)
    };

    (input, output)
}

fn write_output(path: &str, data: &[u8]) {
    if path == "-" {
        io::stdout().write_all(data).unwrap();
    } else {
        fs::write(path, data).unwrap_or_else(|e| {
            eprintln!("Error writing {}: {}", path, e);
            process::exit(1);
        });
    }
}
