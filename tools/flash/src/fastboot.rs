//! A fastboot client (docs/FLASHER.md step 2), so the executable needs no Android platform-tools: the protocol over any bulk transport (usb.rs over nusb on the phone; a script in the tests).
//! The protocol: the host sends one command in ASCII; the bootloader answers packets beginning `INFO` or `TEXT` (words on the way), `DATA` (send this many bytes now), `OKAY` (done) or `FAIL` (refused, with its reason).
//! What this client will not send, whoever asks: `flashing lock`, `flashing lock_critical` and `oem lock` (the lock is the person's own act on the phone, never the tool's: "Claude memories/feedback_lock_prep_not_lock.md"); `set_active` on a board that forbids it (husky); a flash or erase of any partition outside `WRITABLE`.

use crate::board::Board;
use std::time::Duration;

/// A bulk pipe to the bootloader.
pub trait Transport {
    fn write(&mut self, data: &[u8]) -> Result<(), String>;
    /// One packet, up to `max` bytes, waiting at most `wait`.
    fn read(&mut self, max: usize, wait: Duration) -> Result<Vec<u8>, String>;
}

/// The only partitions the tool ever writes thru fastboot: the two boot slots and the custom AVB key the re-lock needs.
pub const WRITABLE: &[&str] = &["boot_a", "boot_b", "avb_custom_key"];

/// How long an ordinary command may take.
pub const WAIT: Duration = Duration::from_secs(1 << 4);
/// How long `flashing unlock` waits for the person to choose on the phone's own screen.
pub const WAIT_PERSON: Duration = Duration::from_secs(1 << 9);
/// The bulk write size within one download: small pieces, so a failing cable fails on a piece the log names.
pub const PIECE: usize = 1 << 20;

/// A finished command: the words after OKAY and every INFO and TEXT line before it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Okay {
    pub text: String,
    pub info: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// The bootloader answered FAIL.
    Fail {
        command: String,
        reason: String,
        info: Vec<String>,
    },
    /// The tool refused to send it.
    Refused(String),
    /// The USB pipe failed (the step and piece in words).
    Pipe(String),
    /// An answer the protocol does not have.
    Garbled(String),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            Error::Fail {
                command, reason, ..
            } => write!(f, "the bootloader refused `{command}`: {reason}"),
            Error::Refused(s) => write!(f, "not sent: {s}"),
            Error::Pipe(s) => write!(f, "USB: {s}"),
            Error::Garbled(s) => write!(f, "an answer fastboot does not have: {s}"),
        }
    }
}

/// The client, bound to the board the phone said it is.
pub struct Fastboot<T: Transport> {
    pub t: T,
    pub board: Board,
    /// Every INFO and TEXT line as it arrives (the run log takes them).
    pub on_line: Option<Box<dyn FnMut(&str)>>,
}

fn forbidden(cmd: &str) -> Option<&'static str> {
    let c = cmd.trim().to_ascii_lowercase();
    if c.starts_with("flashing lock") || c.starts_with("oem lock") {
        return Some("locking is the person's own act on the phone, never the tool's");
    }
    None
}

impl<T: Transport> Fastboot<T> {
    pub fn new(t: T, board: Board) -> Fastboot<T> {
        Fastboot {
            t,
            board,
            on_line: None,
        }
    }

    /// Send one command and wait for its OKAY or FAIL, collecting what comes before.
    pub fn command(&mut self, cmd: &str, wait: Duration) -> Result<Okay, Error> {
        if let Some(why) = forbidden(cmd) {
            return Err(Error::Refused(format!("`{cmd}`: {why}")));
        }
        if cmd.len() > 1 << 6 {
            return Err(Error::Refused(format!(
                "`{cmd}` is longer than a fastboot command may be"
            )));
        }
        self.t
            .write(cmd.as_bytes())
            .map_err(|e| Error::Pipe(format!("sending `{cmd}`: {e}")))?;
        self.answer(cmd, wait)
    }

    fn answer(&mut self, cmd: &str, wait: Duration) -> Result<Okay, Error> {
        let mut info = Vec::new();
        loop {
            let p = self
                .t
                .read(1 << 8, wait)
                .map_err(|e| Error::Pipe(format!("waiting for `{cmd}`: {e}")))?;
            if p.len() < 4 {
                return Err(Error::Garbled(format!("`{cmd}`: {p:?}")));
            }
            let words = String::from_utf8_lossy(&p[4..]).trim_end().to_string();
            match &p[..4] {
                b"INFO" | b"TEXT" => {
                    if let Some(f) = self.on_line.as_mut() {
                        f(&words);
                    }
                    info.push(words);
                }
                b"OKAY" => return Ok(Okay { text: words, info }),
                b"FAIL" => {
                    return Err(Error::Fail {
                        command: cmd.into(),
                        reason: words,
                        info,
                    })
                }
                b"DATA" => {
                    return Err(Error::Garbled(format!(
                        "`{cmd}`: DATA where none was asked"
                    )))
                }
                _ => {
                    return Err(Error::Garbled(format!(
                        "`{cmd}`: {}",
                        String::from_utf8_lossy(&p)
                    )))
                }
            }
        }
    }

