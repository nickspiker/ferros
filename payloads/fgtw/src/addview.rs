//! The add flow as the Fleet page reads it (op 9) and as the keyboard's host reads a stroke's answer (op 11), built from fgtw's shared machine's view (`fgtw::flow::add::View`).
//! Pure: no syscall, no clock, so the mapping is tested on the host (the tests below).

use alloc::vec::Vec;
use fgtw::flow::add::{Phase, Row, View};
use fgtw::sponsor::WordMatch;

/// Op 9's row flags: its BLE beacon heard, its request heard on this network, the device being added, offered as a pick now (nearby, and over Bluetooth only while the add is contended), and owned by another identity's fleet.
pub const ROW_BLE: u8 = 1;
pub const ROW_LAN: u8 = 1 << 1;
pub const ROW_ADDING: u8 = 1 << 2;
pub const ROW_PICK: u8 = 1 << 3;
pub const ROW_OWNED: u8 = 1 << 4;

/// Op 9's phase byte, as the map reads it: 0 closed, 1 open, 2 binding, 3 a device selected and Yes waiting, 4 handing over the key, 5 added, 6 failed or refused.
/// The machine's Open carries the last add's outcome (`View::last`) until the page is shown again; the map shows it as 5 or 6 with the note. A page refused for want of the fleet key (`refused`) is the machine's Closed, and 6 here.
pub fn phase(v: &View, refused: bool) -> u8 {
    match v.phase {
        Phase::Closed if refused => 6,
        Phase::Closed => 0,
        Phase::Open => match v.last {
            Some((_, true)) => 5,
            Some((_, false)) => 6,
            None => 1,
        },
        Phase::Binding => 2,
        Phase::Selected => 3,
        Phase::Growing => 4,
    }
}

/// One row's flags; `adding` is the device the add under way is for.
pub fn flags(r: &Row, adding: Option<[u8; 32]>) -> u8 {
    let mut f = 0;
    if r.ble {
        f |= ROW_BLE;
    }
    if r.lan {
        f |= ROW_LAN;
    }
    if adding == Some(r.device) {
        f |= ROW_ADDING;
    }
    if r.pickable {
        f |= ROW_PICK;
    }
    if r.owned_elsewhere {
        f |= ROW_OWNED;
    }
    f
}

/// What the page says under an open flow when nothing else was said: fgtw.org being read, the add contended, or nobody asking yet.
pub fn quiet_note(v: &View, net_up: bool) -> &'static str {
    if v.listing {
        "reading the devices asking thru fgtw.org"
    } else if v.contended {
        "more than one device asks to join: until an add finishes, only one beside this device (Bluetooth or a tap) can be added"
    } else if v.rows.is_empty() && net_up {
        "waiting for a device to ask (this network, or fgtw.org once its words are typed)"
    } else if v.rows.is_empty() {
        "no network: a device asking is found thru fgtw.org (cellular or WiFi)"
    } else {
        ""
    }
}

