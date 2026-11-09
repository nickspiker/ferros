//! A WiFi network's credential as the vault keeps it (docs/ATTEST-UI.md "WiFi connect"): one VSF document, section `network`, its name and passphrase as ASCII text (`a`: a person's words, typed or read; vsf's `x` needs its Huffman tables, which a bare-metal build does not carry), stamped with its Eagle time and a provenance hash.
//! The person's is `wifi.cred` (identity class: networks travel with the person, AUTH.md); one joined before anyone attests is the device's, `wifi.net`.
//! It replaced the two-line text file (SSID, passphrase) on 2026-11-09; that file is read no more: a credential that is not this document is dropped.
//!
//! This file is shared by path with the host's bridge (`ferros-bridge wifi-cred`), so it uses only `vsf` and `alloc`.

use alloc::string::String;
use alloc::vec::Vec;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::VsfType;

const SECTION: &str = "network";

fn schema() -> SectionSchema {
    SectionSchema::new(SECTION).field("name", TypeConstraint::AsciiText).field("passphrase", TypeConstraint::AsciiText)
}

/// A name and passphrase WPA2-PSK can use: the name 1 to 32 bytes, the passphrase 8 to 63 (IEEE 802.11i's passphrase range).
pub fn usable(name: &[u8], pass: &[u8]) -> bool {
    (1..=32).contains(&name.len()) && (8..=63).contains(&pass.len())
}

/// The document for a credential, made at `eagle` (oscillations): None if the name or passphrase is not ASCII, or not usable.
pub fn build(name: &[u8], pass: &[u8], eagle: i64) -> Option<Vec<u8>> {
    if !usable(name, pass) || !name.is_ascii() || !pass.is_ascii() {
        return None;
    }
    let name = String::from_utf8(name.to_vec()).ok()?;
    let pass = String::from_utf8(pass.to_vec()).ok()?;
    let section = schema().build().set("name", VsfType::a(name)).ok()?.set("passphrase", VsfType::a(pass)).ok()?.encode().ok()?;
    vsf::VsfBuilder::new().creation_time_oscillations(eagle).provenance_only().add_unboxed(SECTION, section).build().ok()
}

/// The name and passphrase from a credential document, or None if it is not one (or what it holds is not usable).
pub fn parse(doc: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let section = SectionBuilder::parse_document(schema(), doc, None).ok()?;
    let text = |field: &str| match section.get_fields(field).first().and_then(|f| f.values.first()) {
        Some(VsfType::a(s)) => Some(s.as_bytes().to_vec()),
        _ => None,
    };
    let (name, pass) = (text("name")?, text("passphrase")?);
    usable(&name, &pass).then_some((name, pass))
}
