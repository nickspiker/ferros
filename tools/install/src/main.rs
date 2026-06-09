// ferros-install — Partition, format, and bootstrap ferros from macOS.
//
// Modeled after the Asahi Linux installer flow but entirely in Rust. Handles the full lifecycle: discover disk, resize macOS, create partition with the BLAKE3-derived GPT type GUID, format the ring layout, and write the genesis spine entry.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, Read, Seek, SeekFrom, Write};
use std::process::Command;

use ferros_layout::*;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// GPT type GUID: BLAKE3("ferros") truncated to 128 bits.
const FERROS_TYPE_GUID: &str = "A0B51225-61C5-0F5A-FFE7-1B644F9CA954";

/// Minimum macOS partition size we'll allow after resize.
const MIN_MACOS_BYTES: u64 = 40 * (1 << 30); // 40 GB

/// Minimum ferros partition size (must fit all ring regions). Reserved(4MB) + Stem(1MB) + Kernels(16MB) + Spine(256MB)
/// + State(1GB) + Ledger(1GB) + some tract = ~2.3 GB minimum.
const MIN_FERROS_BYTES: u64 = 4 * (1 << 30); // 4 GB floor

/// Logo (embedded SVG source for future splash/icon use).
const _LOGO_SVG: &[u8] = include_bytes!("../../../logo.svg");

/// Path to sgdisk binary (installed via Homebrew).
const SGDISK: &str = "/opt/homebrew/bin/sgdisk";

// ---------------------------------------------------------------------------
// Interactive I/O
// ---------------------------------------------------------------------------

fn prompt(msg: &str) -> String {
   print!("{}", msg);
   io::stdout().flush().unwrap();
   let mut line = String::new();
   io::stdin().lock().read_line(&mut line).unwrap();
   line.trim().to_string()
}

fn confirm(msg: &str) -> bool {
   let resp = prompt(&format!("  {} [y/N] ", msg));
   resp.eq_ignore_ascii_case("y") || resp.eq_ignore_ascii_case("yes")
}

fn parse_size_input(input: &str, total: u64) -> Option<u64> {
   let s = input.trim();
   if s.ends_with('%') {
      let pct: f64 = s[..s.len() - 1].parse().ok()?;
      Some((pct / 100.0 * total as f64) as u64)
   } else {
      let s_upper = s.to_uppercase();
      if s_upper.ends_with("GB") {
         let n: f64 = s_upper[..s_upper.len() - 2].trim().parse().ok()?;
         Some((n * (1u64 << 30) as f64) as u64)
      } else if s_upper.ends_with("MB") {
         let n: f64 = s_upper[..s_upper.len() - 2].trim().parse().ok()?;
         Some((n * (1u64 << 20) as f64) as u64)
      } else {
         s.parse::<u64>().ok()
      }
   }
}

// ---------------------------------------------------------------------------
// Display helpers (G# format per project convention)
// ---------------------------------------------------------------------------

fn fmt_hex(val: u64) -> String {
   format!("G#{:X}", val)
}

fn fmt_size(bytes: u64) -> String {
   if bytes >= 1 << 30 {
      format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
   } else if bytes >= 1 << 20 {
      format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64)
   } else if bytes >= 1 << 10 {
      format!("{:.1} KB", bytes as f64 / (1u64 << 10) as f64)
   } else {
      format!("{} B", bytes)
   }
}

// ---------------------------------------------------------------------------
// Shell helpers
// ---------------------------------------------------------------------------

