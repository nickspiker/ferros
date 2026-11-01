//! The AoC block from the AP's side, AoC kept in reset: its power request and the peripherals its firmware drives (SENSORS.md "The direct route"). No AoC firmware, no GSA, no Trusty.
//!
//! The AoC's own peripheral table (aoc.bin's SRAM image) gives each controller's base in the AoC's address space; the AP sees the same registers at that base + G#1700_0000 (`blk_aoc`, 32 MiB, zuma-aoc.dtsi). The sensors' wiring is in the board's registry script (husky_common.reg / husky_dvt.reg): i2c0 carries the light/proximity, one magnetometer, pressure, spectral and infrared sensors, i3c0 the IMU and the other magnetometer.

use core::ptr::{read_volatile, write_volatile};

/// The AoC power request (`aoc_req`, zuma-aoc.dtsi): write 1 to ask for the block, its acknowledge reads at +G#40 (aoc_v1.c `request_aoc_on` / `wait_for_aoc_status`).
pub const AOC_REQ: usize = 0x154B_0000;
/// AP address of AoC-local address 0.
pub const BLK: usize = 0x1700_0000;

/// The AoC firmware's peripheral table, AoC-local bases (aoc.bin build 14591378): name, base, and the registers worth reading to identify the block.
pub const BLOCKS: [(&str, usize, &[usize]); 16] = [
    ("SRAM", 0x0000_0000, &[0x0]),
    ("GPIO", 0x01C0_0000, &[0x0, 0x4]),
    ("UART0", 0x01A0_0000, &[0x0]),
    ("I2C0", 0x01A0_6000, &[0xFC, 0xF8, 0xF4, 0x0, 0x4, 0x6C, 0x70]),
    ("I2C1", 0x01A0_7000, &[0xFC, 0x0]),
    ("I2C2", 0x01A0_8000, &[0xFC, 0x0]),
    ("I2C3", 0x01A0_9000, &[0xFC, 0x0]),
    ("SPI0", 0x01A0_A000, &[0x0]),
    ("I3C_SYNOPS0", 0x01A0_F000, &[0xE0, 0xE4, 0x0, 0x4]),
    ("I3C_SYNOPS1", 0x01A1_0000, &[0xE0, 0xE4, 0x0]),
    ("I3C_SYNOPS2", 0x01A1_1000, &[0xE0, 0xE4, 0x0]),
    ("I3C_SYNOPS3", 0x01A1_2000, &[0xE0, 0xE4, 0x0]),
    ("I3C_SYNOPS4", 0x01A1_3000, &[0xE0, 0xE4, 0x0]),
    ("I3C_SILV_Q0", 0x01C0_A000, &[0x0, 0x4, 0x154, 0x158]),
    ("I3C_SILV_Q1", 0x01C0_B000, &[0x0, 0x154]),
    ("DMA0", 0x01B0_0000, &[0x0]),
];

/// Read a register and say whether the read faulted: SError unmasked around the load and synchronised (dsb + isb) so an external abort lands here, where the SError vector counts it. A synchronous external abort still goes to the fault sink (a reset with a report), a read that never completes to the watchdog.
pub fn probe32(a: usize) -> Option<u32> {
    let before = crate::exception_count();
    let v: u32;
    unsafe {
        core::arch::asm!(
            "mrs {d}, daif",
            "msr daifclr, #4",
            "ldr {v:w}, [{a}]",
            "dsb sy",
            "isb",
            "msr daif, {d}",
            a = in(reg) a, v = out(reg) v, d = out(reg) _,
        );
    }
    if crate::exception_count() != before { None } else { Some(v) }
}

pub fn req() -> (u32, u32) {
    unsafe { (read_volatile(AOC_REQ as *const u32), read_volatile((AOC_REQ + 0x40) as *const u32)) }
}

