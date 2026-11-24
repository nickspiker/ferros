//! The signed release manifest (docs/FLASHER.md "Distribution"): one VSF document, Ed25519-signed with the ferros key, naming every published file by its BLAKE3.
//! The tool fetches it first, checks the signature against the key it was built with, and checks every file it later downloads against the manifest's hash before writing anything.
//! Files live on the host under their own hash (`<base>/<blake3 hex>`), so a host can swap nothing without the manifest changing, and the manifest cannot change without the key.
//! The owner, 2026-11-24: hosted images are production builds only, with every Android path compiled out; the manifest's `channel` says which a release is.

use vsf::{VsfBuilder, VsfHeader, VsfSection, VsfType};

/// The key a release must be signed with: the ferros key (keys/ferros.pub), unless the build names another.
pub const FERROS_PUB: [u8; 32] = [
    0x3b, 0x47, 0x2b, 0x69, 0x72, 0x36, 0x9e, 0x17, 0x9f, 0x7d, 0xfe, 0x0e, 0x7b, 0xb9, 0xeb, 0xbf,
    0x6d, 0xe0, 0xe8, 0xd4, 0x1b, 0xfd, 0x78, 0xbb, 0xce, 0xa4, 0x63, 0x24, 0xed, 0x3c, 0xb8, 0x0d,
];

const RELEASE: &str = "release";
const ARTEFACT: &str = "artefact";

/// What a published file is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// This tool's own executable for one platform (`platform` names it).
    Tool = 0,
    /// A board manifest (board.rs).
    Board = 1,
    /// The whole boot partition, footer included, flashed to both slots.
    Boot = 2,
    /// A shell process's signed capsule, stored into the vault at provisioning.
    Capsule = 3,
    /// The AVB public key blob for `fastboot flash avb_custom_key`, which the re-lock needs.
    AvbKey = 4,
}

impl Role {
    fn from_u64(v: u64) -> Option<Role> {
        [
            Role::Tool,
            Role::Board,
            Role::Boot,
            Role::Capsule,
            Role::AvbKey,
        ]
        .into_iter()
        .find(|r| *r as u64 == v)
    }
}

/// One published file.
#[derive(Clone, Debug, PartialEq)]
pub struct Artefact {
    pub role: Role,
    /// Its name in words of the build (`ferros-flash.exe`, `boot`, `vault`): shown to the person and in the log, never used to find it.
    pub name: String,
    /// The board it is for (empty for the tool and anything any board takes).
    pub board: String,
    /// The tool's platform (`windows-x86_64`, `linux-x86_64`, `macos-arm64`); empty otherwise.
    pub platform: String,
    pub size: u64,
    pub blake3: [u8; 32],
}

impl Artefact {
    /// The file's name on the host: its BLAKE3 in hex.
    pub fn file_name(&self) -> String {
        self.blake3.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Whether `bytes` are this file.
    pub fn check(&self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() as u64 != self.size {
            return Err(format!(
                "{}: A#{} bytes, the release says A#{}",
                self.name,
                bytes.len(),
                self.size
            ));
        }
        if blake3::hash(bytes).as_bytes() != &self.blake3 {
            return Err(format!("{}: its BLAKE3 is not the release's", self.name));
        }
        Ok(())
    }
}

/// A release: which channel, when, from which commit, and its files.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// `production` (the only channel hosted for people, owner 2026-11-24) or `development`.
    pub channel: String,
    pub made: i64,
    /// The ferros commit it was built from (git SHA-1, 20 bytes).
    pub commit: [u8; 20],
    pub artefacts: Vec<Artefact>,
}

fn a(s: &str) -> VsfType {
    VsfType::a(vsf::types::Ascii::new(s).expect("release names are ASCII"))
}

impl Release {
    /// The unsigned document, with the signer's key in the header and its signature slot zeroed for `sign_file`.
    pub fn document(&self, signer: &[u8; 32]) -> Result<Vec<u8>, String> {
        let mut s = VsfSection::new(RELEASE);
        s.add_field("channel", a(&self.channel));
        s.add_field("commit", VsfType::hs(self.commit.to_vec()));
        let mut b = VsfBuilder::new()
            .creation_time_oscillations(self.made)
            .add_section_direct(s)
            .signature_ed25519(*signer, [0; 64]);
        for f in &self.artefacts {
            let mut s = VsfSection::new(ARTEFACT);
            s.set_uint("role", f.role as u64);
            s.add_field("name", a(&f.name));
            if !f.board.is_empty() {
                s.add_field("board", a(&f.board));
            }
            if !f.platform.is_empty() {
                s.add_field("platform", a(&f.platform));
            }
            s.set_uint("size", f.size);
            s.add_field("hash", VsfType::hb(f.blake3.to_vec()));
            b = b.add_section_direct(s);
        }
        b.build()
    }