fn run(cmd: &str, args: &[&str]) -> io::Result<String> {
   let output = Command::new(cmd).args(args).output()?;
   if !output.status.success() {
      let stderr = String::from_utf8_lossy(&output.stderr);
      let stdout = String::from_utf8_lossy(&output.stdout);
      return Err(io::Error::new(
         io::ErrorKind::Other,
         format!("{} failed: {}{}", cmd, stderr, stdout),
      ));
   }
   Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_verbose(cmd: &str, args: &[&str]) -> io::Result<()> {
   println!("  $ {} {}", cmd, args.join(" "));
   let output = Command::new(cmd).args(args).output()?;
   if !output.stdout.is_empty() {
      print!("{}", String::from_utf8_lossy(&output.stdout));
   }
   if !output.stderr.is_empty() {
      eprint!("{}", String::from_utf8_lossy(&output.stderr));
   }
   if !output.status.success() {
      return Err(io::Error::new(
         io::ErrorKind::Other,
         format!("{} exited with {}", cmd, output.status),
      ));
   }
   Ok(())
}

// ---------------------------------------------------------------------------
// Preflight checks
// ---------------------------------------------------------------------------

fn check_root() -> io::Result<()> {
   if libc::geteuid() != 0 {
      eprintln!();
      eprintln!("  This command requires root. Run with sudo:");
      eprintln!("    sudo cargo run -p ferros-install -- <command>");
      eprintln!("  or");
      eprintln!("    sudo ferros-install <command>");
      eprintln!();
      return Err(io::Error::new(io::ErrorKind::PermissionDenied, "not root"));
   }
   Ok(())
}

fn check_sgdisk() -> io::Result<()> {
   if !std::path::Path::new(SGDISK).exists() {
      eprintln!();
      eprintln!("  sgdisk not found at {}", SGDISK);
      eprintln!("  Install it: brew install gptfdisk");
      eprintln!();
      return Err(io::Error::new(io::ErrorKind::NotFound, "sgdisk missing"));
   }
   Ok(())
}

// ---------------------------------------------------------------------------
// Disk discovery
// ---------------------------------------------------------------------------

struct DiskInfo {
   name: String,
   partitions: Vec<PartInfo>,
}

struct PartInfo {
   device: String,
   type_name: String,
   size_bytes: u64,
   label: String,
   type_guid: String,
   is_free: bool,
}

/// Find the system disk by looking for the one with Apple_APFS_ISC as its first partition (same method as the Asahi installer).
fn find_system_disk() -> io::Result<String> {
   let list_out = run("diskutil", &["list", "-plist"])?;
   // On Apple Silicon the system disk always has Apple_APFS_ISC first. Check disk0 explicitly.
   let disk0_out = run("diskutil", &["list", "disk0"])?;
   if disk0_out.contains("Apple_APFS_ISC") {
      return Ok("disk0".to_string());
   }
   // Fall back: scan all physical disks
   for line in list_out.lines() {
      if line.contains("/dev/disk") {
         let dev = line.trim().trim_start_matches("/dev/");
         if dev.starts_with("disk") && !dev.contains('s') {
            let check = run("diskutil", &["list", dev])?;
            if check.contains("Apple_APFS_ISC") {
               return Ok(dev.to_string());
            }
         }
      }
   }
   Err(io::Error::new(io::ErrorKind::NotFound, "Cannot find Apple Silicon system disk (no Apple_APFS_ISC)"))
}

fn discover_disk() -> io::Result<DiskInfo> {
   let disk_name = find_system_disk()?;
   let list_out = run("diskutil", &["list", &disk_name])?;

   let mut partitions = Vec::new();
   for line in list_out.lines() {
      let trimmed = line.trim();
      let prefix = format!("{}s", disk_name);
      if let Some(dev) = trimmed.split_whitespace().last() {
         if !dev.starts_with(&prefix) {
            continue;
         }
         let info_out = run("diskutil", &["info", dev])?;
         let size = parse_diskutil_field(&info_out, "Disk Size:");
         let label = parse_diskutil_string(&info_out, "Volume Name:");
         let type_name = parse_diskutil_string(&info_out, "Partition Type:");
         let type_guid = type_name.to_uppercase();

         partitions.push(PartInfo {
            device: dev.to_string(),
            type_name,
            size_bytes: size,
            label,
            type_guid,
            is_free: false,
         });
      }
   }

   // Detect free space regions from diskutil output
   for line in list_out.lines() {
      if line.contains("(free space") {
         let parts: Vec<&str> = line.split_whitespace().collect();
         for (i, p) in parts.iter().enumerate() {
            if (*p == "GB" || *p == "MB" || *p == "TB") && i > 0 {
               if let Some(num_str) = parts.get(i - 1) {
                  if let Ok(num) = num_str.parse::<f64>() {
                     let bytes = match *p {
                        "TB" => (num * (1u64 << 40) as f64) as u64,
                        "GB" => (num * (1u64 << 30) as f64) as u64,
                        _ =>    (num * (1u64 << 20) as f64) as u64,
                     };
                     partitions.push(PartInfo {
                        device: String::new(),
                        type_name: "(free space)".to_string(),
                        size_bytes: bytes,
                        label: String::new(),
                        type_guid: String::new(),
                        is_free: true,
                     });
                  }
               }
            }
         }
      }
   }

   Ok(DiskInfo { name: disk_name, partitions })
}

fn print_disk_layout(disk: &DiskInfo) {
   println!();
   println!("  Disk layout: {}", disk.name);
   println!("  ─────────────────────────────────────────────────────────");
   for p in &disk.partitions {
      if p.is_free {
         println!("  {:>10}   (free space)", fmt_size(p.size_bytes));
      } else {
         let label = if p.label.is_empty() { &p.type_name } else { &p.label };
         let marker = if p.type_guid.contains("A0B51225") { " ← ferros" } else { "" };
         println!("  {:>10}   {} [{}]{}", fmt_size(p.size_bytes), label, p.device, marker);
      }
   }
   println!();
}

// ---------------------------------------------------------------------------
// Partition discovery (find existing ferros partition)
// ---------------------------------------------------------------------------

struct Partition {
   device: String,
   #[allow(dead_code)]
   disk: String,
   raw_device: String,
   size_bytes: u64,
   name: String,
}

fn find_partition() -> Option<Partition> {
   let disk_name = find_system_disk().ok()?;
   let list_out = run("diskutil", &["list", &disk_name]).ok()?;

   let prefix = format!("{}s", disk_name);
   for line in list_out.lines() {
      let trimmed = line.trim();
      if let Some(dev) = trimmed.split_whitespace().last() {
         if !dev.starts_with(&prefix) {
            continue;
         }
         let info_out = run("diskutil", &["info", dev]).ok()?;
         if info_out.to_uppercase().contains(FERROS_TYPE_GUID) {
            let size = parse_diskutil_field(&info_out, "Disk Size:");
            let name = parse_diskutil_string(&info_out, "Volume Name:");
            return Some(Partition {
               device: dev.to_string(),
               disk: disk_name,
               raw_device: format!("/dev/r{}", dev),
               size_bytes: size,
               name,
            });
         }
      }
   }
   None
}

fn parse_diskutil_field(info: &str, field: &str) -> u64 {
   for line in info.lines() {
      if line.contains(field) {
         if let Some(start) = line.find('(') {
            let rest = &line[start + 1..];
            if let Some(end) = rest.find(" Bytes") {
               if let Ok(val) = rest[..end].parse::<u64>() {
                  return val;
               }
            }
         }
      }
   }
   0
}

fn parse_diskutil_string(info: &str, field: &str) -> String {
   for line in info.lines() {
      if line.contains(field) {
         if let Some(val) = line.split(':').nth(1) {
            return val.trim().to_string();
         }
      }
   }
   String::new()
}

// ---------------------------------------------------------------------------
// Partition info
// ---------------------------------------------------------------------------

fn print_info(part: &Partition) {
   let total_blocks = part.size_bytes / BLOCK_SIZE as u64;

   println!();
   println!("  ferros partition");
   println!("  ─────────────────────────────────────────────");
   println!("  Device:     {}", part.device);
   println!("  Raw device: {}", part.raw_device);
   println!("  Name:       {}", if part.name.is_empty() { "(none)" } else { &part.name });
   println!("  Size:       {} ({} blocks)", fmt_size(part.size_bytes), fmt_hex(total_blocks));
   println!("  Type GUID:  {}", FERROS_TYPE_GUID);
   println!();
   println!("  Ring layout:");
   println!("  ─────────────────────────────────────────────");
   println!("  Reserved     {}─{}       4 MB", fmt_hex(0), fmt_hex(0x3FF));
   println!("  Seed A       {}            16 KB", fmt_hex(SEED_A_BLOCK as u64));
   println!("  Seed B       {}            16 KB", fmt_hex(SEED_B_BLOCK as u64));
   println!("  Stem         {}─{}       1 MB   ({} entries)", fmt_hex(STEM_BASE as u64), fmt_hex(STEM_BASE as u64 + STEM_SIZE as u64 - 1), STEM_SIZE);
   println!("  Kernel A     {}─{}       8 MB", fmt_hex(KERNEL_A_BASE as u64), fmt_hex(KERNEL_A_BASE as u64 + KERNEL_MAX_BLOCKS as u64 - 1));
   println!("  Kernel B     {}─{}      8 MB", fmt_hex(KERNEL_B_BASE as u64), fmt_hex(KERNEL_B_BASE as u64 + KERNEL_MAX_BLOCKS as u64 - 1));
   println!("  Spine        {}─{}    256 MB ({} entries)", fmt_hex(SPINE_BASE as u64), fmt_hex(SPINE_BASE as u64 + SPINE_SIZE as u64 - 1), SPINE_SIZE);
   println!("  State        {}─{}   1 GB   ({} entries)", fmt_hex(STATE_RING_BASE as u64), fmt_hex(STATE_RING_BASE as u64 + STATE_RING_SIZE as u64 - 1), STATE_RING_SIZE);
   println!("  Ledger       {}─{}   1 GB   ({} entries)", fmt_hex(LEDGER_RING_BASE as u64), fmt_hex(LEDGER_RING_BASE as u64 + LEDGER_RING_SIZE as u64 - 1), LEDGER_RING_SIZE);
   println!("  Tract        {}+             {} (plow-managed)", fmt_hex(TRACT_BASE as u64), fmt_size((total_blocks.saturating_sub(TRACT_BASE as u64)) * BLOCK_SIZE as u64));
   println!();
}

// ---------------------------------------------------------------------------
// Install — full fresh install flow (resize + create + format + genesis)
// ---------------------------------------------------------------------------

fn cmd_install() -> io::Result<()> {
   check_root()?;
   check_sgdisk()?;

   println!();
   println!("  Fresh install — create ferros partition from macOS");
   println!();

   // Check if ferros partition already exists
   if let Some(existing) = find_partition() {
      println!("  Existing ferros partition found: {} ({})", existing.device, fmt_size(existing.size_bytes));
      if !confirm("Wipe and reinstall?") {
         println!("  Aborted.");
         return Ok(());
      }
      print_info(&existing);
      cmd_format(&existing)?;
      cmd_genesis(&existing)?;
      println!("  Reinstall complete.");
      return Ok(());
   }

   let disk = discover_disk()?;
   print_disk_layout(&disk);

   // Check for free space
   let free_space: u64 = disk.partitions.iter()
      .filter(|p| p.is_free)
      .map(|p| p.size_bytes)
      .sum();

   if free_space >= MIN_FERROS_BYTES {
      println!("  Free space available: {}", fmt_size(free_space));
      println!();
      if confirm("Create ferros partition in free space?") {
         return install_into_free(&disk);
      }
   } else if free_space > 0 {
      println!("  Free space ({}) is too small for ferros (minimum {}).",
         fmt_size(free_space), fmt_size(MIN_FERROS_BYTES));
      println!("  Need to resize macOS to free more space.");
      println!();
   }

   // Need to resize macOS
   let macos_part = disk.partitions.iter()
      .find(|p| p.type_name.contains("Apple_APFS") && !p.device.is_empty())
      .and_then(|p| {
         // Verify it's actually the macOS data container (has volumes)
         let info = run("diskutil", &["info", &p.device]).ok()?;
         if info.contains("APFS Container") || info.contains("Apple_APFS") {
            Some(p)
         } else {
            None
         }
      });

   let macos_part = match macos_part {
      Some(p) => p,
      None => {
         eprintln!("  Cannot find macOS APFS container to resize.");
         eprintln!("  Expected an Apple_APFS partition on {}.", disk.name);
         return Err(io::Error::new(io::ErrorKind::NotFound, "No macOS APFS container"));
      }
   };

   // Query actual APFS usage to give informed guidance
   let limits = run("diskutil", &["apfs", "resizeContainer", &macos_part.device, "limits", "-plist"]);
   let min_recommended = match &limits {
      Ok(plist) => parse_diskutil_field(plist, "MinimumSizePreferred:"),
      Err(_) => 0,
   };
   let effective_min = if min_recommended > MIN_MACOS_BYTES { min_recommended } else { MIN_MACOS_BYTES };

   println!("  macOS partition: {} ({})", macos_part.device, fmt_size(macos_part.size_bytes));
   if min_recommended > 0 {
      println!("  macOS reports minimum size: {}", fmt_size(min_recommended));
   }
   println!();
   println!("  Enter the new size for macOS (e.g. '80GB', '50%', 'min'):");
   println!("  Minimum: {}", fmt_size(effective_min));
   println!();

   let new_macos_size = loop {
      let input = prompt("  macOS new size: ");
      if input.eq_ignore_ascii_case("min") {
         break effective_min;
      }
      if input.eq_ignore_ascii_case("q") || input.eq_ignore_ascii_case("quit") {
         println!("  Aborted.");
         return Ok(());
      }
      match parse_size_input(&input, macos_part.size_bytes) {
         Some(size) if size >= effective_min => break size,
         Some(size) if size < effective_min => {
            println!("  Too small: {} (minimum {})", fmt_size(size), fmt_size(effective_min));
            if min_recommended > MIN_MACOS_BYTES {
               println!("  Note: macOS reports it needs at least {} (APFS snapshots, etc).", fmt_size(min_recommended));
               println!("  Try deleting Time Machine snapshots: tmutil deletelocalsnapshots /");
            }
         }
         _ => println!("  Invalid size. Try '80GB', '50%', 'min', or 'quit'."),
      }
   };

   let freed = macos_part.size_bytes.saturating_sub(new_macos_size);
   if freed < MIN_FERROS_BYTES {
      eprintln!("  Only {} would be freed — not enough for ferros (minimum {}).",
         fmt_size(freed), fmt_size(MIN_FERROS_BYTES));
      return Err(io::Error::new(io::ErrorKind::Other, "not enough space"));
   }

   println!();
   println!("  macOS will be resized to: {}", fmt_size(new_macos_size));
   println!("  Space freed for ferros:   {}", fmt_size(freed));
   println!();
   println!("  WARNING: Resizing the macOS partition cannot be easily undone.");
   println!("  Your system may appear to freeze during resize. This is normal.");

   if !confirm("Proceed with resize?") {
      println!("  Aborted.");
      return Ok(());
   }

   println!();
   let size_str = format!("{}B", new_macos_size);
   run_verbose("diskutil", &["apfs", "resizeContainer", &macos_part.device, &size_str])?;

   println!();
   println!("  Resize complete.");

   let disk = discover_disk()?;
   print_disk_layout(&disk);
   install_into_free(&disk)
}

fn install_into_free(disk: &DiskInfo) -> io::Result<()> {
   let free_bytes: u64 = disk.partitions.iter()
      .filter(|p| p.is_free)
      .map(|p| p.size_bytes)
      .sum();

   if free_bytes < MIN_FERROS_BYTES {
      return Err(io::Error::new(io::ErrorKind::Other,
         format!("Free space ({}) below minimum ({})", fmt_size(free_bytes), fmt_size(MIN_FERROS_BYTES))));
   }

   println!("  Creating ferros partition ({}) on {}...", fmt_size(free_bytes), disk.name);
   println!();

   // Confirm before writing to the partition table
   println!("  This will modify the GPT partition table on /dev/{}.", disk.name);
   if !confirm("Create partition?") {
      println!("  Aborted.");
      return Ok(());
   }
   println!();

   // Single sgdisk call: auto-pick entry, fill all free space, correct GUID, named
   let dev_path = format!("/dev/{}", disk.name);
   let type_arg = format!("0:{}", FERROS_TYPE_GUID);
   run_verbose(SGDISK, &["-n", "0:0:0", "-t", &type_arg, "-c", "0:ferros", &dev_path])?;

   // Wait for the kernel to re-read the partition table, then retry discovery
   println!();
   println!("  Waiting for partition table update...");
   let part = retry_find_partition(8)?;

   print_info(&part);
   cmd_format(&part)?;
   cmd_genesis(&part)?;

   println!("  ─────────────────────────────────────────────");
   println!("  ferros partition ready.");
   println!("  Device: {} ({})", part.device, fmt_size(part.size_bytes));
   println!("  Genesis state written. Kernel can boot from spine generation 0.");
   println!();

   Ok(())
}

/// Retry partition discovery with backoff, since the kernel may take a moment to re-read the partition table after sgdisk writes.
fn retry_find_partition(max_attempts: u32) -> io::Result<Partition> {
   for attempt in 1..=max_attempts {
      if let Some(part) = find_partition() {
         return Ok(part);
      }
      if attempt < max_attempts {
         let delay = if attempt <= 2 { 1 } else { 2 };
         std::thread::sleep(std::time::Duration::from_secs(delay));
      }
   }
   Err(io::Error::new(io::ErrorKind::NotFound,
      "Cannot find ferros partition after creation. The disk may need a reboot to re-read the partition table."))
}

// ---------------------------------------------------------------------------
// Format — zero out all ring regions
// ---------------------------------------------------------------------------

fn cmd_format(part: &Partition) -> io::Result<()> {
   check_root()?;

   println!();
   println!("  Formatting ferros partition: {}", part.device);
   println!();

   let _ = run("diskutil", &["unmount", &part.device]);

   let mut f = OpenOptions::new()
      .read(true)
      .write(true)
      .open(&part.raw_device)?;

   let zero_block = [0u8; BLOCK_SIZE];

   let regions: &[(&str, u32, u32)] = &[
      ("Reserved",  0,                  0x400),
      ("Seed A",    SEED_A_BLOCK,       4),
      ("Seed B",    SEED_B_BLOCK,       4),
      ("Stem",      STEM_BASE,          STEM_SIZE),
      ("Kernel A",  KERNEL_A_BASE,      KERNEL_MAX_BLOCKS),
      ("Kernel B",  KERNEL_B_BASE,      KERNEL_MAX_BLOCKS),
      ("Spine",     SPINE_BASE,         SPINE_SIZE),
      ("State",     STATE_RING_BASE,    STATE_RING_SIZE),
      ("Ledger",    LEDGER_RING_BASE,   LEDGER_RING_SIZE),
   ];

   // Validate that all regions fit within the partition
   for (name, base, count) in regions {
      let end_byte = block_to_bytes(*base + *count);
      if end_byte > part.size_bytes {
         return Err(io::Error::new(io::ErrorKind::Other,
            format!("{} region (ends at {}) exceeds partition size ({})",
               name, fmt_size(end_byte), fmt_size(part.size_bytes))));
      }
   }

   for (name, base, count) in regions {
      let byte_offset = block_to_bytes(*base);
      let total_bytes = *count as u64 * BLOCK_SIZE as u64;

      print!("  Zeroing {:<12} {} blocks at {}...", name, count, fmt_hex(*base as u64));
      io::stdout().flush()?;

      f.seek(SeekFrom::Start(byte_offset))?;

      let chunk_blocks = 256u32; // 1 MB at a time
      let mut remaining = *count;
      while remaining > 0 {
         let batch = remaining.min(chunk_blocks);
         for _ in 0..batch {
            f.write_all(&zero_block)?;
         }
         remaining -= batch;
      }
      f.flush()?;

      println!(" {} zeroed", fmt_size(total_bytes));
   }

   println!();
   println!("  Format complete. All rings zeroed.");
   println!();
   Ok(())
}

// ---------------------------------------------------------------------------
// Genesis — write the initial spine entry
// ---------------------------------------------------------------------------

fn ewe_encode_u64(val: u64) -> (u8, Vec<u8>) {
   if val <= 0xFF {
      (b'3', vec![val as u8])
   } else if val <= 0xFFFF {
      (b'4', (val as u16).to_be_bytes().to_vec())
   } else if val <= 0xFFFF_FFFF {
      (b'5', (val as u32).to_be_bytes().to_vec())
   } else {
      (b'6', val.to_be_bytes().to_vec())
   }
}

/// Decode a u64 from EWE at the given position. Returns (value, bytes_consumed).
fn ewe_decode_u64(buf: &[u8], pos: usize) -> Option<(u64, usize)> {
   if pos + 2 > buf.len() || buf[pos] != b'u' {
      return None;
   }
   match buf[pos + 1] {
      b'3' if pos + 3 <= buf.len() => Some((buf[pos + 2] as u64, 3)),
      b'4' if pos + 4 <= buf.len() => {
         let v = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]);
         Some((v as u64, 4))
      }
      b'5' if pos + 6 <= buf.len() => {
         let v = u32::from_be_bytes(buf[pos + 2..pos + 6].try_into().ok()?);
         Some((v as u64, 6))
      }
      b'6' if pos + 10 <= buf.len() => {
         let v = u64::from_be_bytes(buf[pos + 2..pos + 10].try_into().ok()?);
         Some((v as u64, 10))
      }
      _ => None,
   }
}