    pub fn getvar(&mut self, name: &str) -> Result<String, Error> {
        self.command(&format!("getvar:{name}"), WAIT)
            .map(|o| o.text)
    }

    /// Every variable the bootloader lists (`getvar:all`), as its own `name: value` lines.
    pub fn getvar_all(&mut self) -> Result<Vec<String>, Error> {
        self.command("getvar:all", WAIT).map(|o| o.info)
    }

    /// Send `bytes` to the bootloader's download buffer, in `PIECE` writes; `on_piece` hears each piece's end (bytes so far).
    pub fn download(&mut self, bytes: &[u8], mut on_piece: impl FnMut(usize)) -> Result<(), Error> {
        let max = self
            .getvar("max-download-size")
            .ok()
            .and_then(|v| parse_num(&v));
        if let Some(m) = max {
            if bytes.len() as u64 > m {
                return Err(Error::Refused(format!(
                    "A#{} bytes is more than this bootloader takes at once (A#{m})",
                    bytes.len()
                )));
            }
        }
        let cmd = format!("download:{:08x}", bytes.len());
        self.t
            .write(cmd.as_bytes())
            .map_err(|e| Error::Pipe(format!("sending `{cmd}`: {e}")))?;
        let p = self
            .t
            .read(1 << 8, WAIT)
            .map_err(|e| Error::Pipe(format!("waiting for DATA: {e}")))?;
        match p.get(..4) {
            Some(b"DATA") => {}
            Some(b"FAIL") => {
                return Err(Error::Fail {
                    command: cmd,
                    reason: String::from_utf8_lossy(&p[4..]).into(),
                    info: Vec::new(),
                })
            }
            _ => {
                return Err(Error::Garbled(format!(
                    "`{cmd}`: {}",
                    String::from_utf8_lossy(&p)
                )))
            }
        }
        let mut sent = 0;
        for piece in bytes.chunks(PIECE) {
            self.t
                .write(piece)
                .map_err(|e| Error::Pipe(format!("piece at A#{sent} of A#{}: {e}", bytes.len())))?;
            sent += piece.len();
            on_piece(sent);
        }
        self.answer(&cmd, WAIT).map(|_| ())
    }

    /// Flash what was downloaded to `part`, one of `WRITABLE`.
    pub fn flash(&mut self, part: &str) -> Result<Okay, Error> {
        if !WRITABLE.contains(&part) {
            return Err(Error::Refused(format!(
                "`{part}` is not a partition this tool writes"
            )));
        }
        self.command(&format!("flash:{part}"), WAIT * 4)
    }

    pub fn set_active(&mut self, slot: &str) -> Result<Okay, Error> {
        if !self.board.slot_switch {
            return Err(Error::Refused(format!(
                "no slot switch on {}: it strands the phone in fastboot",
                self.board.model
            )));
        }
        if slot != "a" && slot != "b" {
            return Err(Error::Refused(format!("`{slot}` is not a slot")));
        }
        self.command(&format!("set_active:{slot}"), WAIT)
    }

    /// Whether the bootloader is unlocked (`getvar unlocked`: yes or no).
    pub fn unlocked(&mut self) -> Result<bool, Error> {
        let v = self.getvar("unlocked")?;
        Ok(v.trim().eq_ignore_ascii_case("yes"))
    }

    /// Whether the phone allows unlocking (Android's "OEM unlocking" switch, or ferros's "Allow unlocking"): `flashing get_unlock_ability` answers 1 or 0 in an INFO line.
    pub fn unlock_ability(&mut self) -> Result<bool, Error> {
        let o = self.command("flashing get_unlock_ability", WAIT)?;
        Ok(o.info
            .iter()
            .chain(core::iter::once(&o.text))
            .any(|l| l.split(':').nth(1).is_some_and(|v| v.trim() == "1")))
    }

    /// Ask the phone to unlock: it shows its own screen, the person chooses with its keys, and the phone wipes itself if they say yes.
    pub fn unlock(&mut self) -> Result<Okay, Error> {
        self.command("flashing unlock", WAIT_PERSON)
    }

    pub fn pkvm_status(&mut self) -> Result<Okay, Error> {
        self.command("oem pkvm status", WAIT)
    }

