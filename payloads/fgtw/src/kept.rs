//! The departure machine's state as this service keeps it in the vault (depart.rs), and the form kept before the machine.
//! Pure: no syscall, no clock, so the conversion of a departure under way is tested on the host (the tests below).

use alloc::string::String;
use alloc::vec::Vec;
use fgtw::flow::depart::Depart;

/// A kept document, read.
pub enum Kept {
    /// The machine's state.
    Machine(Depart),
    /// The form kept before the machine, converted: our request with its stamp, salt, intent and signed frame.
    Old(Depart),
    /// The old form's request from a build before 2026-11-10 (words, no salt): it does not parse, and the person asks again.
    Stale,
    /// Another device's or another identity's: not taken up.
    Foreign,
    /// It does not verify or does not read.
    Unreadable(String),
}

/// The machine's state as kept: `Depart::to_section` without the handle proof (a root, which goes to disk only with unlock at boot), as one VSF document stamped `time`.
pub fn to_doc(m: &Depart, time: i64) -> Result<Vec<u8>, String> {
    let mut sec = m.to_section();
    sec.fields.retain(|f| f.name != "hp");
    vsf::VsfBuilder::new().creation_time_oscillations(time).add_section_direct(sec).build().map_err(|e| alloc::format!("depart state: {e}"))
}

/// A kept document read for the roots in hand (`hp`) and this device (`me`): the handle proof put back into the machine's state; the old form (section `depart`, field `rq`, the signed `depart_req` alone) converted, taken up only when its consent's Ed25519 egg verifies for these roots, as it was before the machine.
pub fn read(doc: &[u8], hp: &[u8; 32], me: &[u8; 32]) -> Kept {
    let (header, end) = match vsf::verification::read_verified(doc, None) {
        Ok(x) => x,
        Err(e) => return Kept::Unreadable(alloc::format!("{e}")),
    };
    let mut sec = match header.primary_section(doc, end) {
        Ok(s) => s,
        Err(e) => return Kept::Unreadable(alloc::format!("{e}")),
    };
    if let Some(frame) = sec.get_field("rq").and_then(|f| f.values.first()).and_then(|v| v.as_bytes()).map(|b| b.to_vec()) {
        return from_old(hp, me, frame);
    }
    if sec.get_field("me").and_then(|f| f.values.first()).and_then(|v| v.as_bytes()) != Some(&me[..]) {
        return Kept::Foreign;
    }
    sec.add_field("hp", vsf::VsfType::hP(hp.to_vec()));
    match Depart::from_section(&sec) {
        Ok(m) => Kept::Machine(m),
        Err(e) => Kept::Unreadable(e),
    }
}