/// Build a genesis spine entry as a VSF document in a 4KB block.
fn build_genesis_entry() -> [u8; BLOCK_SIZE] {
   let mut block = [0u8; BLOCK_SIZE];

   let hash_prefix_len = 34; // 'h' 'p' [32 bytes]
   let mut pos = hash_prefix_len;

   // gen(u{0})
   block[pos] = b'u';
   pos += 1;
   let (width_tag, payload) = ewe_encode_u64(0);
   block[pos] = width_tag;
   pos += 1;
   block[pos..pos + payload.len()].copy_from_slice(&payload);
   pos += payload.len();

   // prev_hash(hp{BLAKE3([0;32])})
   let genesis_prev = blake3::hash(&[0u8; 32]);
   block[pos] = b'h';
   pos += 1;
   block[pos] = b'p';
   pos += 1;
   block[pos..pos + 32].copy_from_slice(genesis_prev.as_bytes());
   pos += 32;

   // plow(u{TRACT_BASE})
   block[pos] = b'u';
   pos += 1;
   let (width_tag, payload) = ewe_encode_u64(TRACT_BASE as u64);
   block[pos] = width_tag;
   pos += 1;
   block[pos..pos + payload.len()].copy_from_slice(&payload);
   pos += payload.len();

   // ledger_head(hp{BLAKE3([0;32])})
   let genesis_ledger = blake3::hash(&[0u8; 32]);
   block[pos] = b'h';
   pos += 1;
   block[pos] = b'p';
   pos += 1;
   block[pos..pos + 32].copy_from_slice(genesis_ledger.as_bytes());
   pos += 32;

   // Provenance hash over inner document
   let entry_hash = blake3::hash(&block[hash_prefix_len..pos]);
   block[0] = b'h';
   block[1] = b'p';
   block[2..34].copy_from_slice(entry_hash.as_bytes());

   block
}

