//! ferros-mkimg — build flashable images and manage the trust chain.
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
//! # Generate Ed25519 keypair:
//! ferros-mkimg keygen -o keys/
//!
//! # Sign a kernel binary:
//! ferros-mkimg sign-kernel kernel.elf --key keys/ferros.secret -o kernel.signed
//!
//! # Build signed seed boot.img:
//! ferros-mkimg seed seed.elf --key keys/ferros.secret -o seed.img
//!
//! # Generate a random anchor key image:
//! ferros-mkimg anchor-key -o anchor.img
//! ```
//!
//! ## Android Boot Image Format
//!
//! Supports v4 (Android 13 GKI — required for Pixel 8 / Tensor G3):
//!
//! ```text
//! boot_img_hdr_v4: 0x000   8       Magic: "ANDROID!" 0x008   4       kernel_size 0x00C   4       ramdisk_size (0) 0x010   4       os_version (0) 0x014   4       header_size (1584) 0x018   16      reserved (zeros) 0x028   4       header_version (4) 0x02C   1536    cmdline 0x62C   4       signature_size (0)
//! ```
//!
//! Page size is always 4096 for v3/v4. No kernel_addr field — ABL uses the ARM64 Image header's text_offset instead.
//!
//! Tensor G3 ABL requires the kernel payload LZ4-compressed (legacy frame, magic G#02214C18) — see BOOT.md "Tensor G3 ABL facts".
//! `--pad <MiB>` pads the flat binary and patches the ARM64 Image header image_size to match, for bisecting ABL's silent minimum-size rejection.

mod avb;

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
        "keygen" => cmd_keygen(&args[2..]),
        "sign-kernel" => cmd_sign_kernel(&args[2..]),
        "seed" => cmd_seed(&args[2..]),
        "anchor-key" => cmd_anchor_key(&args[2..]),
        "avb-keygen" => cmd_avb_keygen(&args[2..]),
        "vbmeta-info" => cmd_vbmeta_info(&args[2..]),
        "avb-pubkey" => cmd_avb_pubkey(&args[2..]),
        "avb-sign" => cmd_avb_sign(&args[2..]),
        "gate" => cmd_gate(&args[2..]),
        "unpack-kernel" => cmd_unpack_kernel(&args[2..]),
        "help" | "--help" | "-h" => usage(),
        _ => {
            eprintln!("Unknown command: {}", args[1]);
            usage();
            process::exit(1);
        }
    }
}

fn usage() {
    eprintln!("ferros-mkimg — build flashable images + trust chain");
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  ferros-mkimg flat <kernel.elf> -o <output.bin>");
    eprintln!("  ferros-mkimg boot <kernel.elf> [--pad <MiB>] -o <output.img>");
    eprintln!("  ferros-mkimg keygen -o <dir/>");
    eprintln!("  ferros-mkimg sign-kernel <kernel.elf> --key <secret> -o <kernel.signed>");
    eprintln!("  ferros-mkimg seed <seed.elf> --key <secret> [--pad <MiB>] -o <seed.img>");
    eprintln!("  ferros-mkimg anchor-key -o <anchor.img>");
    eprintln!("  ferros-mkimg avb-keygen -o <keys dir>                 (RSA-4096 for avb_custom_key, as <dir>/avb.vsf: kgr + kvr)");
    eprintln!("  ferros-mkimg avb-pubkey --key <avb.vsf> -o <avb_custom_key.bin>");
    eprintln!("  ferros-mkimg vbmeta-info <vbmeta.img>");
    eprintln!("  ferros-mkimg gate --hv <hv.bin> --image <Linux Image> [--ferros <ferros.bin>] [--cmdline <s>] -o <boot.img>");
    eprintln!("  ferros-mkimg unpack-kernel --boot <boot.img> -o <Image>");
    eprintln!("  ferros-mkimg avb-sign --key <avb.vsf> --google-vbmeta <vbmeta.img> --google-boot <boot.img> --gate <gate.img> --out-vbmeta <vbmeta.img> --out-boot <boot.img>");
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

/// Convert ELF to flat binary — concatenate all PT_LOAD segments into a single contiguous image starting at the lowest vaddr.
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
    let (args, pad_mib) = extract_pad(args);
    let (input, output) = parse_io_args(&args);
    let elf_data = fs::read(&input).unwrap_or_else(|e| {
        eprintln!("Error reading {}: {}", input, e);
        process::exit(1);
    });

    eprintln!("Building Android boot image: {}", input);
    let mut flat = elf_to_flat(&elf_data);
    if let Some(mib) = pad_mib {
        flat = pad_kernel(flat, mib);
    }
    let boot_img = make_boot_img(&flat);
    write_output(&output, &boot_img);
    eprintln!("Wrote boot.img -> {} ({} bytes)", output, boot_img.len());
}

