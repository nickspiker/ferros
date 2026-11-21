//! The driver's inputs from the vault, as the kernel's camera tools have always read them (camera.rs `replay_init_on`, `parse_prog`): the sensor's init table (`cam.boitata.init`), the CPU's setup writes around the PDMA start (`cam.ispfe.prog`, a 'CPR2'), and the bundle of both (`cam.frame`, a 'CFRM').
//! Each is generated from an oracle capture and carries the vendor's settings, so it lives in the vault and in local files, never in the repository.

/// One record of a sensor init table: `W` len (u16 BE) then the bytes of one I2C write; `R` count (u16 BE) and a two-byte register (a read, its value dropped); `D` microseconds (u32 BE).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rec<'a> {
    Write(&'a [u8]),
    Read { count: u16, reg: [u8; 2] },
    Delay(u32),
}

/// The records of an init table, in order; `Err(at)` at the first byte that starts no record (a truncated or corrupt table: the driver stops there rather than guess).
pub struct Records<'a> {
    t: &'a [u8],
    at: usize,
}

pub fn records(t: &[u8]) -> Records<'_> {
    Records { t, at: 0 }
}

impl<'a> Iterator for Records<'a> {
    type Item = Result<Rec<'a>, usize>;
    fn next(&mut self) -> Option<Self::Item> {
        let (t, i) = (self.t, self.at);
        if i >= t.len() {
            return None;
        }
        let r = match t[i] {
            b'W' if i + 3 <= t.len() => {
                let len = u16::from_be_bytes([t[i + 1], t[i + 2]]) as usize;
                if i + 3 + len > t.len() {
                    Err(i)
                } else {
                    self.at = i + 3 + len;
                    Ok(Rec::Write(&t[i + 3..i + 3 + len]))
                }
            }
            b'R' if i + 5 <= t.len() => {
                self.at = i + 5;
                Ok(Rec::Read { count: u16::from_be_bytes([t[i + 1], t[i + 2]]), reg: [t[i + 3], t[i + 4]] })
            }
            b'D' if i + 5 <= t.len() => {
                self.at = i + 5;
                Ok(Rec::Delay(u32::from_be_bytes([t[i + 1], t[i + 2], t[i + 3], t[i + 4]])))
            }
            _ => Err(i),
        };
        if r.is_err() {
            self.at = t.len();
        }
        Some(r)
    }
}

/// A 'CFRM' bundle split: (the init table, the 'CPR2' program). None when it is not one.
pub fn split_bundle(b: &[u8]) -> Option<(&[u8], &[u8])> {
    if b.len() < 8 || &b[..4] != b"CFRM" {
        return None;
    }
    let n = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
    (8 + n <= b.len()).then(|| (&b[8..8 + n], &b[8 + n..]))
}

/// (register, value) pairs, little-endian words, as a 'CPR2' section holds them.
#[derive(Clone, Copy)]
pub struct Pairs<'a>(&'a [u8]);

impl<'a> Pairs<'a> {
    pub fn len(&self) -> usize {
        self.0.len() / 8
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = (usize, u32)> + 'a {
        self.0.chunks_exact(8).map(|c| (u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as usize, u32::from_le_bytes([c[4], c[5], c[6], c[7]])))
    }
}

/// A 'CPR2' (tools/cam-oracle `cpu`): 'CPR2', a count of buffer addresses and the addresses (unused here: the PDMA job carries its own), then three sections of a count and that many (register, value) pairs: the CPU's writes before the PDMA start, the program (which the PDMA job carries instead), the CPU's writes after it.
pub struct Prog<'a> {
    pub before: Pairs<'a>,
    pub program: Pairs<'a>,
    pub after: Pairs<'a>,
}

pub fn prog(p: &[u8]) -> Option<Prog<'_>> {
    let w = |i: usize| p.get(i * 4..i * 4 + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    if p.get(..4)? != b"CPR2" {
        return None;
    }
    let mut at = 2 + w(1)?;
    let mut section = || -> Option<Pairs<'_>> {
        let n = w(at)?;
        let (s, e) = ((at + 1) * 4, (at + 1 + 2 * n) * 4);
        let bytes = p.get(s..e)?;
        at += 1 + 2 * n;
        Some(Pairs(bytes))
    };
    let before = section()?;
    let program = section()?;
    let after = section()?;
    Some(Prog { before, program, after })
}