fn cmd_genesis(part: &Partition) -> io::Result<()> {
   check_root()?;

   println!();
   println!("  Writing genesis spine entry...");

   let _ = run("diskutil", &["unmount", &part.device]);

   let mut f = OpenOptions::new()
      .read(true)
      .write(true)
      .open(&part.raw_device)?;

   let entry = build_genesis_entry();
   let entry_hash = blake3::hash(&entry[34..]);

   let offset = block_to_bytes(SPINE_BASE);
   f.seek(SeekFrom::Start(offset))?;
   f.write_all(&entry)?;
   f.flush()?;

   // Write-verify
   f.seek(SeekFrom::Start(offset))?;
   let mut readback = [0u8; BLOCK_SIZE];
   f.read_exact(&mut readback)?;

   if readback != entry {
      eprintln!("  Write-verify: FAILED — read-back does not match written data.");
      eprintln!("  The disk may have bad sectors or the write was not flushed.");
      return Err(io::Error::new(io::ErrorKind::Other, "write-verify failed"));
   }
   println!("  Write-verify: OK");

   println!();
   println!("  Genesis spine entry written at block {}", fmt_hex(SPINE_BASE as u64));
   println!("  Generation:   0");
   println!("  Plow:         {} (tract start)", fmt_hex(TRACT_BASE as u64));
   println!("  Entry hash:   {}", hex::encode(entry_hash.as_bytes()));
   println!("  Prev hash:    {} (genesis)", hex::encode(blake3::hash(&[0u8; 32]).as_bytes()));
   println!();
   Ok(())
}