/// Split off `--pad <MiB>` from an arg list so positional parsing stays simple.
fn extract_pad(args: &[String]) -> (Vec<String>, Option<usize>) {
    let mut out = Vec::with_capacity(args.len());
    let mut pad = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--pad" && i + 1 < args.len() {
            pad = args[i + 1].parse::<usize>().ok();
            if pad.is_none() {
                eprintln!("Error: --pad expects a size in MiB, got '{}'", args[i + 1]);
                process::exit(1);
            }
            i += 2;
        } else {
            out.push(args[i].clone());
            i += 1;
        }
    }
    (out, pad)
}

/// Pad a flat kernel binary with zeros to `pad_mib` MiB and patch the ARM64 Image header's image_size (offset G#10) to match.
/// ABL silently rejects small kernels (12KB failed, 37MB ran — see BOOT.md); patching image_size is what makes the pad experiment test the suspected header validation, not just the file size.
fn pad_kernel(mut flat: Vec<u8>, pad_mib: usize) -> Vec<u8> {
    let target = pad_mib * 1024 * 1024;
    if flat.len() >= target {
        eprintln!("  Pad: binary already {} bytes >= {} MiB, leaving as-is", flat.len(), pad_mib);
        return flat;
    }
    if flat.len() < 0x40 || &flat[0x38..0x3C] != b"ARM\x64" {
        eprintln!("Error: flat binary has no ARM64 Image header magic at G#38 — cannot pad/patch image_size");
        process::exit(1);
    }
    flat.resize(target, 0);
    let sz = flat.len() as u64;
    flat[0x10..0x18].copy_from_slice(&sz.to_le_bytes());
    eprintln!("  Padded to {} MiB, image_size patched to G#{:X}", pad_mib, sz);
    flat
}

fn cmd_anchor_key(args: &[String]) {
    let output = if args.len() >= 2 && args[0] == "-o" {
        args[1].clone()
    } else {
        "anchor.img".to_string()
    };

    // Generate 32 bytes of random key material. On a real system this would use /dev/urandom or a CSPRNG. For dev, we use a deterministic-but-unique seed from the timestamp.
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

    // Anchor key image format: [0..8]   magic: "FEANCH01" (ferros anchor v01) [8..40]  32-byte anchor key [40..48] ring_size as u64 LE (default 4096 slots) [48..64] reserved (zeros)
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
/// This matches the format confirmed working on FP5 (QCM6490) per U-Boot docs: mkbootimg --pagesize 4096 --header_version 2 --kernel_offset 0x00008000
///
/// ABL detects gzip magic (0x1f8b) and decompresses before loading. After decompression, it reads the ARM64 Image header's text_offset.
///
/// boot_img_hdr_v0/v1/v2 layout: 0x000  8     magic "ANDROID!" 0x008  4     kernel_size 0x00C  4     kernel_addr (base + kernel_offset) 0x010  4     ramdisk_size (0) 0x014  4     ramdisk_addr 0x018  4     second_size (0) 0x01C  4     second_addr 0x020  4     tags_addr 0x024  4     page_size (4096) 0x028  4     header_version (2) 0x02C  4     os_version (0) 0x030  16    name 0x040  512   cmdline 0x240  32    id 0x260  1024  extra_cmdline -- v1 fields -- 0x660  4     recovery_dtbo_size (0) 0x664  8     recovery_dtbo_offset (0) 0x66C  4     header_size -- v2 fields -- 0x670  4     dtb_size (0) 0x674  8     dtb_addr (0)
fn make_boot_img(kernel: &[u8]) -> Vec<u8> {
    // The cold ferros image: os_version as GrapheneOS wrote it, no command line.
    eprintln!("  Kernel: {} bytes (uncompressed)", kernel.len());
    let kernel = lz4_legacy_compress(kernel);
    eprintln!("  Kernel: {} bytes (LZ4 legacy)", kernel.len());
    boot_img_v4(&kernel, 0x200001A3, "")
}

/// A boot image v4 around an already LZ4-legacy kernel, no ramdisk: the header (magic, kernel_size, ramdisk_size 0, os_version, header_size 1584, header_version 4, cmdline, signature_size 0) padded to a 4 KiB page, then the kernel padded to a page.
fn boot_img_v4(kernel: &[u8], os_version: u32, cmdline: &str) -> Vec<u8> {
    // Pixel 8 ABL requires boot image header v4 (GKI 2.0 format). v4 extends v3 with a signature_size field at the end.
    //
    // boot_img_hdr_v4 layout: 0x000  8     magic "ANDROID!" 0x008  4     kernel_size 0x00C  4     ramdisk_size (0) 0x010  4     os_version 0x014  4     header_size (1584) 0x018  16    reserved (zeros) 0x028  4     header_version (4) 0x02C  1536  cmdline (zeros) 0x62C  4     signature_size (0 — v4 addition) Total header: 1584 bytes, padded to 4096
    const PAGE_SIZE: usize = 4096;
    const HEADER_VERSION: u32 = 4;
    const HEADER_SIZE: u32 = 1584;

    // Tensor G3 ABL requires the kernel LZ4-compressed (legacy frame): it decompresses, then jumps to the ARM64 Image entry (BOOT.md "Tensor G3 ABL facts").

    // Build the header (padded to page_size)
    let mut header = vec![0u8; PAGE_SIZE];

    header[0..8].copy_from_slice(b"ANDROID!");
    write_le32(&mut header, 0x008, kernel.len() as u32);      // kernel_size
    write_le32(&mut header, 0x00C, 0);                        // ramdisk_size
    write_le32(&mut header, 0x010, os_version);
    write_le32(&mut header, 0x014, HEADER_SIZE);               // header_size
    // 0x018..0x028: reserved (zeros)
    write_le32(&mut header, 0x028, HEADER_VERSION);            // header_version = 4
    // 0x02C..0x62C: cmdline (1536 bytes, NUL-terminated)
    assert!(cmdline.len() < 1536, "cmdline over 1535 bytes");
    header[0x02C..0x02C + cmdline.len()].copy_from_slice(cmdline.as_bytes());
    write_le32(&mut header, 0x62C, 0);                        // signature_size = 0

    // Pad kernel to page boundary
    let kernel_pages = (kernel.len() + PAGE_SIZE - 1) / PAGE_SIZE;
    let mut kernel_padded = vec![0u8; kernel_pages * PAGE_SIZE];
    kernel_padded[..kernel.len()].copy_from_slice(&kernel);

    let mut img = header;
    img.extend_from_slice(&kernel_padded);

    eprintln!("  Boot image v4: {} header + {} kernel = {} total",
        PAGE_SIZE, kernel_padded.len(), img.len());

    img
}

/// LZ4 legacy frame (magic G#02214C18 on the wire, u32 G#184C2102 LE): magic, then per 8 MiB input block a u32 LE compressed size + raw LZ4 block.
/// This is the only compression Tensor ABL accepts for the kernel payload.
fn lz4_legacy_compress(data: &[u8]) -> Vec<u8> {
    const LZ4_LEGACY_MAGIC: u32 = 0x184C2102;
    const LZ4_LEGACY_BLOCK: usize = 8 * 1024 * 1024;

    let mut out = Vec::with_capacity(data.len() / 2 + 16);
    out.extend_from_slice(&LZ4_LEGACY_MAGIC.to_le_bytes());
    for chunk in data.chunks(LZ4_LEGACY_BLOCK) {
        let block = lz4_flex::block::compress(chunk);
        out.extend_from_slice(&(block.len() as u32).to_le_bytes());
        out.extend_from_slice(&block);
    }
    out
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

/// Simple hash for the boot image ID field. Not cryptographic — just for identification in fastboot output.
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

/// Find a named argument value: --key <val> or -k <val>.
fn find_arg<'a>(args: &'a [String], long: &str, short: &str) -> Option<&'a str> {
    for i in 0..args.len().saturating_sub(1) {
        if args[i] == long || args[i] == short {
            return Some(&args[i + 1]);
        }
    }
    None
}