/// The old form's signed request as the machine's own request: the section `Depart::to_section` writes for it (the stamp, salt, intent and the signed frame), so its delivery resumes with the frame the fleet may already hold and a deny naming its salt still ends it.
fn from_old(hp: &[u8; 32], me: &[u8; 32], frame: Vec<u8>) -> Kept {
    let Ok((req, leaver)) = fgtw::protocol::parse_depart_req(&frame) else { return Kept::Stale };
    let msg = fgtw::fleet::departreq_signing_bytes(hp, me, req.t);
    let ours = leaver == *me && fgtw::pq::eggs_from_bytes(&req.consent_sig).is_ok_and(|eggs| eggs.iter().any(|e| e.scheme == fgtw::fleet::scheme::ED25519 && fgtw::pq::verify_egg(e.scheme, me, &msg, &e.sig)));
    if !ours {
        return Kept::Foreign;
    }
    let mut s = vsf::VsfSection::new("depart");
    s.add_field("hp", vsf::VsfType::hP(hp.to_vec()));
    s.add_field("me", vsf::VsfType::ke(me.to_vec()));
    s.add_field("own_t", vsf::VsfType::e(vsf::types::EtType::e6(req.t)));
    s.add_field("own_salt", vsf::VsfType::hb(req.salt.to_vec()));
    s.add_field("own_intent", vsf::VsfType::u(req.intent as usize, false));
    s.add_field("own_frame", vsf::VsfType::v(b'r', frame));
    match Depart::from_section(&s) {
        Ok(m) => Kept::Old(m),
        Err(e) => Kept::Unreadable(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fgtw::flow::{Action, Event, To};
    use photon_core::join::{self, DeviceKeys};

    const HP: [u8; 32] = [0x11; 32];
    const T: i64 = 1 << 40;

    /// The old form as the build before the machine wrote it (`keep_depart`): section `depart`, `rq` the signed request as a binary blob.
    fn old_doc(keys: &DeviceKeys, salt: [u8; 32]) -> (Vec<u8>, Vec<u8>) {
        let consent = join::depart_consent(keys, &HP, T);
        let frame = join::depart_req(&join::DepartReq { t: T, consent_sig: consent, intent: join::DEPART_NEW_OWNER, salt }, keys, T).unwrap();
        let mut section = vsf::VsfSection::new("depart");
        section.add_field("rq", vsf::VsfType::v(b'b', frame.clone()));
        let doc = vsf::VsfBuilder::new().creation_time_oscillations(T).add_section_direct(section).build().unwrap();
        (doc, frame)
    }

    #[test]
    fn a_departure_kept_in_the_old_form_resumes_with_its_salt_and_frame() {
        let keys = DeviceKeys::derive(&[7; 32]);
        let me = keys.device_pubkey();
        let other = DeviceKeys::derive(&[8; 32]).device_pubkey();
        let salt = [0x5A; 32];
        let (doc, frame) = old_doc(&keys, salt);
        let Kept::Old(mut m) = read(&doc, &HP, &me) else { panic!("the old form was not converted") };
        assert!(m.asking());
        let sec = m.to_section();
        assert_eq!(sec.eagle("own_t"), Some(T));
        assert_eq!(sec.get_field("own_salt").and_then(|f| f.values.first()).and_then(|v| v.as_bytes()), Some(&salt[..]));
        assert_eq!(sec.get_field("own_frame").and_then(|f| f.values.first()).and_then(|v| v.as_bytes()), Some(&frame[..]));
        // Resumed before the fold, then told the members: the same signed frame to the other device, once, and its salt on the tag.
        let r = m.resume();
        assert!(r.actions.iter().any(|a| *a == Action::NfcServe(Some(salt))));
        assert!(!r.actions.iter().any(|a| matches!(a, Action::Build { .. })));
        let s = m.step(Event::Members { members: &[me, other], locked: &[] }, T + 1);
        let sends: Vec<_> = s.actions.iter().filter_map(|a| if let Action::Send { to, frame: f } = a { Some((*to, f.clone())) } else { None }).collect();
        assert_eq!(sends, alloc::vec![(To::Device(other), frame.clone())]);
        // Kept again in the machine's form (no handle proof in it) and read back: still the same request.
        let kept = to_doc(&m, T + 2).unwrap();
        assert!(!kept.windows(32).any(|w| w == HP));
        let Kept::Machine(back) = read(&kept, &HP, &me) else { panic!("the machine's form did not read back") };
        assert!(back.asking());
        assert_eq!(back.to_section().get_field("own_frame").and_then(|f| f.values.first()).and_then(|v| v.as_bytes()), Some(&frame[..]));
    }

    #[test]
    fn another_identitys_or_devices_departure_is_not_taken_up() {
        let keys = DeviceKeys::derive(&[7; 32]);
        let me = keys.device_pubkey();
        let (doc, _) = old_doc(&keys, [1; 32]);
        assert!(matches!(read(&doc, &[0x22; 32], &me), Kept::Foreign));
        let other = DeviceKeys::derive(&[9; 32]).device_pubkey();
        assert!(matches!(read(&doc, &HP, &other), Kept::Foreign));
        let m = Depart::new(HP, me);
        let kept = to_doc(&m, T).unwrap();
        assert!(matches!(read(&kept, &HP, &other), Kept::Foreign));
    }

    #[test]
    fn an_old_request_without_a_salt_is_stale() {
        let mut section = vsf::VsfSection::new("depart");
        section.add_field("rq", vsf::VsfType::v(b'b', alloc::vec![1, 2, 3]));
        let doc = vsf::VsfBuilder::new().creation_time_oscillations(T).add_section_direct(section).build().unwrap();
        assert!(matches!(read(&doc, &HP, &[3; 32]), Kept::Stale));
    }
}