/// Op 9: [phase][rows], each row [the key's first 8 bytes][flags, `ROW_*`][name length][name], then [note length][note], then the selected device's [name length][name][masked words length, u16 LE][masked words], both empty when none is selected.
/// `note` is the executor's last word on the flow; empty, the open flow's quiet note stands in.
pub fn reply(v: &View, refused: bool, adding: Option<[u8; 32]>, note: &str, net_up: bool) -> Vec<u8> {
    let mut r = Vec::with_capacity(256);
    let ph = phase(v, refused);
    r.push(ph);
    // WHY the cap: the Fleet page lays out at most 8 candidates, and the count is one byte.
    let n = v.rows.len().min(8);
    r.push(n as u8);
    for row in &v.rows[..n] {
        r.extend_from_slice(&row.device[..8]);
        r.push(flags(row, adding));
        // Two voca words: at most 36 bytes (the longest word is 18), so the length fits its byte.
        r.push(row.name.len() as u8);
        r.extend_from_slice(row.name.as_bytes());
    }
    let note = if note.is_empty() && ph == 1 { quiet_note(v, net_up) } else { note };
    // WHY the cap: a length byte; every note is a short sentence, a failure's reason the longest (fgtw's error text).
    let note = &note.as_bytes()[..note.len().min(255)];
    r.push(note.len() as u8);
    r.extend_from_slice(note);
    match &v.selected {
        Some((_, name, words)) => {
            // WHY the caps: a length byte, and two for the words; a device name is two words and the masked words a few hundred bytes.
            let name = &name.as_bytes()[..name.len().min(255)];
            r.push(name.len() as u8);
            r.extend_from_slice(name);
            let words = &words.as_bytes()[..words.len().min(0xFFFF)];
            r.extend_from_slice(&(words.len() as u16).to_le_bytes());
            r.extend_from_slice(words);
        }
        None => r.extend_from_slice(&[0, 0, 0]),
    }
    r
}

