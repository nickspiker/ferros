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
//! ## Android Boot Image Format
//!
//! Supports v4 (Android 13 GKI — required for FP5/QCM6490):
//!
//! ```text
//! boot_img_hdr_v4:
//! 0x000   8       Magic: "ANDROID!"
//! 0x008   4       kernel_size
//! 0x00C   4       ramdisk_size (0)
//! 0x010   4       os_version (0)
//! 0x014   4       header_size (1584)
//! 0x018   16      reserved (zeros)
//! 0x028   4       header_version (4)
//! 0x02C   1536    cmdline
//! 0x62C   4       signature_size (0)
//! ```
//!
//! Page size is always 4096 for v3/v4. No kernel_addr field —
//! ABL uses the ARM64 Image header's text_offset instead.
//!
//! Also supports legacy v0 via `boot-v0` command.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::process;

use flate2::Compression;
use flate2::write::GzEncoder;

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
            "  LOAD: vaddr=G#{:08x} filesz={} memsz={} -> offset G#{:x}",
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

/// Build an Android boot image (v2 format) wrapping a gzip-compressed kernel.
///
/// This matches the format confirmed working on FP5 (QCM6490) per U-Boot docs:
///   mkbootimg --pagesize 4096 --header_version 2 --kernel_offset 0x00008000
///
/// ABL detects gzip magic (0x1f8b) and decompresses before loading.
/// After decompression, it reads the ARM64 Image header's text_offset.
///
/// boot_img_hdr_v0/v1/v2 layout:
///   0x000  8     magic "ANDROID!"
///   0x008  4     kernel_size
///   0x00C  4     kernel_addr (base + kernel_offset)
///   0x010  4     ramdisk_size (0)
///   0x014  4     ramdisk_addr
///   0x018  4     second_size (0)
///   0x01C  4     second_addr
///   0x020  4     tags_addr
///   0x024  4     page_size (4096)
///   0x028  4     header_version (2)
///   0x02C  4     os_version (0)
///   0x030  16    name
///   0x040  512   cmdline
///   0x240  32    id
///   0x260  1024  extra_cmdline
///   -- v1 fields --
///   0x660  4     recovery_dtbo_size (0)
///   0x664  8     recovery_dtbo_offset (0)
///   0x66C  4     header_size
///   -- v2 fields --
///   0x670  4     dtb_size (0)
///   0x674  8     dtb_addr (0)
fn make_boot_img(kernel: &[u8]) -> Vec<u8> {
    // FP5 uses boot image header v3 (GKI format).
    // Stock kernel is UNCOMPRESSED PE/COFF — ABL may not support gzip.
    // v3 removed kernel_addr, ramdisk_addr, tags_addr, second_*, page_size fields.
    // Page size is always 4096 (implicit). ABL uses ARM64 Image header text_offset.
    //
    // boot_img_hdr_v3 layout:
    //   0x000  8     magic "ANDROID!"
    //   0x008  4     kernel_size
    //   0x00C  4     ramdisk_size (0)
    //   0x010  4     os_version (0)
    //   0x014  4     header_size (1580)
    //   0x018  16    reserved (zeros)
    //   0x028  4     header_version (3)
    //   0x02C  1536  cmdline (zeros)
    //   Total header: 1580 bytes, padded to 4096
    const PAGE_SIZE: usize = 4096;
    const HEADER_VERSION: u32 = 3;
    const HEADER_SIZE: u32 = 1580;

    // NO gzip compression — FP5 ABL expects uncompressed ARM64 Image
    eprintln!("  Kernel: {} bytes (uncompressed)", kernel.len());

    // Build the header (padded to page_size)
    let mut header = vec![0u8; PAGE_SIZE];

    header[0..8].copy_from_slice(b"ANDROID!");
    write_le32(&mut header, 0x008, kernel.len() as u32);      // kernel_size
    write_le32(&mut header, 0x00C, 0);                        // ramdisk_size
    write_le32(&mut header, 0x010, 0);                        // os_version
    write_le32(&mut header, 0x014, HEADER_SIZE);               // header_size
    // 0x018..0x028: reserved (zeros)
    write_le32(&mut header, 0x028, HEADER_VERSION);            // header_version = 3
    // 0x02C..0x62C: cmdline (zeros)

    // Pad kernel to page boundary
    let kernel_pages = (kernel.len() + PAGE_SIZE - 1) / PAGE_SIZE;
    let mut kernel_padded = vec![0u8; kernel_pages * PAGE_SIZE];
    kernel_padded[..kernel.len()].copy_from_slice(kernel);

    let mut img = header;
    img.extend_from_slice(&kernel_padded);

    eprintln!("  Boot image v3: {} header + {} kernel = {} total",
        PAGE_SIZE, kernel_padded.len(), img.len());

    img
}

#[allow(dead_code)]
fn gzip_compress(data: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn write_le32(buf: &mut [u8], offset: usize, val: u32) {
    buf[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
}

/// Simple hash for the boot image ID field. Not cryptographic —
/// just for identification in fastboot output.
#[allow(dead_code)]
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