    pub fn pkvm_disable(&mut self) -> Result<Okay, Error> {
        self.command("oem pkvm disable", WAIT)
    }

    pub fn reboot(&mut self) -> Result<Okay, Error> {
        self.command("reboot", WAIT)
    }

    pub fn reboot_bootloader(&mut self) -> Result<Okay, Error> {
        self.command("reboot-bootloader", WAIT)
    }
}

/// A bootloader's number: `0x` hex (fastboot's own form) or plain decimal.
pub fn parse_num(s: &str) -> Option<u64> {
    let s = s.trim();
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A bootloader played from a script: what it answers, and what it was sent.
    struct Script {
        answers: VecDeque<Vec<u8>>,
        sent: Vec<Vec<u8>>,
    }

    impl Transport for Script {
        fn write(&mut self, data: &[u8]) -> Result<(), String> {
            self.sent.push(data.to_vec());
            Ok(())
        }
        fn read(&mut self, _max: usize, _wait: Duration) -> Result<Vec<u8>, String> {
            self.answers.pop_front().ok_or_else(|| "timed out".into())
        }
    }

    fn fb(answers: &[&[u8]], board: Board) -> Fastboot<Script> {
        Fastboot::new(
            Script {
                answers: answers.iter().map(|a| a.to_vec()).collect(),
                sent: Vec::new(),
            },
            board,
        )
    }

    #[test]
    fn getvar_and_info_lines() {
        let mut f = fb(&[b"OKAYakita"], Board::akita());
        assert_eq!(f.getvar("product").unwrap(), "akita");
        assert_eq!(f.t.sent[0], b"getvar:product");
        let mut f = fb(
            &[b"INFOproduct:akita", b"INFOunlocked:yes", b"OKAY"],
            Board::akita(),
        );
        assert_eq!(
            f.getvar_all().unwrap(),
            vec!["product:akita", "unlocked:yes"]
        );
    }

    #[test]
    fn fail_carries_its_reason() {
        let mut f = fb(&[b"FAILunknown command"], Board::akita());
        match f.pkvm_status() {
            Err(Error::Fail { reason, .. }) => assert_eq!(reason, "unknown command"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn download_in_pieces_then_flash() {
        let img = vec![9u8; PIECE * 2 + 5];
        let mut f = fb(
            &[b"OKAY0x10000000", b"DATA00200005", b"OKAY", b"OKAY"],
            Board::akita(),
        );
        let mut pieces = Vec::new();
        f.download(&img, |n| pieces.push(n)).unwrap();
        f.flash("boot_b").unwrap();
        assert_eq!(pieces, vec![PIECE, PIECE * 2, PIECE * 2 + 5]);
        assert_eq!(f.t.sent[1], b"download:00200005");
        assert_eq!(f.t.sent.last().unwrap(), b"flash:boot_b");
    }

    #[test]
    fn too_big_for_the_bootloader_is_refused_before_sending() {
        let mut f = fb(&[b"OKAY0x100"], Board::akita());
        assert!(matches!(
            f.download(&[0; 512], |_| {}),
            Err(Error::Refused(_))
        ));
        assert_eq!(f.t.sent.len(), 1);
    }

    #[test]
    fn never_locks_never_writes_elsewhere() {
        let mut f = fb(&[], Board::akita());
        for c in [
            "flashing lock",
            "flashing lock_critical",
            "oem lock",
            "  FLASHING LOCK",
        ] {
            assert!(matches!(f.command(c, WAIT), Err(Error::Refused(_))), "{c}");
        }
        for p in [
            "userdata",
            "super",
            "vendor_boot_a",
            "bootloader_a",
            "persist",
            "efs",
        ] {
            assert!(matches!(f.flash(p), Err(Error::Refused(_))), "{p}");
        }
        assert!(f.t.sent.is_empty());
    }

    #[test]
    fn husky_never_switches_slots() {
        let mut f = fb(&[], Board::husky());
        assert!(matches!(f.set_active("b"), Err(Error::Refused(_))));
        let mut f = fb(&[b"OKAY"], Board::akita());
        assert!(f.set_active("b").is_ok());
        assert!(matches!(f.set_active("c"), Err(Error::Refused(_))));
    }

    #[test]
    fn unlock_ability() {
        let mut f = fb(&[b"INFOget_unlock_ability: 1", b"OKAY"], Board::akita());
        assert!(f.unlock_ability().unwrap());
        let mut f = fb(&[b"INFOget_unlock_ability: 0", b"OKAY"], Board::akita());
        assert!(!f.unlock_ability().unwrap());
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_num("0x10000000"), Some(1 << 28));
        assert_eq!(parse_num("4096"), Some(4096));
        assert_eq!(parse_num("yes"), None);
    }
}
