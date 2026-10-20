//! The cellular modem (Shannon S5300 over PCIe channel 0): what the cold boot needs, step by step (docs/oracle/MODEM-BOOT-RECON.md).
//!
//! Step 1 here: the images. `modem.bin` in `modem_a` begins with a TOC of 32-byte entries {name[12], file offset, CP load address, size, CRC, id}; BOOT is the ROM stage, MAIN/VSS/APM go over the boot channel, and the three NV sections come from per-unit files (efs nv_normal.bin / nv_protected.bin, modem_userdata replay_region.bin).
//! Nothing here prints NV contents: they are per-unit.

use alloc::vec::Vec;

/// One TOC entry.
#[derive(Clone, Copy, Debug)]
pub struct Section {
    pub name: [u8; 12],
    pub offset: u32,
    pub load: u32,
    pub size: u32,
    pub crc: u32,
    pub id: u32,
}

impl Section {
    pub fn name(&self) -> &str {
        let n = self.name.iter().position(|&c| c == 0).unwrap_or(12);
        core::str::from_utf8(&self.name[..n]).unwrap_or("?")
    }
}

pub const MODEM_PART: &str = "modem_a";
pub const MODEM_BIN: &str = "/images/default/modem.bin";

/// Parse the TOC from the first block of modem.bin.
pub fn toc(first: &[u8]) -> Vec<Section> {
    let mut v = Vec::new();
    for e in first.chunks(32).take(32) {
        if e.len() < 32 || e[0] == 0 || !e[..12].iter().take_while(|&&c| c != 0).all(|&c| (32..127).contains(&c)) {
            break;
        }
        let w = |o: usize| u32::from_le_bytes([e[o], e[o + 1], e[o + 2], e[o + 3]]);
        let mut name = [0u8; 12];
        name.copy_from_slice(&e[..12]);
        v.push(Section { name, offset: w(12), load: w(16), size: w(20), crc: w(24), id: w(28) });
    }
    v
}

fn line(rep: &mut Vec<u8>, s: alloc::string::String) {
    rep.extend_from_slice(s.as_bytes());
}

/// `diag modem files`: find and list the modem's images on the flash, read only — modem.bin's TOC, and the sizes of the per-unit NV files.
pub fn files(rep: &mut Vec<u8>) {
    let t0 = crate::platform::cntpct();
    match crate::partfs::mount(MODEM_PART) {
        Err(e) => line(rep, alloc::format!("{e}\n")),
        Ok(mut m) => match m.open(MODEM_BIN) {
            Err(e) => line(rep, alloc::format!("{MODEM_PART}: {e}\n")),
            Ok(ino) => {
                line(rep, alloc::format!("{MODEM_PART}{MODEM_BIN}: A#{} bytes\n", ino.size));
                let mut first = alloc::vec![0u8; 4096];
                match m.read(&ino, 0, &mut first) {
                    Err(e) => line(rep, alloc::format!("{e}\n")),
                    Ok(_) => {
                        for s in toc(&first) {
                            line(rep, alloc::format!("  {:<8} offset G#{:X} load G#{:X} size G#{:X} crc G#{:X} id A#{}\n", s.name(), s.offset, s.load, s.size, s.crc, s.id));
                        }
                    }
                }
            }
        },
    }
    match crate::partfs::mount("efs") {
        Err(e) => line(rep, alloc::format!("{e}\n")),
        Ok(mut m) => {
            for f in ["/nv_normal.bin", "/nv_protected.bin"] {
                match m.open(f) {
                    Ok(i) => line(rep, alloc::format!("efs{f}: A#{} bytes\n", i.size)),
                    Err(e) => line(rep, alloc::format!("efs: {e}\n")),
                }
            }
        }
    }
    match crate::partfs::find("modem_userdata") {
        Some(p) => line(rep, alloc::format!("modem_userdata: LU A#{} LBA A#{}..A#{} (f2fs, not read: replay_region.bin)\n", p.lun, p.first, p.last)),
        None => line(rep, alloc::format!("modem_userdata not found\n")),
    }
    let ms = (crate::platform::cntpct() - t0) / (crate::platform::cntfrq() / 1000);
    line(rep, alloc::format!("A#{ms} ms\n"));
}

/// `diag modem readmain`: read all of MAIN out of modem.bin through ext4 and hash it (BLAKE3), timing it: the read path the boot download will use. Compare the hash with the host's over the same slice of modem.bin (the TOC's CRC is not plain CRC-32, and the boot channel only forwards it, so it is not recomputed).
pub fn read_main(rep: &mut Vec<u8>) {
    let mut m = match crate::partfs::mount(MODEM_PART) {
        Ok(m) => m,
        Err(e) => return line(rep, alloc::format!("{e}\n")),
    };
    let ino = match m.open(MODEM_BIN) {
        Ok(i) => i,
        Err(e) => return line(rep, alloc::format!("{e}\n")),
    };
    let mut first = alloc::vec![0u8; 4096];
    if let Err(e) = m.read(&ino, 0, &mut first) {
        return line(rep, alloc::format!("{e}\n"));
    }
    let Some(main) = toc(&first).into_iter().find(|s| s.name() == "MAIN") else {
        return line(rep, alloc::format!("no MAIN in the TOC\n"));
    };
    // MAIN starts mid-block: read whole blocks from the block holding its start and hash the slice that is MAIN.
    let t0 = crate::platform::cntpct();
    let start = main.offset as u64;
    let end = start + main.size as u64;
    let mut h = blake3::Hasher::new();
    let mut buf = alloc::vec![0u8; 256 * 1024];
    let mut pos = start & !4095;
    while pos < end {
        crate::platform::pet_watchdogs();
        let n = match m.read(&ino, pos, &mut buf) {
            Ok(n) if n > 0 => n,
            Ok(_) => break,
            Err(e) => return line(rep, alloc::format!("at G#{pos:X}: {e}\n")),
        };
        let lo = start.saturating_sub(pos) as usize;
        let hi = ((end - pos) as usize).min(n);
        if lo < hi {
            h.update(&buf[lo..hi]);
        }
        pos += n as u64;
    }
    let ms = (crate::platform::cntpct() - t0) / (crate::platform::cntfrq() / 1000);
    let d = h.finalize();
    let mut hx = alloc::string::String::new();
    for b in &d.as_bytes()[..16] {
        hx.push_str(&alloc::format!("{b:02x}"));
    }
    line(rep, alloc::format!("MAIN G#{:X} bytes from file offset G#{:X} read in A#{ms} ms, BLAKE3 {hx}...\n", main.size, main.offset));
}
