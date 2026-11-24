//! Board manifests (docs/FLASHER.md step 1): everything the flasher knows about one phone model, as one VSF document.
//! The tool carries the manifests it was built with (`akita`, `husky`) and takes a newer one only from a signed release (release.rs), so a board fix ships without a new executable.
//! The owner, 2026-11-24: pure ferros only, no Android beside it ("Default pure ferros, we do not support Android dual boot. Period.").
//! So a manifest names no Android option: the carve gives ferros everything past userdata's floor.

use vsf::{VsfBuilder, VsfHeader, VsfSection, VsfType};

/// Where a firmware or calibration file comes from today and where it lives on the phone (the survey of 2026-11-24, docs/FLASHER.md "Firmware from the phone").
#[derive(Clone, Debug, PartialEq)]
pub struct Firmware {
    /// The name the driver asks for (a vault name today, `wifi.fw`), or the file's own name where the driver already reads the partition.
    pub name: String,
    /// The partition it lives on: `vendor` (the slot's, inside super), `persist`, `efs`, `modem`, `modem_userdata`; empty when it is on no partition of the phone.
    pub part: String,
    /// Its path inside that partition.
    pub path: String,
    /// Where the driver takes it from today.
    pub today: Source,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    /// Read from the phone's own partition by the kernel's partfs (nothing to provision).
    Partition = 0,
    /// A copy stored in the vault by a host script (to be moved to `Partition`).
    Vault = 1,
    /// Not a file on the phone at all: an oracle capture or the owner's own measurement, shipped with ferros.
    Ferros = 2,
}

impl Source {
    fn from_u64(v: u64) -> Option<Source> {
        match v {
            0 => Some(Source::Partition),
            1 => Some(Source::Vault),
            2 => Some(Source::Ferros),
            _ => None,
        }
    }
}

/// One board, as the flasher uses it.
#[derive(Clone, Debug, PartialEq)]
pub struct Board {
    /// The board's name and the product the bootloader answers to `getvar product` (they are the same word on Pixels).
    pub product: String,
    /// The name people know it by.
    pub model: String,
    /// The bootloader's fastboot USB identity.
    pub fastboot_vid: u16,
    pub fastboot_pid: u16,
    /// ferros's own USB identity once it runs.
    pub ferros_vid: u16,
    pub ferros_pid: u16,
    /// The flash's logical block size and the LU holding userdata and ferros.
    pub block: u64,
    pub lun: u64,
    /// Whether `set_active` may be sent (husky: never, it strands the phone in fastboot, BOOT.md 2026-10-23).
    pub slot_switch: bool,
    /// The boot partition's size: images are flashed whole, with their own AVB footer at its end.
    pub boot_bytes: u64,
    /// The largest single fastboot download the tool sends before it cuts an image (the bootloader's own `max-download-size` caps it further).
    pub send_piece: u64,
    /// userdata's size after the carve: the GPT entry stays (ABL and the GPT's A/B entries are left as Google made them), its contents are never used.
    pub userdata_floor: u64,
    /// The smallest ferros partition today's kernel opens a vault in (its fixed windows: the gap to ONE-VAULT.md).
    pub ferros_min: u64,
    /// Where today's kernel requires the ferros partition to start and end, when it does (husky: ufs_device.rs FERROS_BASE_LBA..=FERROS_LAST_LBA); 0 = found by name, anywhere.
    pub ferros_first_lba: u64,
    pub ferros_last_lba: u64,
    /// The per-unit partitions backed up to the computer before anything is written, every one, never optional.
    pub backup: Vec<String>,
    /// Where each firmware and calibration file lives on the phone.
    pub firmware: Vec<Firmware>,
    /// What is known to go wrong on this board, in words, shown before the step it concerns.
    pub traps: Vec<String>,
}

/// The document's section names.
const BOARD: &str = "board";
const BACKUP: &str = "backup";
const FIRMWARE: &str = "firmware";
const TRAP: &str = "trap";

const GIB: u64 = 1 << 30;

fn a(s: &str) -> VsfType {
    VsfType::a(vsf::types::Ascii::new(s).expect("board manifest names are ASCII"))
}

fn fw(name: &str, part: &str, path: &str, today: Source) -> Firmware {
    Firmware {
        name: name.into(),
        part: part.into(),
        path: path.into(),
        today,
    }
}

