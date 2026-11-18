//! `part`: Android's partitions read off the device block for block, for the per-unit backups (persist, efs, efs_backup, modem_userdata, devinfo, the GPTs: keys/<device>/, never in git), read-only.
//! Every block comes thru the kernel's development `diag lun <lu> <lba> <count>` (service.rs: up to 8 blocks a call, a hex dump of their non-zero 16-byte rows, left out of production builds), so nothing on the device changes and no kernel flash is needed.
//! The GPT is read and walked here on the host from the same raw blocks, and a dump covers exactly the partition's GPT extent.
//! The output files hold the device's identity (IMEIs, serials): this prints sizes and places only, never the bytes.

use crate::usb::UsbLink;
use std::io::Write;

/// A UFS logical block (the LUs on husky and akita are 4 KiB-block LUs; the kernel's dump is of its 4 KiB data buffer).
const BLOCK: usize = 1 << 12;
/// Blocks per `diag lun` call: the kernel's own most.
const PER_CALL: u64 = 1 << 3;
/// LUs the kernel's lookup walks (partfs.rs find): 0 to 7.
const LUS: u8 = 1 << 3;
/// GPT header fields (UEFI spec 5.3.2).
const H_ALTERNATE: usize = 0x20;
const H_FIRST_USABLE: usize = 0x28;
const H_LAST_USABLE: usize = 0x30;
const H_ENTRY_LBA: usize = 0x48;
const H_ENTRY_COUNT: usize = 0x50;
const H_ENTRY_SIZE: usize = 0x54;

/// One partition as the LU's primary GPT has it.
struct Entry {
    lu: u8,
    name: String,
    first: u64,
    last: u64,
}

/// One LU's GPT header, the fields a dump needs.
struct Header {
    alternate: u64,
    first_usable: u64,
    last_usable: u64,
    entry_lba: u64,
    count: u64,
    esize: u64,
}

fn le64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

fn le32(b: &[u8], at: usize) -> u64 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as u64
}

async fn diag(link: &UsbLink, params: &str) -> Result<Vec<u8>, String> {
    let cmd = crate::build_cmd(ferros_pt::command::caps::DIAG, ferros_pt::Op::Read, params.as_bytes());
    let c = crate::pt_send(link, &cmd).await?;
    if !c.success {
        return Err(format!("the device refused `diag {params}`"));
    }
    crate::pt_recv(link).await
}

fn hexval(c: u8) -> Result<u8, String> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(format!("not a hex digit in the dump: G#{c:X}")),
    }
}

/// Blocks `first..first + count` of LU `lu` (count at most PER_CALL), from the kernel's hex dump.
/// The dump leaves out all-zero rows, so the buffer starts zeroed; every row the dump has is placed at its offset.
/// Anything not in the dump's exact shape (an older kernel answering with its boot log, an unreadable block, a block missing) is an error, never a guess.
async fn blocks(link: &UsbLink, lu: u8, first: u64, count: u64) -> Result<Vec<u8>, String> {
    let rep = diag(link, &format!("lun {lu} {first} {count}")).await?;
    let mut out = vec![0u8; count as usize * BLOCK];
    let mut seen = 0u64;
    for line in rep.split(|&c| c == b'\n') {
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix(b"LBA ") {
            let s = std::str::from_utf8(rest).map_err(|_| String::from("the dump's LBA line is not text"))?;
            let lba: u64 = s.split(' ').next().unwrap_or("").parse().map_err(|_| format!("the dump's LBA line has no number: `diag lun` may be missing on this kernel"))?;
            if lba != first + seen {
                return Err(format!("the dump gave LBA A#{lba} where A#{} was due", first + seen));
            }
            if s.contains("unreadable") {
                return Err(format!("LU A#{lu} LBA A#{lba} unreadable (four tries on the device)"));
            }
            seen += 1;
            continue;
        }
        // A row: "  XXXX: hh hh … hh" (16 bytes), under the last LBA line.
        // Checked because it is the device's answer (external input): a short or foreign line would otherwise index past the row or the block.
        if seen == 0 || line.len() != 2 + 4 + 1 + 16 * 3 || &line[0..2] != b"  " || line[6] != b':' {
            return Err(format!("unexpected line in the dump ({} bytes): `diag lun` may be missing on this kernel", line.len()));
        }
        let mut off = 0usize;
        for &c in &line[2..6] {
            off = off << 4 | hexval(c)? as usize;
        }
        if off & 15 != 0 || off >= BLOCK {
            return Err(format!("row offset G#{off:X} outside a block"));
        }
        let base = (seen as usize - 1) * BLOCK + off;
        for i in 0..16 {
            let p = 7 + i * 3;
            if line[p] != b' ' {
                return Err(String::from("malformed row in the dump"));
            }
            out[base + i] = hexval(line[p + 1])? << 4 | hexval(line[p + 2])?;
        }
    }
    if seen != count {
        return Err(format!("the dump had A#{seen} of A#{count} blocks"));
    }
    Ok(out)
}