/// Ask for (or release) the AoC block and wait up to `ms` for the acknowledge to follow; returns the acknowledge seen and the microseconds it took.
pub fn request(on: bool, ms: u64) -> (u32, u64) {
    let f = crate::platform::cntfrq();
    let t0 = crate::platform::cntpct();
    unsafe { write_volatile(AOC_REQ as *mut u32, on as u32) };
    loop {
        let ack = unsafe { read_volatile((AOC_REQ + 0x40) as *const u32) };
        let dt = crate::platform::cntpct() - t0;
        if (ack & 1 != 0) == on || dt > f / 1000 * ms {
            return (ack, dt * 1_000_000 / f);
        }
    }
}

/// `diag aoc`: the request and acknowledge, then (only with the acknowledge up, a powered-down block hangs the core) every block's identifying registers.
pub fn report(out: &mut alloc::vec::Vec<u8>) {
    let (r, a) = req();
    out.extend_from_slice(alloc::format!("aoc_req G#{r:X}, ack G#{a:X}\n").as_bytes());
    if a & 1 == 0 {
        out.extend_from_slice(b"AoC block not acknowledged: no peripheral reads (`diag aoc on` asks for it)\n");
        return;
    }
    for (name, base, regs) in BLOCKS {
        let mut line = alloc::format!("{name:>12} G#{:X}:", BLK + base);
        for &o in regs {
            match probe32(BLK + base + o) {
                Some(v) => line.push_str(&alloc::format!(" +{o:X}=G#{v:X}")),
                None => line.push_str(&alloc::format!(" +{o:X}=ABORT")),
            }
        }
        line.push('\n');
        out.extend_from_slice(line.as_bytes());
    }
}

// ── Starting Google's AoC firmware through GSA's mailbox (no Trusty) ──
// The sequence is Linux's (`aoc_fw_callback`, google-modules/aoc aoc.c) with GSA's AoC commands sent on the non-secure mailbox (gsa.rs) instead of over Trusty IPC: GSA answers GSA_AOC_GET_STATE there on this firmware (2026-10-23). The firmware is the phone's own `/vendor/firmware/aoc.bin`, stored in the vault as `aoc.fw.0`, `aoc.fw.1`, ... (2 MiB pieces, tools/husky-aoc-fw.sh); nothing of it is in the repo.

/// The AoC's DRAM carve-out (`aoc_reserve`, zuma-rmem.dtsi): 48 MiB, which the AoC sees at G#9800_0000. The image body loads at its start, the IPC control block follows the image (at `image_size`), the sensor/playback/capture heaps sit at its top.
pub const CARVEOUT: usize = 0x9400_0000;
pub const CARVEOUT_SIZE: usize = 0x0300_0000;
const AOC_VIEW: usize = 0x9800_0000;
/// Where GSA reads the 4 KiB signed header from: inside the carve-out, above the image and the IPC area, below the heaps; discardable once the load returns.
const HDR_BUF: usize = 0x96B0_0000;
const AUTH_HEADER: usize = 4096;
const SUPERBIN_MAGIC: u32 = 0xAABB_CCDD;
const PARAMETER_MAGIC: u32 = 0x0A0C_DA7A;
const AOC_MAGIC: u32 = 0xA0C0_0A0C;
const GSA_CMD_LOAD_AOC: u32 = 90;

fn wr32(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}
fn rd32(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}

/// Copy bytes into Device memory with aligned 64-bit writes (the carve-out is mapped Device-nGnRnE: no cache maintenance, no unaligned access).
fn copy_dev(dst: usize, src: &[u8]) {
    let mut i = 0;
    while i + 8 <= src.len() {
        let v = u64::from_le_bytes(src[i..i + 8].try_into().unwrap());
        unsafe { write_volatile((dst + i) as *mut u64, v) };
        i += 8;
    }
    while i < src.len() {
        unsafe { write_volatile((dst + i) as *mut u8, src[i]) };
        i += 1;
    }
}