/// Load Ed25519 secret key from file (64 bytes).
fn load_secret_key(path: &str) -> ed25519_compact::SecretKey {
    let data = fs::read(path).unwrap_or_else(|e| {
        eprintln!("Error reading key {}: {}", path, e);
        process::exit(1);
    });
    if data.len() != 64 {
        eprintln!("Error: secret key must be 64 bytes, got {}", data.len());
        process::exit(1);
    }
    ed25519_compact::SecretKey::from_slice(&data).unwrap_or_else(|e| {
        eprintln!("Error: invalid Ed25519 secret key: {}", e);
        process::exit(1);
    })
}

// ---------------------------------------------------------------------------
// ELF symbol table parsing
// ---------------------------------------------------------------------------

const SHT_SYMTAB: u32 = 2;

/// Find a symbol's virtual address in an ELF file by name.
fn elf_find_symbol(elf_data: &[u8], name: &str) -> Option<u64> {
    if elf_data.len() < 64 || elf_data[0..4] != ELF_MAGIC { return None; }

    let e_shoff = u64::from_le_bytes(elf_data[40..48].try_into().ok()?) as usize;
    let e_shentsize = u16::from_le_bytes(elf_data[58..60].try_into().ok()?) as usize;
    let e_shnum = u16::from_le_bytes(elf_data[60..62].try_into().ok()?) as usize;

    // Find SHT_SYMTAB and its string table
    for i in 0..e_shnum {
        let sh = e_shoff + i * e_shentsize;
        if sh + e_shentsize > elf_data.len() { break; }

        let sh_type = u32::from_le_bytes(elf_data[sh + 4..sh + 8].try_into().ok()?);
        if sh_type != SHT_SYMTAB { continue; }

        let sh_offset = u64::from_le_bytes(elf_data[sh + 24..sh + 32].try_into().ok()?) as usize;
        let sh_size = u64::from_le_bytes(elf_data[sh + 32..sh + 40].try_into().ok()?) as usize;
        let sh_entsize = u64::from_le_bytes(elf_data[sh + 56..sh + 64].try_into().ok()?) as usize;
        let sh_link = u32::from_le_bytes(elf_data[sh + 40..sh + 44].try_into().ok()?) as usize;

        if sh_entsize == 0 { continue; }

        // Get string table for this symtab
        let strtab_sh = e_shoff + sh_link * e_shentsize;
        let strtab_off = u64::from_le_bytes(
            elf_data[strtab_sh + 24..strtab_sh + 32].try_into().ok()?
        ) as usize;

        // Iterate symbols
        let num_syms = sh_size / sh_entsize;
        for j in 0..num_syms {
            let sym = sh_offset + j * sh_entsize;
            if sym + sh_entsize > elf_data.len() { break; }

            let st_name = u32::from_le_bytes(
                elf_data[sym..sym + 4].try_into().ok()?
            ) as usize;
            let st_value = u64::from_le_bytes(
                elf_data[sym + 8..sym + 16].try_into().ok()?
            );

            // Read null-terminated name from strtab
            let name_start = strtab_off + st_name;
            if name_start >= elf_data.len() { continue; }
            let name_end = elf_data[name_start..]
                .iter()
                .position(|&b| b == 0)
                .map(|p| name_start + p)
                .unwrap_or(elf_data.len());
            let sym_name = std::str::from_utf8(&elf_data[name_start..name_end]).ok()?;

            if sym_name == name {
                return Some(st_value);
            }
        }
    }
    None
}