/// Blocks `first..=last` of LU `lu` into `out`, PER_CALL a call, the progress on stderr.
async fn range(link: &UsbLink, lu: u8, first: u64, last: u64, out: &mut impl Write) -> Result<u64, String> {
    let total = last + 1 - first;
    let mut lba = first;
    let mut next_say = 0u64;
    let t0 = std::time::Instant::now();
    while lba <= last {
        let n = PER_CALL.min(last + 1 - lba);
        let b = blocks(link, lu, lba, n).await?;
        out.write_all(&b).map_err(|e| format!("write: {e}"))?;
        lba += n;
        let done = lba - first;
        if done >= next_say || lba > last {
            eprint!("\r  A#{done} of A#{total} blocks, A#{} s ", t0.elapsed().as_secs());
            next_say = done + (1 << 11);
        }
    }
    eprintln!();
    Ok(total * BLOCK as u64)
}

/// LU `lu`'s primary GPT header, or None when the LU has none (not enabled, unreadable, no "EFI PART").
async fn header(link: &UsbLink, lu: u8) -> Result<Option<Header>, String> {
    let h = match blocks(link, lu, 1, 1).await {
        Ok(h) => h,
        Err(e) if e.contains("unreadable") => return Ok(None),
        Err(e) => return Err(e),
    };
    if &h[0..8] != b"EFI PART" {
        return Ok(None);
    }
    Ok(Some(Header {
        alternate: le64(&h, H_ALTERNATE),
        first_usable: le64(&h, H_FIRST_USABLE),
        last_usable: le64(&h, H_LAST_USABLE),
        entry_lba: le64(&h, H_ENTRY_LBA),
        count: le32(&h, H_ENTRY_COUNT),
        esize: le32(&h, H_ENTRY_SIZE),
    }))
}

/// Every partition on LUs 0-7, from each primary GPT's entry array.
async fn entries(link: &UsbLink) -> Result<Vec<Entry>, String> {
    let mut all = Vec::new();
    for lu in 0..LUS {
        let Some(h) = header(link, lu).await? else { continue };
        // Checked because the header is read off the flash: an entry size the array does not tile 4 KiB blocks with would misplace every name after the first block.
        if h.esize < 128 || BLOCK as u64 % h.esize != 0 {
            return Err(format!("LU A#{lu}: GPT entry size A#{} not taken", h.esize));
        }
        let nblocks = (h.count * h.esize).div_ceil(BLOCK as u64);
        let mut arr = Vec::new();
        range(link, lu, h.entry_lba, h.entry_lba + nblocks - 1, &mut arr).await?;
        for e in arr.chunks(h.esize as usize).take(h.count as usize) {
            if e[..16].iter().all(|&b| b == 0) {
                continue;
            }
            let name: String = e[56..128].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).filter_map(|c| char::from_u32(c as u32)).collect();
            all.push(Entry { lu, name, first: le64(e, 32), last: le64(e, 40) });
        }
    }
    Ok(all)
}