// ---------------------------------------------------------------------------
// Verify — scan the spine and decode entries
// ---------------------------------------------------------------------------

fn cmd_verify(part: &Partition) -> io::Result<()> {
   println!();
   println!("  Scanning spine on {}...", part.device);
   println!();

   let _ = run("diskutil", &["unmount", &part.device]);

   let mut f = File::open(&part.raw_device)?;
   let base = block_to_bytes(SPINE_BASE);
   let mut valid_count = 0u32;
   let mut highest_gen: Option<u64> = None;

   // Binary search would be O(16) reads; for verify we scan linearly but only the first 256 slots (and last 256) to bound I/O.
   let scan_ranges: &[(u32, u32)] = &[(0, 256), (SPINE_SIZE - 256, SPINE_SIZE)];

   for &(start, end) in scan_ranges {
      for i in start..end {
         f.seek(SeekFrom::Start(base + i as u64 * BLOCK_SIZE as u64))?;
         let mut block = [0u8; BLOCK_SIZE];
         f.read_exact(&mut block)?;

         if block[0] != b'h' || block[1] != b'p' {
            if !block.iter().all(|&b| b == 0) {
               println!("  Slot {:>5}: unknown data (G#{:02X} G#{:02X})", i, block[0], block[1]);
            }
            continue;
         }

         // Verify provenance hash
         let stored_hash = &block[2..34];
         let inner_start = 34;

         // Find the end of meaningful data (scan for trailing zeros)
         let mut inner_end = BLOCK_SIZE;
         while inner_end > inner_start && block[inner_end - 1] == 0 {
            inner_end -= 1;
         }

         let computed = blake3::hash(&block[inner_start..inner_end]);
         let hash_valid = stored_hash == computed.as_bytes();

         // Also try hashing the full remainder (as build_genesis_entry does)
         let computed_full = blake3::hash(&block[inner_start..]);
         let hash_valid_full = stored_hash == computed_full.as_bytes();

         let valid = hash_valid || hash_valid_full;

         // Decode generation from the inner document
         let generation = ewe_decode_u64(&block, inner_start).map(|(v, _)| v);

         // Decode plow position (skip gen + prev_hash to find it)
         let plow = decode_plow(&block, inner_start);

         valid_count += 1;
         if let Some(g) = generation {
            highest_gen = Some(highest_gen.map_or(g, |h: u64| h.max(g)));
         }

         print!("  Slot {:>5}: ", i);
         if valid {
            print!("VALID");
         } else {
            print!("HASH MISMATCH");
         }
         if let Some(g) = generation {
            print!("  gen={}", g);
         }
         if let Some(p) = plow {
            print!("  plow={}", fmt_hex(p));
         }
         println!();
      }
   }

   println!();
   if valid_count == 0 {
      println!("  Spine is empty — partition needs genesis.");
   } else {
      println!("  Found {} entries.", valid_count);
      if let Some(g) = highest_gen {
         println!("  Highest generation: {}", g);
      }
   }
   println!();
   Ok(())
}

