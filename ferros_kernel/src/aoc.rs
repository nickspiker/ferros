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

/// The AoC's DesignWare APB GPIO (version G#3231_332A, four ports of 32 pins, no hardware-control mode): the one page of the AoC block the AP may touch (Linux clears an interrupt bit here). Port n's data, direction and control registers at n·G#C (+0, +4, +8), the pins' levels at G#50 + n·4. The registry's `intr_id` numbers are these pins (43 the IMU's interrupt, 45 the light/proximity sensor's, 47 pressure, 56-59 the spectral sensor's).
pub const GPIO: usize = BLK + 0x01C0_0000;

/// The four ports' pin levels.
pub fn pins() -> [u32; 4] {
    core::array::from_fn(|n| unsafe { read_volatile((GPIO + 0x50 + 4 * n) as *const u32) })
}

/// Write one register of the AoC GPIO page (and nothing else).
pub fn gpio_write(off: usize, v: u32) -> bool {
    if off >= 0x1000 {
        return false;
    }
    unsafe { write_volatile((GPIO + off) as *mut u32, v) };
    true
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
const AOC_START: u32 = 1;

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
pub fn boot(get: &mut dyn FnMut(&str) -> Option<alloc::vec::Vec<u8>>, out: &mut alloc::vec::Vec<u8>) -> bool {
    use alloc::format;
    let say = |out: &mut alloc::vec::Vec<u8>, s: alloc::string::String| out.extend_from_slice(s.as_bytes());
    let f = crate::platform::cntfrq();
    let us = |t0: u64| (crate::platform::cntpct() - t0) * 1_000_000 / f;
    match crate::gsa::call(crate::gsa::CMD_AOC, &[crate::gsa::AOC_GET_STATE], 256) {
        Ok((_, 0, a)) if a.first() == Some(&0) => {}
        r => {
            say(out, format!("GSA AoC state is not inactive ({r:?}); not loading\n"));
            return false;
        }
    }
    // The pieces, in order, into the carve-out: the file's first 4 KiB is GSA's signed header, the rest the body at the load address.
    let t0 = crate::platform::cntpct();
    let (mut off, mut image_size) = (0usize, 0usize);
    for i in 0.. {
        let Some(piece) = get(&format!("aoc.fw.{i}")) else { break };
        let mut p: &[u8] = &piece;
        if off == 0 {
            if p.len() < AUTH_HEADER + 16 || u32::from_le_bytes(p[AUTH_HEADER..AUTH_HEADER + 4].try_into().unwrap()) != SUPERBIN_MAGIC {
                say(out, "aoc.fw.0 is not an AoC image (no superbin magic after the 4 KiB header)\n".into());
                return false;
            }
            image_size = u32::from_le_bytes(p[AUTH_HEADER + 12..AUTH_HEADER + 16].try_into().unwrap()) as usize;
            copy_dev(HDR_BUF, &p[..AUTH_HEADER]);
            p = &p[AUTH_HEADER..];
            off = AUTH_HEADER;
        }
        let at = off - AUTH_HEADER;
        if at + p.len() > image_size.max(1) + (16 << 20) || at + p.len() > HDR_BUF - CARVEOUT {
            say(out, format!("piece {i} runs past the carve-out's image area\n"));
            return false;
        }
        copy_dev(CARVEOUT + at, p);
        off += p.len();
    }
    unsafe { core::arch::asm!("dsb sy") };
    if off == 0 {
        say(out, "no aoc.fw.* in the vault (tools/husky-aoc-fw.sh puts them there)\n".into());
        return false;
    }
    say(out, format!("copied A#{} bytes (image size G#{image_size:X}) in A#{} us\n", off, us(t0)));
    let (ack, _) = request(true, 256);
    say(out, format!("aoc_req ack G#{ack:X}\n"));
    let t1 = crate::platform::cntpct();
    let r = crate::gsa::call(GSA_CMD_LOAD_AOC, &[HDR_BUF as u32, 0, CARVEOUT as u32, 0], 8192);
    say(out, format!("GSA load: {r:?} after A#{} us\n", us(t1)));
    match r {
        Ok((_, 0, _)) => {}
        _ => return false,
    }
    let ipc = CARVEOUT + image_size;
    parameters(ipc);
    unsafe { core::arch::asm!("dsb sy") };
    let t2 = crate::platform::cntpct();
    let r = crate::gsa::call(crate::gsa::CMD_AOC, &[AOC_START], 2048);
    say(out, format!("GSA start: {r:?}\n"));
    // Up when the firmware has replaced the parameter block with its control block.
    while us(t2) < 4_000_000 {
        if rd32(ipc) == AOC_MAGIC {
            break;
        }
    }
    let up = rd32(ipc) == AOC_MAGIC;
    say(out, format!("control block after A#{} us: magic G#{:X} fw G#{:X} hw G#{:X} status G#{:X} services A#{}\n", us(t2), rd32(ipc), rd32(ipc + 4), rd32(ipc + 8), rd32(ipc + 12), rd32(ipc + 24)));
    up
}