/// Find a symbol whose name ends with the given suffix (for Rust-mangled names). E.g. suffix "PUBKEY" matches "_ZN11ferros_seed6PUBKEY17h...E".
fn elf_find_symbol_suffix(elf_data: &[u8], suffix: &str) -> Option<u64> {
    if elf_data.len() < 64 || elf_data[0..4] != ELF_MAGIC { return None; }

    let e_shoff = u64::from_le_bytes(elf_data[40..48].try_into().ok()?) as usize;
    let e_shentsize = u16::from_le_bytes(elf_data[58..60].try_into().ok()?) as usize;
    let e_shnum = u16::from_le_bytes(elf_data[60..62].try_into().ok()?) as usize;

    for i in 0..e_shnum {
        let sh = e_shoff + i * e_shentsize;
        if sh + e_shentsize > elf_data.len() { break; }

        let sh_type = u32::from_le_bytes(elf_data[sh + 4..sh + 8].try_into().ok()?);
        if sh_type != SHT_SYMTAB { continue; }

        let sh_offset = u64::from_le_bytes(elf_data[sh + 24..sh + 32].try_into().ok()?) as usize;
        let sh_size = u64::from_le_bytes(elf_data[sh + 32..sh + 40].try_into().ok()?) as usize;
        let sh_entsize = u64::from_le_bytes(elf_data[sh + 56..sh + 64].try_into().ok()?) as usize;
        let sh_link = u32::from_le_bytes(elf_data[sh + 40..sh + 44].try_into().ok()?) as usize;

        if sh_entsize == 0 { continue; }

        let strtab_sh = e_shoff + sh_link * e_shentsize;
        let strtab_off = u64::from_le_bytes(
            elf_data[strtab_sh + 24..strtab_sh + 32].try_into().ok()?
        ) as usize;

        let num_syms = sh_size / sh_entsize;
        for j in 0..num_syms {
            let sym = sh_offset + j * sh_entsize;
            if sym + sh_entsize > elf_data.len() { break; }

            let st_name = u32::from_le_bytes(
                elf_data[sym..sym + 4].try_into().ok()?
            ) as usize;
            let st_value = u64::from_le_bytes(
                elf_data[sym + 8..sym + 16].try_into().ok()?
            );

            let name_start = strtab_off + st_name;
            if name_start >= elf_data.len() { continue; }
            let name_end = elf_data[name_start..]
                .iter()
                .position(|&b| b == 0)
                .map(|p| name_start + p)
                .unwrap_or(elf_data.len());
            let sym_name = std::str::from_utf8(&elf_data[name_start..name_end]).ok()?;

            // Rust mangling: look for the suffix between length prefix and hash suffix e.g. _ZN11ferros_seed6PUBKEY17h...E contains "PUBKEY"
            if sym_name.contains(suffix) && st_value != 0 {
                return Some(st_value);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Synthesizer commands
// ---------------------------------------------------------------------------

const SIGNED_TRAILER_MAGIC: &[u8; 8] = b"FERROSIG";

/// Generate Ed25519 keypair.
fn cmd_keygen(args: &[String]) {
    let dir = if args.len() >= 2 && args[0] == "-o" {
        args[1].clone()
    } else {
        "keys".to_string()
    };

    fs::create_dir_all(&dir).unwrap_or_else(|e| {
        eprintln!("Error creating {}: {}", dir, e);
        process::exit(1);
    });

    let kp = ed25519_compact::KeyPair::generate();

    let secret_path = format!("{}/ferros.secret", dir);
    let pub_path = format!("{}/ferros.pub", dir);

    fs::write(&secret_path, kp.sk.as_ref()).unwrap();
    fs::write(&pub_path, kp.pk.as_ref()).unwrap();

    // Restrict secret key permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&secret_path, fs::Permissions::from_mode(0o600)).ok();
    }

    let fingerprint = blake3::hash(kp.pk.as_ref());
    eprintln!("Generated Ed25519 keypair");
    eprintln!("  Secret: {}", secret_path);
    eprintln!("  Public: {}", pub_path);
    eprintln!("  Fingerprint: {}", &fingerprint.to_hex()[..16]);
}

/// Sign a kernel binary: ELF → flat → BLAKE3 → Ed25519 → .signed package.
fn cmd_sign_kernel(args: &[String]) {
    if args.is_empty() {
        eprintln!("Usage: ferros-mkimg sign-kernel <kernel.elf> --key <secret> -o <output>");
        process::exit(1);
    }

    let input = &args[0];
    let output = find_arg(args, "-o", "-o").unwrap_or("kernel.signed");
    let key_path = find_arg(args, "--key", "-k");

    let elf_data = fs::read(input).unwrap_or_else(|e| {
        eprintln!("Error reading {}: {}", input, e);
        process::exit(1);
    });

    eprintln!("Signing kernel: {}", input);
    let flat = elf_to_flat(&elf_data);

    let hash = blake3::hash(&flat);
    eprintln!("  BLAKE3: {}", &hash.to_hex()[..16]);

    let mut sig = [0u8; 64];
    if let Some(kp) = key_path {
        let sk = load_secret_key(kp);
        let signature = sk.sign(hash.as_bytes(), None);
        sig.copy_from_slice(signature.as_ref());
        eprintln!("  Ed25519: signed");
    } else {
        eprintln!("  Ed25519: unsigned (dev build)");
    }

    // Build .signed package: [flat][size:u32 LE][hash:32][sig:64][FERROSIG]
    let mut signed = flat.clone();
    signed.extend_from_slice(&(flat.len() as u32).to_le_bytes());
    signed.extend_from_slice(hash.as_bytes());
    signed.extend_from_slice(&sig);
    signed.extend_from_slice(SIGNED_TRAILER_MAGIC);

    write_output(output, &signed);
    eprintln!("Wrote {} ({} bytes, kernel {} bytes)",
        output, signed.len(), flat.len());
}

/// Build signed seed boot.img: ELF → patch pubkey+sig → boot.img.
fn cmd_seed(args: &[String]) {
    if args.is_empty() {
        eprintln!("Usage: ferros-mkimg seed <seed.elf> --key <secret> [--pad <MiB>] -o <output>");
        process::exit(1);
    }

    let (args, pad_mib) = extract_pad(args);
    let args = &args[..];
    let input = &args[0];
    let output = find_arg(args, "-o", "-o").unwrap_or("seed.img");
    let key_path = find_arg(args, "--key", "-k");

    let elf_data = fs::read(input).unwrap_or_else(|e| {
        eprintln!("Error reading {}: {}", input, e);
        process::exit(1);
    });

    eprintln!("Building signed seed: {}", input);

    // Find PUBKEY, SEED_SIG, and __bss_start in the ELF symbol table. Rust mangles statics, so search by suffix.
    let pubkey_addr = elf_find_symbol_suffix(&elf_data, "PUBKEY").unwrap_or_else(|| {
        eprintln!("Error: PUBKEY symbol not found in ELF");
        process::exit(1);
    });
    let sig_addr = elf_find_symbol_suffix(&elf_data, "SEED_SIG").unwrap_or_else(|| {
        eprintln!("Error: SEED_SIG symbol not found in ELF");
        process::exit(1);
    });
    let bss_start_addr = elf_find_symbol(&elf_data, "__bss_start").unwrap_or_else(|| {
        eprintln!("Error: __bss_start symbol not found in ELF");
        process::exit(1);
    });

    let mut flat = elf_to_flat(&elf_data);

    // Pad BEFORE hashing/signing: the image_size patch at G#10 is inside the signed _start..__bss_start range, so it must be in place when the hash is computed (and the loaded in-memory copy carries the same bytes, keeping self_verify consistent).
    if let Some(mib) = pad_mib {
        flat = pad_kernel(flat, mib);
    }

    // Convert symbol addresses to flat binary offsets
    let segments = parse_elf_segments(&elf_data);
    let base_addr = segments[0].vaddr;
    let pubkey_off = (pubkey_addr - base_addr) as usize;
    let sig_off = (sig_addr - base_addr) as usize;
    let bss_off = (bss_start_addr - base_addr) as usize;

    eprintln!("  PUBKEY offset: G#{:X} (vaddr G#{:X})", pubkey_off, pubkey_addr);
    eprintln!("  SEED_SIG offset: G#{:X} (vaddr G#{:X})", sig_off, sig_addr);
    eprintln!("  __bss_start offset: G#{:X} (vaddr G#{:X})", bss_off, bss_start_addr);

    // Bounds check
    if pubkey_off + 32 > flat.len() || sig_off + 64 > flat.len() {
        eprintln!("Error: symbol offsets exceed binary size");
        process::exit(1);
    }
    if bss_off > flat.len() || sig_off + 64 > bss_off {
        eprintln!("Error: __bss_start must be after SEED_SIG");
        process::exit(1);
    }

    if let Some(kp) = key_path {
        let sk = load_secret_key(kp);
        let pk = sk.public_key();

        // Patch public key
        flat[pubkey_off..pubkey_off + 32].copy_from_slice(pk.as_ref());
        eprintln!("  Patched pubkey at G#{:X}", pubkey_off);

        // Hash only _start..__bss_start (file-backed data, excludes BSS/stack). Matches seed's self_verify() which hashes the same range. Signature region is still zero (matches self_verify's zeroing logic).
        let hash = blake3::hash(&flat[..bss_off]);
        eprintln!("  BLAKE3 (..bss): {}", &hash.to_hex()[..16]);

        // Sign
        let signature = sk.sign(hash.as_bytes(), None);

        // Patch signature
        flat[sig_off..sig_off + 64].copy_from_slice(signature.as_ref());
        eprintln!("  Patched signature at G#{:X}", sig_off);
    } else {
        eprintln!("  Unsigned dev build (pubkey and sig zeroed)");
    }

    // Wrap as boot.img
    let boot_img = make_boot_img(&flat);
    write_output(output, &boot_img);
    eprintln!("Wrote seed.img -> {} ({} bytes)", output, boot_img.len());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip the LZ4 legacy frame: compress >8 MiB (forces multiple blocks), re-parse the frame, decompress each block, compare byte-exact.
    #[test]
    fn lz4_legacy_roundtrip() {
        const BLOCK: usize = 8 * 1024 * 1024;
        let mut data = vec![0u8; BLOCK + BLOCK / 2];
        for (i, b) in data.iter_mut().enumerate() {
            *b = ((i * 31) ^ (i >> 8)) as u8;
        }

        let frame = lz4_legacy_compress(&data);
        assert_eq!(&frame[0..4], &0x184C2102u32.to_le_bytes());

        let mut out = Vec::new();
        let mut pos = 4;
        while pos + 4 <= frame.len() {
            let csize = u32::from_le_bytes(frame[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            let block = lz4_flex::block::decompress(&frame[pos..pos + csize], BLOCK).unwrap();
            out.extend_from_slice(&block);
            pos += csize;
        }
        assert_eq!(out, data);
    }
}

fn arg(args: &[String], name: &str) -> String {
    match args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)) {
        Some(v) => v.clone(),
        None => {
            eprintln!("missing {name}");
            process::exit(1)
        }
    }
}

fn die(what: &str, e: impl std::fmt::Display) -> ! {
    eprintln!("{what}: {e}");
    process::exit(1)
}

fn sha(b: &[u8]) -> String {
    avb::hex(&<sha2::Sha256 as sha2::Digest>::digest(b))
}

/// The owner's AVB key (BOOT.md, "Locked-bootloader endgame"): RSA-4096 because ABL's avb_custom_key takes only RSA, as one VSF document. Refuses to overwrite a key already there.
fn cmd_avb_keygen(args: &[String]) {
    let path = format!("{}/avb.vsf", arg(args, "-o"));
    if fs::metadata(&path).is_ok() {
        die(&path, "exists: not overwriting a signing key");
    }
    let (key, doc) = avb::generate().unwrap_or_else(|e| die("keygen", e));
    fs::write(&path, &doc).unwrap_or_else(|e| die(&path, e));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).ok();
    }
    // Read back: the document on disk must make the same key.
    let again = avb::load(&fs::read(&path).unwrap()).unwrap_or_else(|e| die("the key read back", e));
    if avb::public_key_blob(&again) != avb::public_key_blob(&key) {
        die(&path, "the key read back is not the one generated: unusable");
    }
    eprintln!("Generated RSA-4096 AVB key: {path} ({} bytes, VSF)", doc.len());
    eprintln!("  public-key blob sha256: {}", sha(&avb::public_key_blob(&key)));
}

/// The public-key blob for `fastboot flash avb_custom_key`.
fn cmd_avb_pubkey(args: &[String]) {
    let key = avb::load(&fs::read(arg(args, "--key")).unwrap_or_else(|e| die("key", e))).unwrap_or_else(|e| die("key", e));
    let out = arg(args, "-o");
    let blob = avb::public_key_blob(&key);
    fs::write(&out, &blob).unwrap_or_else(|e| die(&out, e));
    eprintln!("{out}: {} bytes, sha256 {}", blob.len(), sha(&blob));
}

fn cmd_vbmeta_info(args: &[String]) {
    let file = fs::read(args.first().map(String::as_str).unwrap_or("")).unwrap_or_else(|e| die("read", e));
    let (img, footer) = avb::find(&file).unwrap_or_else(|e| die("vbmeta", e));
    let v = avb::parse(img).unwrap_or_else(|e| die("vbmeta", e));
    if let Some(size) = footer {
        println!("footer: image {size} bytes, vbmeta {} bytes", img.len());
    }
    println!("algorithm {}  rollback index {} at location {}  flags {}  libavb {}.{}  release \"{}\"", v.algorithm, v.rollback_index, v.rollback_index_location, v.flags, u32::from_be_bytes(v.header[4..8].try_into().unwrap()), u32::from_be_bytes(v.header[8..12].try_into().unwrap()), v.release);
    println!("hash: {}   signature: {}", match avb::hash_ok(img) { Some(true) => "matches", Some(false) => "DOES NOT MATCH (patched after signing)", None => "none" }, match avb::signature_ok(img) { Ok(true) => "verifies".to_string(), Ok(false) => "DOES NOT VERIFY".to_string(), Err(e) => e });
    println!("public key: {} bytes, sha256 {}", v.public_key.len(), sha(&v.public_key));
    for d in &v.descriptors {
        println!("  {}", avb::describe(d));
    }
}

/// Both images a slot needs, made together so they cannot drift: the gate image as a whole boot partition with its own signed footer, and the top-level vbmeta whose boot chain names the same key. Each is checked as ABL will check it before anything is written.
fn cmd_avb_sign(args: &[String]) {
    let key = avb::load(&fs::read(arg(args, "--key")).unwrap_or_else(|e| die("key", e))).unwrap_or_else(|e| die("key", e));
    let google_vbmeta = fs::read(arg(args, "--google-vbmeta")).unwrap_or_else(|e| die("google vbmeta", e));
    let google_boot = fs::read(arg(args, "--google-boot")).unwrap_or_else(|e| die("google boot", e));
    let gate = fs::read(arg(args, "--gate")).unwrap_or_else(|e| die("gate", e));
    let (out_vbmeta, out_boot) = (arg(args, "--out-vbmeta"), arg(args, "--out-boot"));
    let top = avb::parse(&google_vbmeta).unwrap_or_else(|e| die("google vbmeta", e));
    let (gb, _) = avb::find(&google_boot).unwrap_or_else(|e| die("google boot", e));
    let gb = avb::parse(gb).unwrap_or_else(|e| die("google boot", e));
    // The boot partition is as large as Google's whole-partition boot image.
    let salt: [u8; 32] = rand::random();
    let boot = avb::boot_partition(&gate, google_boot.len(), &gb, &key, &salt).unwrap_or_else(|e| die("boot", e));
    let vbmeta = avb::top_vbmeta(&top, &key).unwrap_or_else(|e| die("vbmeta", e));
    // Check both as ABL will: hashes, signatures, the gate's digest, and the boot chain naming the key that signed the footer.
    let blob = avb::public_key_blob(&key);
    let (bv, size) = avb::find(&boot).unwrap_or_else(|e| die("boot (check)", e));
    let bp = avb::parse(bv).unwrap();
    let gate_ok = bp.descriptors.iter().filter_map(|d| avb::hash_of(d)).any(|(name, n, salt, digest)| {
        let mut h = <sha2::Sha256 as sha2::Digest>::new();
        sha2::Digest::update(&mut h, &salt);
        sha2::Digest::update(&mut h, &boot[..n as usize]);
        name == "boot" && Some(n) == size && sha2::Digest::finalize(h).as_slice() == digest.as_slice()
    });
    let tp = avb::parse(&vbmeta).unwrap();
    let chain_ok = tp.descriptors.iter().filter_map(|d| avb::chain_of(d)).any(|(name, k)| name == "boot" && k == blob);
    let checks = [
        ("boot footer hash", avb::hash_ok(bv) == Some(true)),
        ("boot footer signature", avb::signature_ok(bv).unwrap_or(false)),
        ("boot footer key is ours", bp.public_key == blob),
        ("gate digest", gate_ok),
        ("vbmeta hash", avb::hash_ok(&vbmeta) == Some(true)),
        ("vbmeta signature", avb::signature_ok(&vbmeta).unwrap_or(false)),
        ("vbmeta key is ours", tp.public_key == blob),
        ("boot chain names our key", chain_ok),
        ("other descriptors Google's", tp.descriptors.len() == top.descriptors.len()),
    ];
    for (what, ok) in checks {
        if !ok {
            die("check failed, nothing written", what);
        }
    }
    fs::write(&out_boot, &boot).unwrap_or_else(|e| die(&out_boot, e));
    fs::write(&out_vbmeta, &vbmeta).unwrap_or_else(|e| die(&out_vbmeta, e));
    eprintln!("boot:   {out_boot} ({} bytes: gate {} bytes + footer vbmeta {} bytes, rollback {} at location {})", boot.len(), gate.len(), bv.len(), bp.rollback_index, bp.rollback_index_location);
    eprintln!("vbmeta: {out_vbmeta} ({} bytes, {} descriptors, rollback {} at location {})", vbmeta.len(), tp.descriptors.len(), tp.rollback_index, tp.rollback_index_location);
    eprintln!("key:    sha256 {}", sha(&blob));
    eprintln!("checked: {}", checks.iter().map(|(w, _)| *w).collect::<Vec<_>>().join(", "));
}

/// LZ4 legacy frame to its bytes: magic, then per block a u32 LE compressed size and a raw LZ4 block of at most 8 MiB out (a repeated magic between blocks is skipped, as concatenated frames carry one).
fn lz4_legacy_decompress(d: &[u8]) -> Result<Vec<u8>, String> {
    const MAGIC: u32 = 0x184C2102;
    if d.len() < 4 || u32::from_le_bytes(d[0..4].try_into().unwrap()) != MAGIC {
        return Err("not an LZ4 legacy frame".into());
    }
    let mut out = Vec::new();
    let mut p = 4;
    while p + 4 <= d.len() {
        let n = u32::from_le_bytes(d[p..p + 4].try_into().unwrap()) as usize;
        p += 4;
        if n as u32 == MAGIC {
            continue;
        }
        if n == 0 || p + n > d.len() {
            break;
        }
        out.extend_from_slice(&lz4_flex::block::decompress(&d[p..p + n], 8 << 20).map_err(|e| e.to_string())?);
        p += n;
    }
    Ok(out)
}

/// `unpack-kernel --boot <boot.img> -o <Image>`: the kernel of a boot image v4, out of its LZ4 legacy frame (what husky-hv.sh used Google's unpack_bootimg.py and Python's lz4 for).
fn cmd_unpack_kernel(args: &[String]) {
    let boot = fs::read(arg(args, "--boot")).unwrap_or_else(|e| die("boot", e));
    let out = arg(args, "-o");
    if boot.len() < 4096 || &boot[0..8] != b"ANDROID!" || u32::from_le_bytes(boot[0x28..0x2C].try_into().unwrap()) != 4 {
        die("boot", "not a boot image v4");
    }
    let ks = u32::from_le_bytes(boot[8..12].try_into().unwrap()) as usize;
    if 4096 + ks > boot.len() {
        die("boot", "kernel_size runs past the image");
    }
    let img = lz4_legacy_decompress(&boot[4096..4096 + ks]).unwrap_or_else(|e| die("kernel", e));
    fs::write(&out, &img).unwrap_or_else(|e| die(&out, e));
    eprintln!("{out}: {} bytes (kernel {} bytes LZ4 legacy)", img.len(), ks);
}

/// `gate --hv <hv.bin> --image <Linux Image> [--ferros <ferros.bin>] [--cmdline <s>] -o <boot.img>`: the boot gate's image (tools/husky-hv.sh, HV.md), in Rust. The blob is the hv padded to G#18_0000, then the Linux Image (2 MiB aligned at G#8020_0000 once ABL loads the blob at G#8008_0000), then optionally the cold ferros image past Linux's image_size rounded up to 2 MiB, behind a 4 KiB header {"FERROSGT", u64 length} that gate.rs finds. LZ4 legacy, in a boot image v4 with no ramdisk, os_version 0 and the command line given (what mkbootimg.py wrote).
fn cmd_gate(args: &[String]) {
    const PAD: usize = 0x18_0000;
    const LOAD: usize = 0x8008_0000;
    const RAMDISKS: usize = 0x8600_0000;
    let hv = fs::read(arg(args, "--hv")).unwrap_or_else(|e| die("hv", e));
    let img = fs::read(arg(args, "--image")).unwrap_or_else(|e| die("image", e));
    let out = arg(args, "-o");
    let cmdline = args.iter().position(|a| a == "--cmdline").and_then(|i| args.get(i + 1)).cloned().unwrap_or_default();
    let ferros = args.iter().position(|a| a == "--ferros").and_then(|i| args.get(i + 1)).map(|p| fs::read(p).unwrap_or_else(|e| die("ferros", e)));
    if hv.len() > PAD {
        die("hv", format!("{} bytes, over the G#18_0000 pad", hv.len()));
    }
    // The ARM64 Image header: text_offset 0, image_size, flags with bit 3 (4 KiB pages anywhere), magic "ARM\x64".
    let le64 = |at: usize| u64::from_le_bytes(img[at..at + 8].try_into().unwrap()) as usize;
    if img.len() < 64 || &img[56..60] != b"ARM\x64" || le64(8) != 0 || le64(24) & 8 == 0 {
        die("image", "not an ARM64 Image with text_offset 0 and flags bit 3");
    }
    let image_size = le64(16);
    let mut blob = hv.clone();
    blob.resize(PAD, 0);
    blob.extend_from_slice(&img);
    if let Some(fer) = &ferros {
        let at = PAD + ((image_size + 0x1F_FFFF) & !0x1F_FFFF);
        if LOAD + at + 0x1000 + fer.len() >= RAMDISKS {
            die("ferros", "the image would reach ABL's ramdisks at G#8600_0000");
        }
        blob.resize(at, 0);
        let mut hdr = vec![0u8; 0x1000];
        hdr[0..8].copy_from_slice(b"FERROSGT");
        hdr[8..16].copy_from_slice(&(fer.len() as u64).to_le_bytes());
        blob.extend_from_slice(&hdr);
        blob.extend_from_slice(fer);
        eprintln!("ferros {} B packed at G#{:X}", fer.len(), LOAD + at);
    }
    let kernel = lz4_legacy_compress(&blob);
    let boot = boot_img_v4(&kernel, 0, &cmdline);
    fs::write(&out, &boot).unwrap_or_else(|e| die(&out, e));
    eprintln!("hv {} B, Image {} B (image_size G#{:X}), blob {} B, lz4 {} B, boot {} B", hv.len(), img.len(), image_size, blob.len(), kernel.len(), boot.len());
}