/// Decode the plow field from a spine entry's inner document. Layout: gen(u{...}) prev_hash(hp{32}) plow(u{...})
fn decode_plow(block: &[u8], inner_start: usize) -> Option<u64> {
   let mut pos = inner_start;

   // Skip generation: u + width_tag + payload
   let (_, gen_len) = ewe_decode_u64(block, pos)?;
   pos += gen_len;

   // Skip prev_hash: 'h' 'p' [32 bytes] = 34
   if pos + 34 > block.len() || block[pos] != b'h' || block[pos + 1] != b'p' {
      return None;
   }
   pos += 34;

   // Decode plow
   let (val, _) = ewe_decode_u64(block, pos)?;
   Some(val)
}

// ---------------------------------------------------------------------------
// Kernel — parse .signed package, write kernel + stem entry
// ---------------------------------------------------------------------------

const FERROSIG_MAGIC: &[u8; 8] = b"FERROSIG";

struct SignedKernel {
   flat: Vec<u8>,
   hash: [u8; 32],
   sig: [u8; 64],
}

fn parse_signed_kernel(path: &str) -> io::Result<SignedKernel> {
   let data = std::fs::read(path)?;

   // Trailer: [size:4 LE][hash:32][sig:64][FERROSIG:8] = 108 bytes
   if data.len() < 108 {
      return Err(io::Error::new(io::ErrorKind::InvalidData, "File too small for .signed format"));
   }
   let trailer_start = data.len() - 108;
   let magic = &data[data.len() - 8..];
   if magic != FERROSIG_MAGIC {
      return Err(io::Error::new(io::ErrorKind::InvalidData,
         "Missing FERROSIG magic — is this a signed kernel? (use ferros-mkimg sign-kernel)"));
   }

   let size = u32::from_le_bytes(data[trailer_start..trailer_start + 4].try_into().unwrap()) as usize;
   let mut hash = [0u8; 32];
   hash.copy_from_slice(&data[trailer_start + 4..trailer_start + 36]);
   let mut sig = [0u8; 64];
   sig.copy_from_slice(&data[trailer_start + 36..trailer_start + 100]);

   if size > trailer_start {
      return Err(io::Error::new(io::ErrorKind::InvalidData,
         format!("Kernel size in trailer ({}) exceeds file data ({})", size, trailer_start)));
   }

   let flat = data[..size].to_vec();

   // Verify hash matches
   let computed = blake3::hash(&flat);
   if computed.as_bytes() != &hash {
      return Err(io::Error::new(io::ErrorKind::InvalidData,
         "BLAKE3 hash mismatch — signed kernel is corrupt"));
   }

   Ok(SignedKernel { flat, hash, sig })
}

/// Build a stem (kernel ring) entry as a proper VSF document. Must match the format that ferros_seed's parse_kernel_entry() expects: RÅ< z(0) y(0) b(hlen) eu(0) hp(hash) n(5) > [ (generation:u(N)) (kernel_lba:u(N)) (kernel_size:u(N)) (kernel_hash:hp(32)) (kernel_sig:ge(64)) ]
fn build_stem_entry(
   generation: u64,
   kernel_lba: u32,
   kernel_size: u32,
   kernel_hash: &[u8; 32],
   kernel_sig: &[u8; 64],
) -> [u8; BLOCK_SIZE] {
   let mut block = [0u8; BLOCK_SIZE];
   let mut pos = 0;

   // --- VSF magic ---
   let magic: [u8; 4] = [0x52, 0xC3, 0x85, 0x3C]; // RÅ<
   block[pos..pos + 4].copy_from_slice(&magic);
   pos += 4;

   // z(0) — version
   block[pos] = b'z'; pos += 1;
   block[pos] = b'3'; pos += 1; block[pos] = 0; pos += 1;

   // y(0) — backward compat version
   block[pos] = b'y'; pos += 1;
   block[pos] = b'3'; pos += 1; block[pos] = 0; pos += 1;

   // b(header_len) — placeholder, fill in after header
   let hlen_pos = pos;
   block[pos] = b'b'; pos += 1;
   block[pos] = b'3'; pos += 1; block[pos] = 0; pos += 1; // placeholder

   // eu(0) — eagle time (0 = no time source on host)
   block[pos] = b'e'; pos += 1;
   block[pos] = b'u'; pos += 1;
   block[pos] = b'3'; pos += 1; block[pos] = 0; pos += 1;

   // hp(hash) — provenance hash placeholder (fill after body)
   let _hp_pos = pos;
   block[pos] = b'h'; pos += 1;
   block[pos] = b'p'; pos += 1;
   block[pos] = b'3'; pos += 1; block[pos] = 31; pos += 1; // EWE(31) = len-1
   let hp_hash_pos = pos;
   pos += 32; // 32 zero bytes = placeholder

   // n(5) — field count
   block[pos] = b'n'; pos += 1;
   block[pos] = b'3'; pos += 1; block[pos] = 5; pos += 1;

   // > — close header
   block[pos] = 0x3E; pos += 1;

   // Fill in header length (from after magic to close, exclusive)
   let hlen = (pos - 4) as u8; // subtract magic length
   block[hlen_pos + 2] = hlen;

   // --- Body: [ fields ] ---
   block[pos] = b'['; pos += 1;

   // Helper: write a named field with u64 value
   fn write_uint_field(block: &mut [u8], pos: &mut usize, name: &str, val: u64) {
      block[*pos] = b'('; *pos += 1;
      // d(name)
      block[*pos] = b'd'; *pos += 1;
      let (wt, pl) = ewe_encode_u64(name.len() as u64);
      block[*pos] = wt; *pos += 1;
      block[*pos..*pos + pl.len()].copy_from_slice(&pl); *pos += pl.len();
      block[*pos..*pos + name.len()].copy_from_slice(name.as_bytes()); *pos += name.len();
      block[*pos] = b':'; *pos += 1;
      // u(val)
      block[*pos] = b'u'; *pos += 1;
      let (wt, pl) = ewe_encode_u64(val);
      block[*pos] = wt; *pos += 1;
      block[*pos..*pos + pl.len()].copy_from_slice(&pl); *pos += pl.len();
      block[*pos] = b')'; *pos += 1;
   }

   write_uint_field(&mut block, &mut pos, "generation", generation);
   write_uint_field(&mut block, &mut pos, "kernel_lba", kernel_lba as u64);
   write_uint_field(&mut block, &mut pos, "kernel_size", kernel_size as u64);

   // (kernel_hash: hp(32))
   block[pos] = b'('; pos += 1;
   block[pos] = b'd'; pos += 1;
   let (wt, pl) = ewe_encode_u64(11); // "kernel_hash".len()
   block[pos] = wt; pos += 1;
   block[pos..pos + pl.len()].copy_from_slice(&pl); pos += pl.len();
   block[pos..pos + 11].copy_from_slice(b"kernel_hash"); pos += 11;
   block[pos] = b':'; pos += 1;
   block[pos] = b'h'; pos += 1;
   block[pos] = b'p'; pos += 1;
   block[pos] = b'3'; pos += 1; block[pos] = 31; pos += 1; // EWE(31)
   block[pos..pos + 32].copy_from_slice(kernel_hash); pos += 32;
   block[pos] = b')'; pos += 1;

   // (kernel_sig: ge(64))
   block[pos] = b'('; pos += 1;
   block[pos] = b'd'; pos += 1;
   let (wt, pl) = ewe_encode_u64(10); // "kernel_sig".len()
   block[pos] = wt; pos += 1;
   block[pos..pos + pl.len()].copy_from_slice(&pl); pos += pl.len();
   block[pos..pos + 10].copy_from_slice(b"kernel_sig"); pos += 10;
   block[pos] = b':'; pos += 1;
   block[pos] = b'g'; pos += 1;
   block[pos] = b'e'; pos += 1;
   block[pos] = b'3'; pos += 1; block[pos] = 63; pos += 1; // EWE(63)
   block[pos..pos + 64].copy_from_slice(kernel_sig); pos += 64;
   block[pos] = b')'; pos += 1;

   block[pos] = b']'; pos += 1;

   // Compute provenance hash: BLAKE3 of entire block with hp hash zeroed
   let mut temp = block;
   for i in 0..32 { temp[hp_hash_pos + i] = 0; }
   let hash = blake3::hash(&temp);
   block[hp_hash_pos..hp_hash_pos + 32].copy_from_slice(hash.as_bytes());

   let _ = pos; // suppress unused warning
   block
}