/// The firmware's parameter block (`aoc_pass_fw_information`), written at the IPC offset before the AoC starts: magic, count, total size, then (key, 4, value) triplets. Values as Linux passes them for husky (zuma-husky-audio.dtsi board HK3, id G#40202 rev G#10000; chip id registers at G#1000_0000).
fn parameters(ipc: usize) {
    let heap_top = CARVEOUT + CARVEOUT_SIZE;
    let sensor = heap_top - (4 << 20);
    let playback = sensor - (16 << 10);
    let capture = playback - (64 << 10);
    let to_aoc = |a: usize| (a - CARVEOUT + AOC_VIEW) as u32;
    let product = rd32(0x1000_0000);
    let rev = rd32(0x1000_0010);
    let revision = (((rev >> 20) & 0xF) << 4) | ((rev >> 16) & 0xF);
    let seed = crate::platform::cntpct() as u32;
    let fw: [(u32, u32); 22] = [
        (0x1001, 0x40202),             // board id
        (0x1002, 0x10000),             // board revision
        (0x1003, 0),                   // SRAM repaired (Linux: always false)
        (0x1005, CARVEOUT as u32),     // carve-out address (AP physical)
        (0x1006, CARVEOUT_SIZE as u32),
        (0x1007, to_aoc(sensor)),      // sensor direct heap (AoC address)
        (0x1008, 4 << 20),
        (0x100C, to_aoc(playback)),
        (0x100D, 16 << 10),
        (0x100E, to_aoc(capture)),
        (0x100F, 64 << 10),
        (0x1009, 0),                   // force VNOM
        (0x100A, 0),                   // disable monitor mode
        (0x100B, 0),                   // UART logging
        (0x1010, 0),                   // force speaker ultrasonic
        (0x1011, seed),                // random seed
        (0x1012, revision),            // chip revision
        (0x1013, product & 0xF),       // chip type
        (0x1014, 0),                   // GNSS type (not in husky's DT)
        (0x1015, 0),                   // VoLTE release MIF
        (0x1016, product & 0xFFFF_F000), // chip product id
        (0x1017, 0),                   // WiFi chip (not in husky's DT)
    ];
    wr32(ipc, PARAMETER_MAGIC);
    wr32(ipc + 4, fw.len() as u32);
    wr32(ipc + 8, 12 + fw.len() as u32 * 12);
    for (i, (k, v)) in fw.iter().enumerate() {
        wr32(ipc + 12 + i * 12, *k);
        wr32(ipc + 16 + i * 12, 4);
        wr32(ipc + 20 + i * 12, *v);
    }
}

/// `diag aoc boot`: load and start the AoC from the vault's copy of aoc.bin. `get` reads one vault key. Returns whether the AoC came up (its control block magic).
/// GSA's view of the AoC: 0 inactive, 1 loaded, 2 running (None: GSA did not answer).
pub fn gsa_state() -> Option<u32> {
    match crate::gsa::call(crate::gsa::CMD_AOC, &[crate::gsa::AOC_GET_STATE], 256) {
        Ok((_, 0, a)) => a.first().map(|&s| s as u32),
        _ => None,
    }
}

/// Loading the firmware a piece at a time, so the service loop can go on between pieces (each 2 MiB piece is some 50 ms out of the vault; the whole image 0.7 s, too long for the X3 to stop serving USB and drawing).
pub struct Loader {
    pub next: usize,
    off: usize,
    image_size: usize,
}

impl Loader {
    /// Begin, if GSA has the AoC inactive.
    pub fn begin(out: &mut alloc::vec::Vec<u8>) -> Option<Loader> {
        match gsa_state() {
            Some(0) => Some(Loader { next: 0, off: 0, image_size: 0 }),
            s => {
                out.extend_from_slice(alloc::format!("GSA AoC state is not inactive ({s:?}); not loading\n").as_bytes());
                None
            }
        }
    }