/// Op 11, the words typed so far as the machine left them: [kind][count], then the word to flag or the matched name. Kind 0 no candidates yet (the voca spell-check's first bad word after), 1 nothing typed (the count asking), 2 matched and being added (`added`, its name), 3 no device has this word, 4 one device still matches (its name), 5 several do (how many), 6 the flow is not open for words.
pub fn words(v: &View, added: Option<&str>) -> Vec<u8> {
    let mut r = Vec::with_capacity(48);
    if let Some(name) = added {
        r.extend_from_slice(&[2, 1]);
        r.extend_from_slice(name.as_bytes());
        return r;
    }
    // The machine matched against its word pool: every candidate, or only those heard over Bluetooth while the add is contended, in row order.
    let pool: Vec<&Row> = v.rows.iter().filter(|row| !v.contended || row.ble).collect();
    match &v.words {
        Some(WordMatch::NoCandidates { typo }) => {
            r.extend_from_slice(&[0, 0]);
            if let Some(w) = typo {
                r.extend_from_slice(w.as_bytes());
            }
        }
        // WHY the cap: a byte on the wire; past 255 devices asking the page has nothing different to say.
        Some(WordMatch::Nothing { asking }) => r.extend_from_slice(&[1, (*asking).min(255) as u8]),
        Some(WordMatch::NoMatch(w)) => {
            r.extend_from_slice(&[3, 0]);
            r.extend_from_slice(w.as_bytes());
        }
        Some(WordMatch::One(i)) => {
            r.extend_from_slice(&[4, 1]);
            // WHY .get: the index names the machine's own pool, which the view does not carry; it is rebuilt here from the rows by the machine's filter, and a pool that ever differs (a machine change) leaves the name out rather than ending the identity service.
            if let Some(row) = pool.get(*i) {
                r.extend_from_slice(row.name.as_bytes());
            }
        }
        // WHY the cap: as above.
        Some(WordMatch::Many(k)) => r.extend_from_slice(&[5, (*k).min(255) as u8]),
        // A full match is never left standing (the machine adds at once and clears the words); one refused (locked out, another fleet's) is said on the Fleet page, and the words end.
        Some(WordMatch::Full(_)) | None => r.extend_from_slice(&[6, 0]),
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::{String, ToString};

    fn row(k: u8, ble: bool, lan: bool, pickable: bool) -> Row {
        Row { device: [k; 32], name: alloc::format!("dev{k}"), ble, lan, pickable, owned_elsewhere: false }
    }

    fn view(phase: Phase, rows: Vec<Row>) -> View {
        View { phase, rows, selected: None, words: None, contended: false, listing: false, last: None }
    }

    #[test]
    fn phases_map_to_the_maps_bytes() {
        let mut v = view(Phase::Closed, Vec::new());
        assert_eq!(phase(&v, false), 0);
        assert_eq!(phase(&v, true), 6);
        v.phase = Phase::Open;
        assert_eq!(phase(&v, false), 1);
        v.last = Some(([1; 32], true));
        assert_eq!(phase(&v, false), 5);
        v.last = Some(([1; 32], false));
        assert_eq!(phase(&v, false), 6);
        v.phase = Phase::Binding;
        assert_eq!(phase(&v, false), 2);
        v.phase = Phase::Selected;
        assert_eq!(phase(&v, false), 3);
        v.phase = Phase::Growing;
        assert_eq!(phase(&v, false), 4);
    }

    #[test]
    fn a_reply_reads_back_as_the_map_reads_it() {
        let mut v = view(Phase::Selected, alloc::vec![row(1, true, false, true), row(2, false, true, false)]);
        v.rows[1].owned_elsewhere = true;
        v.selected = Some(([1; 32], "dev1".to_string(), "alpha **** gamma".to_string()));
        let r = reply(&v, false, Some([2; 32]), "", true);
        assert_eq!(r[0], 3);
        assert_eq!(r[1], 2);
        // Row 0: key prefix, flags (BLE, pickable), name.
        assert_eq!(&r[2..10], &[1; 8]);
        assert_eq!(r[10], ROW_BLE | ROW_PICK);
        assert_eq!(r[11], 4);
        assert_eq!(&r[12..16], b"dev1");
        // Row 1: LAN, the device being added, another fleet's; not pickable.
        assert_eq!(r[24], ROW_LAN | ROW_ADDING | ROW_OWNED);
        let at = 16 + 10 + 4;
        // No note for a selected flow with nothing said.
        assert_eq!(r[at], 0);
        assert_eq!(r[at + 1], 4);
        assert_eq!(&r[at + 2..at + 6], b"dev1");
        let wl = u16::from_le_bytes([r[at + 6], r[at + 7]]) as usize;
        assert_eq!(&r[at + 8..at + 8 + wl], b"alpha **** gamma");
        assert_eq!(r.len(), at + 8 + wl);
    }

    #[test]
    fn an_open_flow_with_nothing_said_says_why_it_is_quiet() {
        let v = view(Phase::Open, Vec::new());
        let r = reply(&v, false, None, "", false);
        let note = quiet_note(&v, false);
        assert_eq!(&r[..3], &[1, 0, note.len() as u8]);
        assert_eq!(&r[3..3 + note.len()], note.as_bytes());
        assert_eq!(&r[3 + note.len()..], &[0, 0, 0]);
        // A note said stands over the quiet one; a closed flow says nothing of its own.
        let r = reply(&v, false, None, "added", true);
        assert_eq!(&r[2..8], b"\x05added");
        let r = reply(&view(Phase::Closed, Vec::new()), false, None, "", true);
        assert_eq!(r, alloc::vec![0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_word_match_names_the_device_from_the_machines_pool() {
        let mut v = view(Phase::Open, alloc::vec![row(1, false, true, true), row(2, true, false, true)]);
        v.words = Some(WordMatch::One(0));
        assert_eq!(words(&v, None), b"\x04\x01dev1".to_vec());
        // Contended: the pool is the Bluetooth-heard rows only, so index 0 is the second row.
        v.contended = true;
        assert_eq!(words(&v, None), b"\x04\x01dev2".to_vec());
        v.words = Some(WordMatch::NoMatch(String::from("zebra")));
        assert_eq!(words(&v, None), b"\x03\x00zebra".to_vec());
        v.words = Some(WordMatch::Many(300));
        assert_eq!(words(&v, None), alloc::vec![5, 255]);
        v.words = Some(WordMatch::NoCandidates { typo: None });
        assert_eq!(words(&v, None), alloc::vec![0, 0]);
        v.words = None;
        assert_eq!(words(&v, None), alloc::vec![6, 0]);
        assert_eq!(words(&v, Some("dev2")), b"\x02\x01dev2".to_vec());
    }
}
