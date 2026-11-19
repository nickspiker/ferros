//! The broadcast ephemerides for the chip (docs/GPS.md "Broadcast ephemerides in the windows"): a RINEX 3 navigation file's sets as `gps.eph` (`eph`), and a replay's windows rebuilt with them (`brdc`), both thru ferros_gnss, the GPS driver's own path.

use ferros_gnss::windows::{self, Set, GLONASS, GPS};

/// The sets a RINEX 3 navigation file gives the chip: each GPS satellite's latest healthy LNAV set, and each GLONASS slot's healthy broadcast vectors carried and fitted with IS-GPS-200's ephemeris over toe ± 1 hour (toe the latest vector's epoch to 16 s; the clock -τn + γn (toe - tb), drift γn).
pub fn sets_from_rinex(text: &[u8]) -> Vec<Set> {
    let mut out: Vec<Set> = Vec::new();
    ferros_gnss::eph::each_gps(text, |e| {
        if e.health != 0 {
            return;
        }
        match out.iter_mut().find(|s| s.system == GPS && s.number == e.prn) {
            Some(s) if e.week as f64 * 604_800. + e.toe > s.eph.week as f64 * 604_800. + s.eph.toe => s.eph = e,
            Some(_) => {}
            None => out.push(Set { system: GPS, number: e.prn, eph: e }),
        }
    });
    let mut glo = Vec::new();
    ferros_gnss::glo::each_glo(text, |e| {
        if e.health == 0 {
            glo.push(e)
        }
    });
    for slot in 1..=32u8 {
        let recs: Vec<_> = glo.iter().copied().filter(|e| e.slot == slot).collect();
        let Some(last) = recs.iter().copied().max_by(|a, b| (a.week as f64 * 604_800. + a.tb).partial_cmp(&(b.week as f64 * 604_800. + b.tb)).unwrap()) else { continue };
        let recs: Vec<_> = recs.into_iter().filter(|e| e.week == last.week).collect();
        let toe = (last.tb / 16.).round() * 16.;
        let (mut e, rms) = ferros_gnss::glo::fit(&|t| ferros_gnss::glo::orbit_at(&recs, t).unwrap(), last.week, toe, 3600.);
        e.prn = slot;
        e.af0 = last.tau + last.gamma * (toe - last.tb);
        e.af1 = last.gamma;
        eprintln!("R{slot:02}: fitted to A#{} broadcast vectors, RMS {rms:.1} m", recs.len());
        out.push(Set { system: GLONASS, number: slot, eph: e });
    }
    out
}

/// `gps-trace eph <rinex> <out.vsf>`: the sets as `gps.eph` (ferros_kept `eph`: a VSF document, fetched now on the Eagle clock), for `ferros-bridge store put gps.eph @<out.vsf>` until the device fetches its own.
pub fn eph(args: &[String]) {
    let text = std::fs::read(&args[2]).unwrap_or_else(|e| panic!("{}: {e}", args[2]));
    let sets = sets_from_rinex(&text);
    let now = vsf::eagle_time_oscillations();
    let doc = ferros_kept::eph::build(&sets, now, now).expect("the document did not build");
    std::fs::write(&args[3], &doc).unwrap_or_else(|e| panic!("{}: {e}", args[3]));
    let n = |s: u8| sets.iter().filter(|x| x.system == s).count();
    println!("{}: A#{} GPS and A#{} GLONASS sets, A#{} bytes", args[3], n(GPS), n(GLONASS), doc.len());
}

/// `gps-trace brdc <in.gnr> <rinex> <out.gnr>`: the replay's windows rebuilt with the RINEX file's sets (`windows::rebuild`); each window record gpsd sent is first compared with the set that replaces it, at the record's toe, as a distance (never a place).
pub fn brdc(args: &[String]) {
    let (inp, rinex, out) = (&args[2], &args[3], &args[4]);
    let b = std::fs::read(inp).unwrap_or_else(|e| panic!("{inp}: {e}"));
    assert!(b.starts_with(b"GNR1"));
    let mut recs: Vec<(u16, Vec<u8>)> = Vec::new();
    let mut at = 4;
    while at + 4 <= b.len() {
        let len = u16::from_le_bytes([b[at], b[at + 1]]) as usize;
        recs.push((u16::from_le_bytes([b[at + 2], b[at + 3]]), b[at + 4..at + 4 + len].to_vec()));
        at += 4 + len;
    }
    let first = recs.iter().position(|(_, r)| windows::has_window(r)).expect("no 82 81 in the replay");
    let sets = sets_from_rinex(&std::fs::read(rinex).unwrap_or_else(|e| panic!("{rinex}: {e}")));
    // gpsd's windows against the sets: rebuilt with nothing (the same messages), read back, compared.
    let (orig, _) = windows::rebuild(&recs[first - 1].1, &recs[first..], &[]).expect("no whole window");
    let mut stream = Vec::new();
    for (_, r) in &orig {
        for f in windows::frames(r) {
            let p = &f[windows::payload_start(&f)..f.len() - 1];
            stream.extend_from_slice(&p[if p[1] & 0x80 != 0 { 5 } else { 4 }..]);
        }
    }
    for (m, done) in crate::inner(&stream) {
        if !done || m.len() < 0xBC || m[..2] != [0x82, 0x81] {
            continue;
        }
        let w = |at: usize| u32::from_le_bytes([m[at], m[at + 1], m[at + 2], m[at + 3]]);
        let system = if w(0x4C) != 0 { GPS } else { GLONASS };
        for r in 0..w(0x68) as usize {
            let old = windows::decode(&m[0xBC + 0xA8 * r..0xBC + 0xA8 * (r + 1)]);
            if let Some(s) = sets.iter().find(|s| s.system == system && s.number == old.prn) {
                let (a, b) = (ferros_gnss::eph::gps_ecef(&old, old.toe), ferros_gnss::eph::gps_ecef(&s.eph, old.toe));
                println!("{}{:02}: gpsd's window against the broadcast set at its toe: {:.0} m", if system == GPS { 'G' } else { 'R' }, old.prn, ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt());
            }
        }
    }
    let (new, did) = windows::rebuild(&recs[first - 1].1, &recs[first..], &sets).expect("no whole window");
    let mut o = b"GNR1".to_vec();
    for (g, r) in recs[..first].iter().chain(new.iter()) {
        o.extend_from_slice(&(r.len() as u16).to_le_bytes());
        o.extend_from_slice(&g.to_le_bytes());
        o.extend_from_slice(r);
    }
    std::fs::write(out, &o).unwrap_or_else(|e| panic!("{out}: {e}"));
    println!("{out}: A#{} window messages, A#{} GPS and A#{} GLONASS records from the broadcast ephemerides, A#{} kept as they were; A#{} records, A#{} bytes", did.messages, did.gps, did.glonass, did.kept, first + new.len(), o.len());
}