    /// Copy the next piece into the carve-out: Ok(true) when one was copied, Ok(false) when there are no more. The file's first 4 KiB is GSA's signed header, the rest the body at the load address.
    pub fn piece(&mut self, get: &mut dyn FnMut(&str) -> Option<alloc::vec::Vec<u8>>, out: &mut alloc::vec::Vec<u8>) -> Result<bool, ()> {
        let i = self.next;
        let Some(piece) = get(&alloc::format!("aoc.fw.{i}")) else {
            if self.off == 0 {
                out.extend_from_slice(b"no aoc.fw.* in the vault (tools/husky-aoc-fw.sh puts them there)\n");
                return Err(());
            }
            return Ok(false);
        };
        let mut p: &[u8] = &piece;
        if self.off == 0 {
            if p.len() < AUTH_HEADER + 16 || u32::from_le_bytes(p[AUTH_HEADER..AUTH_HEADER + 4].try_into().unwrap()) != SUPERBIN_MAGIC {
                out.extend_from_slice(b"aoc.fw.0 is not an AoC image (no superbin magic after the 4 KiB header)\n");
                return Err(());
            }
            self.image_size = u32::from_le_bytes(p[AUTH_HEADER + 12..AUTH_HEADER + 16].try_into().unwrap()) as usize;
            copy_dev(HDR_BUF, &p[..AUTH_HEADER]);
            p = &p[AUTH_HEADER..];
            self.off = AUTH_HEADER;
        }
        let at = self.off - AUTH_HEADER;
        if at + p.len() > self.image_size.max(1) + (16 << 20) || at + p.len() > HDR_BUF - CARVEOUT {
            out.extend_from_slice(alloc::format!("piece {i} runs past the carve-out's image area\n").as_bytes());
            return Err(());
        }
        copy_dev(CARVEOUT + at, p);
        self.off += p.len();
        self.next += 1;
        Ok(true)
    }

    /// With every piece in: the AoC SYSMMU from the firmware's own IOMMU table (now in HDR_BUF), the block's power request, GSA's load (it authenticates the image), the parameter block. The start then goes over Trusty (trusty::aoc_start), as Linux does: GSA's mailbox command 91 moves the state but does not run the firmware, and leaves GSA in a state the Trusty start then refuses.
    pub fn finish(&self, out: &mut alloc::vec::Vec<u8>) -> bool {
        let f = crate::platform::cntfrq();
        unsafe { core::arch::asm!("dsb sy") };
        out.extend_from_slice(alloc::format!("copied A#{} bytes (image size G#{:X})\n", self.off, self.image_size).as_bytes());
        mmu(out);
        let (ack, _) = request(true, 256);
        out.extend_from_slice(alloc::format!("aoc_req ack G#{ack:X}\n").as_bytes());
        let t1 = crate::platform::cntpct();
        let r = crate::gsa::call(GSA_CMD_LOAD_AOC, &[HDR_BUF as u32, 0, CARVEOUT as u32, 0], 8192);
        out.extend_from_slice(alloc::format!("GSA load: {r:?} after A#{} us\n", (crate::platform::cntpct() - t1) * 1_000_000 / f).as_bytes());
        if !matches!(r, Ok((_, 0, _))) {
            return false;
        }
        parameters(CARVEOUT + self.image_size);
        unsafe { core::arch::asm!("dsb sy") };
        out.extend_from_slice(alloc::format!("loaded; GSA AoC state {:?} (1 = loaded)\n", gsa_state()).as_bytes());
        true
    }
}

/// Whether the firmware's control block is up (its magic in place of the parameter block).
pub fn online() -> bool {
    rd32(IPC) == AOC_MAGIC
}

/// The AoC's control block: up once the firmware has replaced the parameter block with it (magic G#A0C0_0A0C). Polls up to `ms`; returns whether it came up and the first words.
pub fn wait_online(ms: u64) -> (bool, [u32; 7]) {
    let ipc = CARVEOUT + 0x1C0_0000;
    let f = crate::platform::cntfrq();
    let t0 = crate::platform::cntpct();
    while crate::platform::cntpct() - t0 < f / 1000 * ms {
        if rd32(ipc) == AOC_MAGIC {
            break;
        }
    }
    (rd32(ipc) == AOC_MAGIC, [rd32(ipc), rd32(ipc + 4), rd32(ipc + 8), rd32(ipc + 12), rd32(ipc + 16), rd32(ipc + 20), rd32(ipc + 24)])
}