    /// The signed document (the publisher's side; the seed comes from keys/, never from this repo).
    #[cfg(feature = "publish")]
    pub fn sign(&self, seed: &[u8; 32]) -> Result<Vec<u8>, String> {
        let public = ed25519_dalek::SigningKey::from_bytes(seed)
            .verifying_key()
            .to_bytes();
        vsf::verification::sign_file(self.document(&public)?, seed)
    }

    /// A release read from its document, only if it is signed by `key` and the signature holds.
    pub fn verify(doc: &[u8], key: &[u8; 32]) -> Result<Release, String> {
        let (h, end) = VsfHeader::decode_strict(doc)?;
        match &h.signer_pubkey {
            Some(VsfType::ke(k)) if k.as_slice() == key => {}
            Some(_) => return Err("the release is signed by another key".into()),
            None => return Err("the release is not signed".into()),
        }
        if !vsf::verification::verify_file_signature(doc)? {
            return Err("the release's signature does not hold".into());
        }
        let sections = h.sections(doc, end)?;
        let r = sections
            .iter()
            .find(|s| s.name == RELEASE)
            .ok_or("no release section")?;
        let commit: [u8; 20] = r
            .bytes("commit")
            .and_then(|b| b.try_into().ok())
            .ok_or("release: no 20-byte `commit`")?;
        let artefacts = sections
            .iter()
            .filter(|s| s.name == ARTEFACT)
            .map(|s| {
                Ok(Artefact {
                    role: s
                        .uint("role")
                        .and_then(Role::from_u64)
                        .ok_or("artefact: no `role`")?,
                    name: s.text("name").ok_or("artefact: no `name`")?.to_string(),
                    board: s.text("board").unwrap_or("").to_string(),
                    platform: s.text("platform").unwrap_or("").to_string(),
                    size: s.uint("size").ok_or("artefact: no `size`")?,
                    blake3: s.bytes32("hash").ok_or("artefact: no 32-byte `hash`")?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Release {
            channel: r
                .text("channel")
                .ok_or("release: no `channel`")?
                .to_string(),
            made: h
                .creation_time
                .as_ref()
                .and_then(|v| v.as_eagle())
                .unwrap_or(0),
            commit,
            artefacts,
        })
    }

    /// The release's file of `role` for `board` (and `platform`, for the tool), when it has one.
    pub fn find(&self, role: Role, board: &str, platform: &str) -> Option<&Artefact> {
        self.artefacts
            .iter()
            .find(|f| f.role == role && f.board == board && f.platform == platform)
    }
}

#[cfg(all(test, feature = "publish"))]
mod tests {
    use super::*;

    fn sample() -> Release {
        let boot = vec![7u8; 1 << 12];
        Release {
            channel: "production".into(),
            made: vsf::eagle_time_oscillations(),
            commit: [0xAB; 20],
            artefacts: vec![
                Artefact {
                    role: Role::Boot,
                    name: "boot".into(),
                    board: "akita".into(),
                    platform: String::new(),
                    size: boot.len() as u64,
                    blake3: *blake3::hash(&boot).as_bytes(),
                },
                Artefact {
                    role: Role::Tool,
                    name: "ferros-flash.exe".into(),
                    board: String::new(),
                    platform: "windows-x86_64".into(),
                    size: 3,
                    blake3: *blake3::hash(b"exe").as_bytes(),
                },
            ],
        }
    }

    #[test]
    fn signed_round_trip() {
        let seed = [42u8; 32];
        let public = ed25519_dalek::SigningKey::from_bytes(&seed)
            .verifying_key()
            .to_bytes();
        let r = sample();
        let doc = r.sign(&seed).unwrap();
        let back = Release::verify(&doc, &public).unwrap();
        assert_eq!(back, r);
        assert!(back
            .find(Role::Boot, "akita", "")
            .unwrap()
            .check(&[7u8; 1 << 12])
            .is_ok());
        assert!(back
            .find(Role::Boot, "akita", "")
            .unwrap()
            .check(&[8u8; 1 << 12])
            .is_err());
        assert!(back.find(Role::Boot, "husky", "").is_none());
    }

    #[test]
    fn another_key_or_a_changed_byte_is_refused() {
        let seed = [42u8; 32];
        let public = ed25519_dalek::SigningKey::from_bytes(&seed)
            .verifying_key()
            .to_bytes();
        let doc = sample().sign(&seed).unwrap();
        assert!(Release::verify(&doc, &FERROS_PUB).is_err());
        // A changed byte anywhere past the header (an artefact's hash, say) breaks the signature.
        let mut bad = doc.clone();
        let last = bad.len() - 2;
        bad[last] ^= 1;
        assert!(Release::verify(&bad, &public).is_err());
    }
}