/// Whether a CPU setup write may be applied by the driver: inside the CSI links and PHY or the front end, never the PDMA's page (the engine is the job's) nor the region only the PDMA reaches (G#1C36: the CPU's writes there go nowhere, docs/oracle/CAMERA.md "The CPU cannot stand in for the PDMA"). The same filter the kernel's `cam pdma` applied; anything else (the SYSMMUs and S2MPUs between the two ranges among them) is refused.
pub fn cpu_write_allowed(a: usize) -> bool {
    let link = (0x1C04_0000..0x1C11_0000).contains(&a);
    let fe = (0x1C30_0000..0x1C40_0000).contains(&a) && !(0x1C39_0000..0x1C39_1000).contains(&a) && !(0x1C36_0000..0x1C37_0000).contains(&a);
    link || fe
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_read_back() {
        let t = [b'W', 0, 3, 1, 2, 3, b'D', 0, 0, 1, 0, b'R', 0, 2, 0, 5];
        let v: Vec<_> = records(&t).collect();
        assert_eq!(v, vec![Ok(Rec::Write(&[1, 2, 3])), Ok(Rec::Delay(256)), Ok(Rec::Read { count: 2, reg: [0, 5] })]);
        let bad = [b'W', 0, 9, 1];
        assert_eq!(records(&bad).collect::<Vec<_>>(), vec![Err(0)]);
    }

    #[test]
    fn prog_reads_back() {
        let mut p = b"CPR2".to_vec();
        for w in [1u32, 0xA000_0000, 1, 0x1C32_0000, 5, 0, 2, 0x1C05_0000, 1, 0x1C30_0004, 2] {
            p.extend_from_slice(&w.to_le_bytes());
        }
        let g = prog(&p).unwrap();
        assert_eq!(g.before.iter().collect::<Vec<_>>(), vec![(0x1C32_0000, 5)]);
        assert!(g.program.is_empty());
        assert_eq!(g.after.iter().collect::<Vec<_>>(), vec![(0x1C05_0000, 1), (0x1C30_0004, 2)]);
    }

    #[test]
    fn filter_keeps_the_fences_out() {
        assert!(cpu_write_allowed(0x1C05_0004));
        assert!(cpu_write_allowed(0x1C10_1800));
        assert!(cpu_write_allowed(0x1C32_00A0));
        assert!(!cpu_write_allowed(0x1C14_0054));
        assert!(!cpu_write_allowed(0x1C11_8000));
        assert!(!cpu_write_allowed(0x1C39_0000));
        assert!(!cpu_write_allowed(0x1C36_95FC));
        assert!(!cpu_write_allowed(0x1C00_0800));
    }

    /// The local bundle, when it is on this machine: its init is the clean recapture (1347 writes, 2 reads; docs/oracle/CAMERA.md "Streaming works") and its program parses.
    #[test]
    fn local_bundle() {
        let Ok(b) = std::fs::read("/mnt/Harbor/Code/ferros/target/cam/cam.frame") else { return };
        let (init, p) = split_bundle(&b).expect("a CFRM");
        let (mut w, mut r) = (0, 0);
        for rec in records(init) {
            match rec.expect("the init parses") {
                Rec::Write(_) => w += 1,
                Rec::Read { .. } => r += 1,
                Rec::Delay(_) => {}
            }
        }
        println!("cam.frame: init {w} writes, {r} reads; program {} bytes", p.len());
        let g = prog(p).expect("a CPR2");
        println!("CPU setup: {} before, {} program, {} after", g.before.len(), g.program.len(), g.after.len());
        assert!(g.before.iter().chain(g.after.iter()).all(|(a, _)| (0x1C00_0000..0x1D00_0000).contains(&a)));
        // Every CPU write the October runs applied lies where the driver may write: none falls between the links and the front end (the SYSMMUs and S2MPUs).
        let refused: Vec<_> = g.before.iter().chain(g.after.iter()).filter(|&(a, _)| (0x1C04_0000..0x1C40_0000).contains(&a) && !cpu_write_allowed(a) && !(0x1C39_0000..0x1C39_1000).contains(&a) && !(0x1C36_0000..0x1C37_0000).contains(&a)).collect();
        assert!(refused.is_empty(), "{refused:X?}");
        let outside = g.before.iter().chain(g.after.iter()).filter(|&(a, _)| !(0x1C04_0000..0x1C40_0000).contains(&a)).count();
        println!("CPU setup writes outside the links and the front end (the harness skipped them too): {outside}");
    }
}