// ── The AoC's SYSMMU (sysmmu@19090000, v9.0), as Linux enables it before the load ──
/// Linux's writes to the AoC SYSMMU's non-secure side, in order, from the hv oracle of Android's bring-up (2026-10-23, SENSORS.md): VM 0's page-table base (G#1909_8404, replaced here by ferros's table) and attributes, the default stream and 37 stream entries, then VM 0 enabled (G#24 → G#25) and G#1 at G#1909_8010.
const MMU_WRITES: [(usize, u32); 122] = [
    (0x1909_8404, 0x0008_C880),
    (0x1909_8408, 0x0002_0F84),
    (0x1909_2FFC, 0x0000_0000),
    (0x1909_2000, 0x0000_010B),
    (0x1909_2010, 0x0100_010B),
    (0x1909_2014, 0x0000_0301),
    (0x1909_2018, 0x0000_0002),
    (0x1909_201C, 0x0000_003F),
    (0x1909_2020, 0x0000_010B),
    (0x1909_2024, 0x0000_0301),
    (0x1909_2028, 0x0000_0003),
    (0x1909_202C, 0x0000_003F),
    (0x1909_2030, 0x0000_010B),
    (0x1909_2034, 0x0000_0301),
    (0x1909_2038, 0x0000_0004),
    (0x1909_203C, 0x0000_003F),
    (0x1909_2040, 0x0000_010B),
    (0x1909_2044, 0x0000_0301),
    (0x1909_2048, 0x0000_0005),
    (0x1909_204C, 0x0000_003F),
    (0x1909_2050, 0x0000_010B),
    (0x1909_2054, 0x0000_0301),
    (0x1909_2058, 0x0000_0006),
    (0x1909_205C, 0x0000_003F),
    (0x1909_2060, 0x0000_010B),
    (0x1909_2064, 0x0000_0301),
    (0x1909_2068, 0x0000_0009),
    (0x1909_206C, 0x0000_003F),
    (0x1909_2070, 0x0000_010B),
    (0x1909_2074, 0x0000_0301),
    (0x1909_2078, 0x0000_000A),
    (0x1909_207C, 0x0000_003F),
    (0x1909_2080, 0x0000_010B),
    (0x1909_2084, 0x0000_0301),
    (0x1909_2088, 0x0000_000B),
    (0x1909_208C, 0x0000_003F),
    (0x1909_2090, 0x0000_010B),
    (0x1909_2094, 0x0000_0301),
    (0x1909_2098, 0x0000_000C),
    (0x1909_209C, 0x0000_003F),
    (0x1909_20A0, 0x0000_010B),
    (0x1909_20A4, 0x0000_0301),
    (0x1909_20A8, 0x0000_000D),
    (0x1909_20AC, 0x0000_003F),
    (0x1909_20B0, 0x0000_010B),
    (0x1909_20B4, 0x0000_0301),
    (0x1909_20B8, 0x0000_000E),
    (0x1909_20BC, 0x0000_003F),
    (0x1909_20C0, 0x0000_010B),
    (0x1909_20C4, 0x0000_0301),
    (0x1909_20C8, 0x0000_0020),
    (0x1909_20CC, 0x0000_003F),
    (0x1909_20D0, 0x0000_010B),
    (0x1909_20D4, 0x0000_0301),
    (0x1909_20D8, 0x0000_0021),
    (0x1909_20DC, 0x0000_003F),
    (0x1909_20E0, 0x0000_010B),
    (0x1909_20E4, 0x0000_0301),
    (0x1909_20E8, 0x0000_0022),
    (0x1909_20EC, 0x0000_003F),
    (0x1909_20F0, 0x0000_010B),
    (0x1909_20F4, 0x0000_0301),
    (0x1909_20F8, 0x0000_0023),
    (0x1909_20FC, 0x0000_003F),
    (0x1909_2100, 0x0000_010B),
    (0x1909_2104, 0x0000_0301),
    (0x1909_2108, 0x0000_0024),
    (0x1909_210C, 0x0000_003F),
    (0x1909_2110, 0x0000_010B),
    (0x1909_2114, 0x0000_0301),
    (0x1909_2118, 0x0000_0025),
    (0x1909_211C, 0x0000_003F),
    (0x1909_2120, 0x0000_010B),
    (0x1909_2124, 0x0000_0301),
    (0x1909_2128, 0x0000_0026),
    (0x1909_212C, 0x0000_003F),
    (0x1909_2130, 0x0000_010B),
    (0x1909_2134, 0x0000_0301),
    (0x1909_2138, 0x0000_0027),
    (0x1909_213C, 0x0000_003F),
    (0x1909_2140, 0x0000_010B),
    (0x1909_2144, 0x0000_0301),
    (0x1909_2148, 0x0000_0028),
    (0x1909_214C, 0x0000_003F),
    (0x1909_2150, 0x0000_010B),
    (0x1909_2154, 0x0000_0301),
    (0x1909_2158, 0x0000_0030),
    (0x1909_215C, 0x0000_003F),
    (0x1909_2160, 0x0000_010B),
    (0x1909_2164, 0x0000_0301),
    (0x1909_2168, 0x0000_0031),
    (0x1909_216C, 0x0000_003F),
    (0x1909_2170, 0x0000_010B),
    (0x1909_2174, 0x0000_0301),
    (0x1909_2178, 0x0000_0032),
    (0x1909_217C, 0x0000_003F),
    (0x1909_2180, 0x0000_010B),
    (0x1909_2184, 0x0000_0301),
    (0x1909_2188, 0x0000_0033),
    (0x1909_218C, 0x0000_003F),
    (0x1909_2190, 0x0000_010B),
    (0x1909_2194, 0x0000_0301),
    (0x1909_2198, 0x0000_0034),
    (0x1909_219C, 0x0000_003F),
    (0x1909_21A0, 0x0000_010B),
    (0x1909_21A4, 0x0000_0301),
    (0x1909_21A8, 0x0000_0035),
    (0x1909_21AC, 0x0000_003F),
    (0x1909_21B0, 0x0000_010B),
    (0x1909_21B4, 0x0000_0301),
    (0x1909_21B8, 0x0000_0036),
    (0x1909_21BC, 0x0000_003F),
    (0x1909_21C0, 0x0000_010B),
    (0x1909_21C4, 0x0000_0301),
    (0x1909_21C8, 0x0000_0037),
    (0x1909_21CC, 0x0000_003F),
    (0x1909_21D0, 0x0000_010B),
    (0x1909_21D4, 0x0000_0301),
    (0x1909_21D8, 0x0000_0038),
    (0x1909_21DC, 0x0000_003F),
    (0x1909_8000, 0x0000_0025),
    (0x1909_8010, 0x0000_0001),
];
/// ferros's first-level table for the AoC (4096 entries of 1 MiB, 16 KiB aligned), in ferros's own non-cacheable DMA memory: not in the carve-out, which GSA locks on load (a table there read back as noise, 2026-10-23). Linux keeps its table in ordinary kernel memory too.
#[repr(C, align(16384))]
struct L1([u32; 4096]);
#[unsafe(link_section = ".bss.dma")]
static mut AOC_L1: L1 = L1([0; 4096]);
fn mmu_table() -> usize {
    (&raw const AOC_L1) as usize
}