fn cmd_kernel(part: &Partition, path: &str) -> io::Result<()> {
   check_root()?;

   println!();
   println!("  Installing kernel from: {}", path);

   let signed = parse_signed_kernel(path)?;
   let kernel_blocks = (signed.flat.len() as u32 + BLOCK_SIZE as u32 - 1) / BLOCK_SIZE as u32;

   if kernel_blocks > KERNEL_MAX_BLOCKS {
      return Err(io::Error::new(io::ErrorKind::Other,
         format!("Kernel too large: {} blocks (max {})", kernel_blocks, KERNEL_MAX_BLOCKS)));
   }

   println!("  Kernel size:  {} ({} blocks)", fmt_size(signed.flat.len() as u64), kernel_blocks);
   println!("  Kernel hash:  {}", hex::encode(&signed.hash));
   println!();

   let _ = run("diskutil", &["unmount", &part.device]);

   let mut f = OpenOptions::new()
      .read(true)
      .write(true)
      .open(&part.raw_device)?;

   // Write kernel to slot A
   let kernel_offset = block_to_bytes(KERNEL_A_BASE);
   println!("  Writing kernel to slot A (block {})...", fmt_hex(KERNEL_A_BASE as u64));
   f.seek(SeekFrom::Start(kernel_offset))?;
   f.write_all(&signed.flat)?;
   // Pad to block boundary
   let pad = (BLOCK_SIZE - (signed.flat.len() % BLOCK_SIZE)) % BLOCK_SIZE;
   if pad > 0 {
      f.write_all(&vec![0u8; pad])?;
   }
   f.flush()?;

   // Write-verify kernel
   f.seek(SeekFrom::Start(kernel_offset))?;
   let mut readback = vec![0u8; signed.flat.len()];
   f.read_exact(&mut readback)?;
   let readback_hash = blake3::hash(&readback);
   if readback_hash.as_bytes() != &signed.hash {
      return Err(io::Error::new(io::ErrorKind::Other, "Kernel write-verify failed"));
   }
   println!("  Write-verify: OK");

   // Write kernel to slot B (redundancy)
   let kernel_b_offset = block_to_bytes(KERNEL_B_BASE);
   println!("  Writing kernel to slot B (block {})...", fmt_hex(KERNEL_B_BASE as u64));
   f.seek(SeekFrom::Start(kernel_b_offset))?;
   f.write_all(&signed.flat)?;
   if pad > 0 {
      f.write_all(&vec![0u8; pad])?;
   }
   f.flush()?;

   // Build and write stem entry at position 0 (generation 1)
   let stem_entry = build_stem_entry(
      1,
      KERNEL_A_BASE,
      signed.flat.len() as u32,
      &signed.hash,
      &signed.sig,
   );

   let stem_offset = block_to_bytes(STEM_BASE);
   println!("  Writing stem entry at block {} (generation 1)...", fmt_hex(STEM_BASE as u64));
   f.seek(SeekFrom::Start(stem_offset))?;
   f.write_all(&stem_entry)?;
   f.flush()?;

   // Write-verify stem
   f.seek(SeekFrom::Start(stem_offset))?;
   let mut stem_readback = [0u8; BLOCK_SIZE];
   f.read_exact(&mut stem_readback)?;
   if stem_readback != stem_entry {
      return Err(io::Error::new(io::ErrorKind::Other, "Stem entry write-verify failed"));
   }
   println!("  Write-verify: OK");

   println!();
   println!("  Kernel installed.");
   println!("  Stem generation: 1");
   println!("  Kernel slot A:   block {} ({} blocks)", fmt_hex(KERNEL_A_BASE as u64), kernel_blocks);
   println!("  Kernel slot B:   block {} (mirror)", fmt_hex(KERNEL_B_BASE as u64));
   println!();

   Ok(())
}

// ---------------------------------------------------------------------------
// Seed — write seed image to both seed slots
// ---------------------------------------------------------------------------