fn usage() -> ! {
    eprintln!("Usage: ferros-bridge part list | dump <name> <out> | gpt <lu> <primary-out> <backup-out>");
    eprintln!("  list    every partition on LUs 0-7: LU, first..=last LBA, KiB (the primary GPTs)");
    eprintln!("  dump    a partition's blocks, exactly its GPT extent, into <out> (read-only; development kernels: `diag lun`)");
    eprintln!("  gpt     an LU's GPT: LBA 0 to the first usable LBA (protective MBR, header, entries) and the last usable LBA to the backup header");
    std::process::exit(1);
}

pub async fn cmd(args: &[String]) {
    let Some(sub) = args.first() else { usage() };
    let link = match UsbLink::open().await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let r = match (sub.as_str(), args.len()) {
        ("list", 1) => list(&link).await,
        ("dump", 3) => dump(&link, &args[1], &args[2]).await,
        ("gpt", 4) => match args[1].parse::<u8>() {
            Ok(lu) if lu < LUS => gpt(&link, lu, &args[2], &args[3]).await,
            _ => Err(format!("LU {} is not one of 0-7", args[1])),
        },
        _ => usage(),
    };
    if let Err(e) = r {
        eprintln!("part {sub}: {e}");
        std::process::exit(1);
    }
}

async fn list(link: &UsbLink) -> Result<(), String> {
    for e in entries(link).await? {
        println!("{} LU A#{} LBA A#{}..=A#{} A#{} KiB", e.name, e.lu, e.first, e.last, (e.last + 1 - e.first) * (BLOCK as u64 >> 10));
    }
    Ok(())
}

async fn dump(link: &UsbLink, name: &str, out: &str) -> Result<(), String> {
    let all = entries(link).await?;
    let found: Vec<&Entry> = all.iter().filter(|e| e.name == name).collect();
    let e = match found.as_slice() {
        [e] => *e,
        [] => return Err(format!("no partition named {name}")),
        _ => return Err(format!("{name} is on more than one LU: not guessing which")),
    };
    eprintln!("{name}: LU A#{} LBA A#{}..=A#{}", e.lu, e.first, e.last);
    let mut f = std::io::BufWriter::new(std::fs::File::create(out).map_err(|e| format!("{out}: {e}"))?);
    let n = range(link, e.lu, e.first, e.last, &mut f).await?;
    f.flush().map_err(|e| format!("{out}: {e}"))?;
    println!("{name} LU A#{} LBA A#{}..=A#{} A#{n} bytes", e.lu, e.first, e.last);
    Ok(())
}

async fn gpt(link: &UsbLink, lu: u8, primary: &str, backup: &str) -> Result<(), String> {
    let Some(h) = header(link, lu).await? else { return Err(format!("LU A#{lu} has no GPT")) };
    // Checked because the header is read off the flash: the backup region runs from past the last usable LBA to the backup header, and a header that says otherwise is not one to dump by.
    if h.first_usable == 0 || h.alternate <= h.last_usable {
        return Err(format!("LU A#{lu}: GPT header not taken (first usable A#{}, last usable A#{}, alternate A#{})", h.first_usable, h.last_usable, h.alternate));
    }
    let mut p = Vec::new();
    let pn = range(link, lu, 0, h.first_usable - 1, &mut p).await?;
    std::fs::write(primary, &p).map_err(|e| format!("{primary}: {e}"))?;
    let mut b = Vec::new();
    let bn = range(link, lu, h.last_usable + 1, h.alternate, &mut b).await?;
    // The backup header is the region's last block.
    if &b[b.len() - BLOCK..b.len() - BLOCK + 8] != b"EFI PART" {
        return Err(format!("LU A#{lu}: no backup GPT header at LBA A#{}", h.alternate));
    }
    std::fs::write(backup, &b).map_err(|e| format!("{backup}: {e}"))?;
    println!("LU A#{lu} GPT primary LBA A#0..=A#{} A#{pn} bytes, backup LBA A#{}..=A#{} A#{bn} bytes", h.first_usable - 1, h.last_usable + 1, h.alternate);
    Ok(())
}