/// The firmware's own IOMMU table, from its signed header (aoc_firmware.c `_aoc_fw_iommu_entry`): image_config at header offset G#C0+, the table at image_config + iommu_offset; each 64-bit entry is iova[19:0]<<12, pa[43:20]<<12, size[63:44]<<12. For husky's aoc.bin: the DRAM window G#9800_0000 (48 MiB) onto the carve-out, then the mailboxes, GPIO, ACPM and alive/CMU windows the firmware reaches.
fn fw_iommu_entries() -> alloc::vec::Vec<(usize, usize, usize)> {
    let mut v = alloc::vec::Vec::new();
    // image_config sits in the 4 KiB signed header copied to HDR_BUF, at offset G#580 (header v2: 512+512 signature/key, then magic/gen/rollback/length G#10, flags G#10, hash G#40, chip_id G#20, auth_config G#100 → image_config). Its layout: version, fw_start, fw_size, privilege_level (4 bytes each), then bl_offset, bl_size, iommu_offset, iommu_size (2 bytes each). So the word at +20 is iommu_size<<16 | iommu_offset.
    let cfg = HDR_BUF + 0x580;
    let w = rd32(cfg + 20);
    let iommu_off = (w & 0xFFFF) as usize;
    let iommu_size = (w >> 16) as usize;
    let base = cfg + iommu_off;
    for k in 0..(iommu_size / 8) {
        let e = unsafe { read_volatile((base + 8 * k) as *const u64) };
        let iova = ((e & 0xFFFFF) << 12) as usize;
        let pa = (((e >> 20) & 0xFFFFFF) << 12) as usize;
        let size = (((e >> 44) & 0xFFFFF) << 12) as usize;
        if size != 0 {
            v.push((iova, pa, size));
        }
    }
    v
}