fn cmd_seed(part: &Partition, path: &str) -> io::Result<()> {
   check_root()?;

   println!();
   println!("  Installing seed from: {}", path);

   let seed_data = std::fs::read(path)?;
   let seed_blocks = (seed_data.len() as u32 + BLOCK_SIZE as u32 - 1) / BLOCK_SIZE as u32;

   // Seed slots are 4 blocks each (16 KB) — check fit
   if seed_blocks > 4 {
      return Err(io::Error::new(io::ErrorKind::Other,
         format!("Seed too large: {} bytes ({} blocks, max 4 blocks = 16 KB)",
            seed_data.len(), seed_blocks)));
   }

   let seed_hash = blake3::hash(&seed_data);
   println!("  Seed size:    {} ({} blocks)", fmt_size(seed_data.len() as u64), seed_blocks);
   println!("  Seed hash:    {}", hex::encode(seed_hash.as_bytes()));
   println!();

   let _ = run("diskutil", &["unmount", &part.device]);

   let mut f = OpenOptions::new()
      .read(true)
      .write(true)
      .open(&part.raw_device)?;

   // Write seed to slot A
   let seed_a_offset = block_to_bytes(SEED_A_BLOCK);
   println!("  Writing seed to slot A (block {})...", fmt_hex(SEED_A_BLOCK as u64));
   f.seek(SeekFrom::Start(seed_a_offset))?;
   f.write_all(&seed_data)?;
   let pad = (BLOCK_SIZE - (seed_data.len() % BLOCK_SIZE)) % BLOCK_SIZE;
   if pad > 0 {
      f.write_all(&vec![0u8; pad])?;
   }
   f.flush()?;

   // Write-verify
   f.seek(SeekFrom::Start(seed_a_offset))?;
   let mut readback = vec![0u8; seed_data.len()];
   f.read_exact(&mut readback)?;
   if blake3::hash(&readback).as_bytes() != seed_hash.as_bytes() {
      return Err(io::Error::new(io::ErrorKind::Other, "Seed A write-verify failed"));
   }
   println!("  Write-verify: OK");

   // Write seed to slot B (redundancy)
   let seed_b_offset = block_to_bytes(SEED_B_BLOCK);
   println!("  Writing seed to slot B (block {})...", fmt_hex(SEED_B_BLOCK as u64));
   f.seek(SeekFrom::Start(seed_b_offset))?;
   f.write_all(&seed_data)?;
   if pad > 0 {
      f.write_all(&vec![0u8; pad])?;
   }
   f.flush()?;

   // Write-verify
   f.seek(SeekFrom::Start(seed_b_offset))?;
   f.read_exact(&mut readback)?;
   if blake3::hash(&readback).as_bytes() != seed_hash.as_bytes() {
      return Err(io::Error::new(io::ErrorKind::Other, "Seed B write-verify failed"));
   }
   println!("  Write-verify: OK");

   println!();
   println!("  Seed installed.");
   println!("  Slot A: block {} ({} blocks)", fmt_hex(SEED_A_BLOCK as u64), seed_blocks);
   println!("  Slot B: block {} (mirror)", fmt_hex(SEED_B_BLOCK as u64));
   println!();

   Ok(())
}

// ---------------------------------------------------------------------------
// Hex encoding
// ---------------------------------------------------------------------------

mod hex {
   const HEX: &[u8; 16] = b"0123456789abcdef";

   pub fn encode(bytes: &[u8]) -> String {
      let mut s = String::with_capacity(bytes.len() * 2);
      for &b in bytes {
         s.push(HEX[(b >> 4) as usize] as char);
         s.push(HEX[(b & 0xf) as usize] as char);
      }
      s
   }
}

// ---------------------------------------------------------------------------
// libc for euid check
// ---------------------------------------------------------------------------

mod libc {
   unsafe extern "C" {
      pub safe fn geteuid() -> u32;
   }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
   let args: Vec<String> = std::env::args().collect();
   let cmd = args.get(1).map(|s| s.as_str());

   println!();
   println!("  ╔═══════════════════════════════╗");
   println!("  ║    ferros-install  v{}       ║", env!("CARGO_PKG_VERSION"));
   println!("  ╚═══════════════════════════════╝");

   match cmd {
      None => {
         match find_partition() {
            Some(part) => {
               print_info(&part);
               println!("  Commands:");
               println!("    sudo ferros-install install           Full install (resize + create + format + genesis)");
               println!("    sudo ferros-install kernel <.signed>  Write signed kernel + stem entry");
               println!("    sudo ferros-install seed <image>      Write seed to both slots");
               println!("    sudo ferros-install format            Zero all ring regions");
               println!("    sudo ferros-install genesis           Write genesis spine entry");
               println!("    sudo ferros-install verify            Scan spine for valid entries");
               println!("    sudo ferros-install all               Format + genesis (existing partition)");
               println!("         ferros-install info              Show partition layout");
               println!();
            }
            None => {
               if let Ok(disk) = discover_disk() {
                  print_disk_layout(&disk);
               }
               println!("  No ferros partition found.");
               println!();
               println!("  Run 'sudo ferros-install install' to create one.");
               println!();
            }
         }
      }
      Some("install") => {
         cmd_install().unwrap_or_else(|e| {
            eprintln!();
            eprintln!("  Install failed: {}", e);
            std::process::exit(1);
         });
      }
      Some("info") | Some("find") => {
         let part = find_partition().expect("No ferros partition found");
         print_info(&part);
      }
      Some("format") => {
         let part = find_partition().expect("No ferros partition found");
         print_info(&part);
         cmd_format(&part).expect("Format failed");
      }
      Some("genesis") => {
         let part = find_partition().expect("No ferros partition found");
         cmd_genesis(&part).expect("Genesis write failed");
      }
      Some("kernel") => {
         let path = args.get(2).expect("Usage: ferros-install kernel <path.signed>");
         let part = find_partition().expect("No ferros partition found");
         cmd_kernel(&part, path).expect("Kernel install failed");
      }
      Some("seed") => {
         let path = args.get(2).expect("Usage: ferros-install seed <path>");
         let part = find_partition().expect("No ferros partition found");
         cmd_seed(&part, path).expect("Seed install failed");
      }
      Some("verify") => {
         let part = find_partition().expect("No ferros partition found");
         cmd_verify(&part).expect("Verify failed");
      }
      Some("all") => {
         let part = find_partition().expect("No ferros partition found");
         print_info(&part);
         cmd_format(&part).expect("Format failed");
         cmd_genesis(&part).expect("Genesis write failed");
         println!("  Partition ready. ferros can boot from spine generation 0.");
         println!();
      }
      Some(other) => {
         eprintln!("  Unknown command: {}", other);
         eprintln!("  Usage: ferros-install [install|info|format|genesis|verify|all]");
         std::process::exit(1);
      }
   }
}