/// The per-unit partitions both boards back up (husky_factory_partitions_backup, akita_pixel8a: the 2026-11-18 sets).
fn calibration() -> Vec<String> {
    [
        "persist",
        "efs",
        "efs_backup",
        "modem_userdata",
        "devinfo",
        "pinfo",
        "blenv",
        "mfg_data",
        "fips",
        "trusty_persist",
        "misc",
        "frp",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

impl Board {
    /// The Pixel 8a (BOARDS.md, akita_pixel8a): the first board of the first real-world test.
    pub fn akita() -> Board {
        Board {
            product: "akita".into(),
            model: "Pixel 8a".into(),
            fastboot_vid: 0x18D1,
            fastboot_pid: 0x4EE0,
            ferros_vid: 0x1209,
            ferros_pid: 0x4665,
            block: 1 << 12,
            lun: 0,
            slot_switch: true,
            boot_bytes: 1 << 26,
            send_piece: 1 << 26,
            userdata_floor: GIB,
            // The vault process's window ends 20 GiB in (process.rs WINDOWS); RAW clips need 72 GiB.
            ferros_min: 20 * GIB,
            ferros_first_lba: 0,
            ferros_last_lba: 0,
            backup: calibration(),
            firmware: vec![
                fw("modem.bin", "modem", "/images/default/modem.bin", Source::Partition),
                fw("uecap", "modem", "/images/default/uecap", Source::Partition),
                fw("nv_normal.bin", "efs", "/nv_normal.bin", Source::Partition),
                fw("nv_protected.bin", "efs", "/nv_protected.bin", Source::Partition),
                fw("replay_region.bin", "modem_userdata", "/replay_region.bin", Source::Partition),
                fw("cpsha", "persist", "/modem/cpsha", Source::Partition),
                fw("carrierconfig", "vendor", "/firmware/carrierconfig", Source::Partition),
                fw("BCM.hcd", "vendor", "/firmware/brcm/BCM.hcd", Source::Partition),
                fw("wifi.fw", "vendor", "/firmware/fw_bcmdhd.bin_4383_a3", Source::Vault),
                fw("wifi.clm", "vendor", "/firmware/bcmdhd_clm.blob_4383_a3", Source::Vault),
                // The board's calibration file is a `bcmdhd.cal_*`: its exact name on the 8a is not yet read from the phone.
                fw("wifi.nvram", "vendor", "/firmware/bcmdhd.cal_*", Source::Vault),
                fw("aoc.fw", "vendor", "/firmware/aoc.bin", Source::Vault),
                fw("aoc.reg", "vendor", "/etc/sensors/registry", Source::Vault),
                fw("aoc.reg.calibration", "persist", "/sensors/registry", Source::Vault),
                fw("gps.kepler", "vendor", "/firmware/kepler.bin", Source::Vault),
                // The 8a's vendor carries r54p1 to r54p3 and r56p0; the gpu process speaks r52p0, so the 8a runs husky's file today.
                fw("gpu.fw", "vendor", "/firmware/mali_csffw-r54p1.bin", Source::Vault),
                fw("gps.kepler.cfg", "", "", Source::Ferros),
                fw("gps.kepler.session", "", "", Source::Ferros),
                fw("cam.oksoko", "", "", Source::Ferros),
                fw("cam.leshen", "", "", Source::Ferros),
            ],
            traps: vec![
                "Boot images are flashed whole with their own AVB footer: a bare image buries Google's vbmeta and the bootloader refuses the slot.".into(),
                "If fastboot shows the phone but stops answering, the bootloader is wedged: hold power and volume down until it restarts.".into(),
                "A USB port or cable that drops the phone within seconds of enumerating looks like a phone fault: try another port or cable first.".into(),
            ],
        }
    }

    /// The Pixel 8 Pro (BOARDS.md, BOOT.md), after akita.
    pub fn husky() -> Board {
        let mut b = Board::akita();
        b.product = "husky".into();
        b.model = "Pixel 8 Pro".into();
        b.slot_switch = false;
        // Today's kernel opens husky's vault only where the 2026-08 carve put it (ufs_device.rs): a person's husky must be carved to exactly this until the kernel finds it by name as on akita.
        b.ferros_first_lba = 28_881_920;
        b.ferros_last_lba = 62_436_346;
        b.firmware = vec![
            fw(
                "modem.bin",
                "modem",
                "/images/default/modem.bin",
                Source::Partition,
            ),
            fw("uecap", "modem", "/images/default/uecap", Source::Partition),
            fw("nv_normal.bin", "efs", "/nv_normal.bin", Source::Partition),
            fw(
                "nv_protected.bin",
                "efs",
                "/nv_protected.bin",
                Source::Partition,
            ),
            fw(
                "replay_region.bin",
                "modem_userdata",
                "/replay_region.bin",
                Source::Partition,
            ),
            fw("cpsha", "persist", "/modem/cpsha", Source::Partition),
            fw(
                "carrierconfig",
                "vendor",
                "/firmware/carrierconfig",
                Source::Partition,
            ),
            fw(
                "BCM.hcd",
                "vendor",
                "/firmware/brcm/BCM.hcd",
                Source::Partition,
            ),
            fw(
                "wifi.fw",
                "vendor",
                "/firmware/fw_bcmdhd.bin_4398_d0",
                Source::Vault,
            ),
            fw(
                "wifi.nvram",
                "vendor",
                "/firmware/bcmdhd.cal_USI_G5SN_9957_V17",
                Source::Vault,
            ),
            fw(
                "wifi.clm",
                "vendor",
                "/firmware/bcmdhd_clm.blob_4398_d0",
                Source::Vault,
            ),
            fw(
                "wifi.txcap",
                "vendor",
                "/firmware/bcmdhd_txcap.blob_4398_d0",
                Source::Vault,
            ),
            fw(
                "gpu.fw",
                "vendor",
                "/firmware/mali_csffw-r52p0.bin",
                Source::Vault,
            ),
            fw("aoc.fw", "vendor", "/firmware/aoc.bin", Source::Vault),
            fw("aoc.reg", "vendor", "/etc/sensors/registry", Source::Vault),
            fw(
                "aoc.reg.calibration",
                "persist",
                "/sensors/registry",
                Source::Vault,
            ),
            fw("gps.replay", "", "", Source::Ferros),
            fw("cam.husky", "", "", Source::Ferros),
        ];
        b.traps = vec![
            "Never switch slots on this phone from fastboot: it boots the UFS boot LU's slot and strands itself in fastboot when told otherwise.".into(),
            "A whole 64 MiB send can fail with \"Write to device failed\" on some cables and hubs: the tool retries, and another cable or a USB 3 port is the cure.".into(),
            "Boot images are flashed whole with their own AVB footer.".into(),
        ];
        b
    }

    /// The boards this executable was built with.
    pub fn builtin() -> [Board; 2] {
        [Board::akita(), Board::husky()]
    }

    /// The built-in board whose product this is.
    pub fn for_product(product: &str) -> Option<Board> {
        Board::builtin().into_iter().find(|b| b.product == product)
    }

    /// The board as a complete VSF document: one `board` section, then one section a backup partition, a firmware file and a trap.
    pub fn to_vsf(&self, made: i64) -> Result<Vec<u8>, String> {
        let mut s = VsfSection::new(BOARD);
        s.add_field("product", a(&self.product));
        s.set_text("model", &self.model);
        s.set_uint("fastboot_vid", self.fastboot_vid as u64);
        s.set_uint("fastboot_pid", self.fastboot_pid as u64);
        s.set_uint("ferros_vid", self.ferros_vid as u64);
        s.set_uint("ferros_pid", self.ferros_pid as u64);
        s.set_uint("block", self.block);
        s.set_uint("lun", self.lun);
        s.add_field("slot_switch", VsfType::u0(self.slot_switch));
        s.set_uint("boot_bytes", self.boot_bytes);
        s.set_uint("send_piece", self.send_piece);
        s.set_uint("userdata_floor", self.userdata_floor);
        s.set_uint("ferros_min", self.ferros_min);
        if self.ferros_last_lba != 0 {
            s.set_uint("ferros_first_lba", self.ferros_first_lba);
            s.set_uint("ferros_last_lba", self.ferros_last_lba);
        }
        let mut b = VsfBuilder::new()
            .creation_time_oscillations(made)
            .provenance_only()
            .add_section_direct(s);
        for p in &self.backup {
            let mut s = VsfSection::new(BACKUP);
            s.add_field("part", a(p));
            b = b.add_section_direct(s);
        }
        for f in &self.firmware {
            let mut s = VsfSection::new(FIRMWARE);
            s.add_field("name", a(&f.name));
            if !f.part.is_empty() {
                s.add_field("part", a(&f.part));
                s.add_field("path", a(&f.path));
            }
            s.set_uint("today", f.today as u64);
            b = b.add_section_direct(s);
        }
        for t in &self.traps {
            let mut s = VsfSection::new(TRAP);
            s.set_text("text", t);
            b = b.add_section_direct(s);
        }
        b.build()
    }

    /// A board read back from its document; every required field must be there and typed as written.
    pub fn from_vsf(doc: &[u8]) -> Result<Board, String> {
        let (h, end) = VsfHeader::decode_strict(doc)?;
        let sections = h.sections(doc, end)?;
        Board::from_sections(&sections)
    }

    /// A board from already parsed sections (a release document carries the same sections beside its own).
    pub fn from_sections(sections: &[VsfSection]) -> Result<Board, String> {
        let s = sections
            .iter()
            .find(|s| s.name == BOARD)
            .ok_or("no board section")?;
        let u = |n: &str| s.uint(n).ok_or_else(|| format!("board: no `{n}`"));
        let t = |n: &str| {
            s.text(n)
                .map(str::to_string)
                .ok_or_else(|| format!("board: no `{n}`"))
        };
        let u16_of = |n: &str| -> Result<u16, String> {
            u16::try_from(u(n)?).map_err(|_| format!("board: `{n}` is not a USB id"))
        };
        Ok(Board {
            product: t("product")?,
            model: t("model")?,
            fastboot_vid: u16_of("fastboot_vid")?,
            fastboot_pid: u16_of("fastboot_pid")?,
            ferros_vid: u16_of("ferros_vid")?,
            ferros_pid: u16_of("ferros_pid")?,
            block: u("block")?,
            lun: u("lun")?,
            slot_switch: s.flag("slot_switch"),
            boot_bytes: u("boot_bytes")?,
            send_piece: u("send_piece")?,
            userdata_floor: u("userdata_floor")?,
            ferros_min: u("ferros_min")?,
            ferros_first_lba: s.uint("ferros_first_lba").unwrap_or(0),
            ferros_last_lba: s.uint("ferros_last_lba").unwrap_or(0),
            backup: sections
                .iter()
                .filter(|s| s.name == BACKUP)
                .map(|s| {
                    s.text("part")
                        .map(str::to_string)
                        .ok_or("backup: no `part`")
                })
                .collect::<Result<_, _>>()?,
            firmware: sections
                .iter()
                .filter(|s| s.name == FIRMWARE)
                .map(|s| {
                    Ok(Firmware {
                        name: s.text("name").ok_or("firmware: no `name`")?.to_string(),
                        part: s.text("part").unwrap_or("").to_string(),
                        path: s.text("path").unwrap_or("").to_string(),
                        today: s
                            .uint("today")
                            .and_then(Source::from_u64)
                            .ok_or("firmware: no `today`")?,
                    })
                })
                .collect::<Result<_, String>>()?,
            traps: sections
                .iter()
                .filter(|s| s.name == TRAP)
                .map(|s| s.text("text").map(str::to_string).ok_or("trap: no `text`"))
                .collect::<Result<_, _>>()?,
        })
    }

    /// The userdata size the carve leaves on this disk: the floor, or what puts ferros exactly where today's kernel requires it (husky).
    pub fn userdata_bytes(&self, userdata_first_lba: u64) -> Result<u64, String> {
        if self.ferros_first_lba == 0 {
            return Ok(self.userdata_floor);
        }
        if self.ferros_first_lba <= userdata_first_lba {
            return Err(format!("userdata starts at LBA A#{userdata_first_lba}, past where this board's kernel needs ferros (A#{})", self.ferros_first_lba));
        }
        Ok((self.ferros_first_lba - userdata_first_lba) * self.block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_both_boards() {
        for b in Board::builtin() {
            let doc = b.to_vsf(vsf::eagle_time_oscillations()).unwrap();
            assert_eq!(&doc[0..3], "RÅ".as_bytes());
            assert_eq!(Board::from_vsf(&doc).unwrap(), b);
        }
    }

    #[test]
    fn product_guard() {
        assert_eq!(Board::for_product("akita").unwrap().model, "Pixel 8a");
        assert!(!Board::for_product("husky").unwrap().slot_switch);
        assert!(Board::for_product("shiba").is_none());
    }

    #[test]
    fn calibration_always_backed_up() {
        for b in Board::builtin() {
            for p in ["persist", "efs", "efs_backup", "modem_userdata", "devinfo"] {
                assert!(b.backup.iter().any(|x| x == p), "{} misses {p}", b.product);
            }
        }
    }

    #[test]
    fn husky_carve_lands_on_the_kernels_lbas() {
        let h = Board::husky();
        assert_eq!(
            h.userdata_bytes(28_000_000).unwrap(),
            (28_881_920 - 28_000_000) * 4096
        );
        assert!(h.userdata_bytes(29_000_000).is_err());
        assert_eq!(Board::akita().userdata_bytes(10_000_000).unwrap(), 1 << 30);
    }

    #[test]
    fn a_damaged_document_is_refused() {
        let doc = Board::akita().to_vsf(0).unwrap();
        assert!(Board::from_vsf(&doc[..doc.len() / 2]).is_err());
    }
}