/// `diag aoc mmu`: build the AoC's table from the firmware's own IOMMU entries as 1 MiB sections (entry = PA >> 4 | 2 | read | write, samsung-iommu-v9.c), then replay Linux's enable with ferros's table base.
pub fn mmu(out: &mut alloc::vec::Vec<u8>) {
    for i in 0..4096 {
        wr32(mmu_table() + 4 * i, 0);
    }
    // v9 sections carry their permissions: bit 4 read, bit 5 write (samsung-iommu-v9.c FLPD_AP_*); without them every access is an access fault (2026-10-23: status G#4).
    let sect = |iova: usize, pa: usize| wr32(mmu_table() + 4 * (iova >> 20), ((pa >> 4) as u32) | 2 | 0x30);
    for (iova, pa, size) in fw_iommu_entries() {
        for off in (0..size).step_by(1 << 20) {
            sect(iova + off, pa + off);
        }
        out.extend_from_slice(alloc::format!("  map iova G#{iova:X} pa G#{pa:X} size G#{size:X}\n").as_bytes());
    }
    unsafe { core::arch::asm!("dsb sy") };
    // Clear a latched fault (FAULT_CLEAR_VM) before enabling again.
    wr32(0x1909_8064, rd32(0x1909_8060));
    for &(a, v) in MMU_WRITES.iter() {
        let v = if a == 0x1909_8404 { (mmu_table() >> 12) as u32 } else { v };
        wr32(a, v);
    }
    unsafe { core::arch::asm!("dsb sy") };
    out.extend_from_slice(alloc::format!("AoC SYSMMU: table G#{:X}, VM0 ctrl G#{:X}, flpt G#{:X}\n", mmu_table(), rd32(0x1909_8000), rd32(0x1909_8404)).as_bytes());
}

// ── The AoC service rings (aoc_ipc, google-modules/aoc_ipc) ──
/// The IPC control block and service table live at the carve-out plus the image size. The image is G#1C0_0000 for husky's aoc.bin (the superbin header's image_size).
pub const IPC: usize = CARVEOUT + 0x1C0_0000;

// ── Talking to an AoC queue service (aoc_ipc write/read + mailbox signal) ──
// The carve-out is Device memory in ferros's map (uncached), so queue reads/writes need no cache maintenance; a dsb orders the payload before the tx bump and the mailbox kick.

/// The sensors' two rails on the sub PMIC (zuma-common.dtsi, the AoC's `sensor_power_list`): LDO7S 1.8 V (L7S_SENSORS_FPS) and LDO5S 3.3 V (L5S_PROX), control registers G#35 and G#33 (s2mpg15-register.h); bit 7 enables. Both are boot-on in the device tree.
pub const RAILS: [(&str, u32); 2] = [("LDO7S 1.8 V", 0x35), ("LDO5S 3.3 V", 0x33)];

/// Turn the sensors' rails on, as Linux's AoC driver does at probe, before the firmware starts (aoc.c reset_sensor_power): the firmware finds no sensors on dark rails, and its sensor list comes back empty (2026-10-24). Moved here from usf.rs with the AoC's start (2026-11-01).
pub fn rails(out: &mut alloc::vec::Vec<u8>) {
    for (name, reg) in RAILS {
        let r = crate::acpm::update(crate::acpm::SUB, crate::acpm::T_PMIC, reg, 0x80, 0x80).and_then(|_| crate::acpm::read(crate::acpm::SUB, crate::acpm::T_PMIC, reg));
        out.extend_from_slice(alloc::format!("{name} on: {:?}\n", r.map(|c| alloc::format!("G#{c:02X}"))).as_bytes());
    }
}
