//! ACPM IPC client for the PMIC channel: how an EL2 kernel reaches the S2MPG14 (main) and S2MPG15 (sub) PMICs, which have no AP-side bus (PMIC.md).
//!
//! The APM firmware owns the PMICs' bus; the AP talks to it through queues in the APM SRAM and a mailbox doorbell.
//! Everything here mirrors the kernel's `acpm_ipc.c` (`channel_init`, `__acpm_ipc_send_data`, `check_response`): the firmware's descriptor table at SRAM + G#A000 gives each channel's queue as offsets from the SRAM base, the AP transmits on the firmware's `rx_*` queue and receives on its `tx_*` queue, and a reply is matched by the sequence number in bits [21:16] of word 0.
//! ferros is the only client (no Linux), so stale replies are dropped and every transaction is polled to completion.

use core::ptr::{read_volatile, write_volatile};

/// APM SRAM as the AP sees it, and its size (zuma.dtsi). The first G#5000 is treated as off limits: only offsets the descriptors give are touched.
const SRAM: usize = 0x1570_0000;
const SRAM_SIZE: usize = 0x5_8000;
/// `initdata-base` in the device tree: the `acpm_framework` descriptor.
const INITDATA: usize = SRAM + 0xA000;
/// AP-to-APM mailbox: INTGR1 rings the firmware, INTCR0 clears the AP's pending bit.
const MBOX: usize = 0x1511_0000;
const INTGR1: usize = 0x40;
const INTCR0: usize = 0x24;
/// The PMIC channel's index in the AP's channel table (`acpm-ipc-channel = <2>` on the PMIC nodes).
pub const PMIC_CHANNEL: usize = 2;
/// One `ipc_channel` descriptor: id, flags, owner, type, `channel_info` (13 words), `ap_poll`.
const DESC_SIZE: usize = 72;
const SEQ_SHIFT: u32 = 16;

/// SPEEDY channels (PMIC.md): S2MPG14 is 0, S2MPG15 is 1.
pub const MAIN: u32 = 0;
pub const SUB: u32 = 1;
/// Register types inside a PMIC (the `I2C_ADDR_*` blocks of the s2mpg1x register headers).
pub const T_TOP: u32 = 0x0;
pub const T_PMIC: u32 = 0x1;
pub const T_GPIO: u32 = 0xC;

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}

/// One AP-side channel, resolved to absolute addresses.
#[derive(Clone, Copy)]
pub struct Channel {
    pub id: u32,
    pub poll: u32,
    pub len: u32,
    pub size: u32,
    tx_front: usize,
    tx_rear: usize,
    tx_base: usize,
    rx_front: usize,
    rx_rear: usize,
    rx_base: usize,
}

fn in_sram(off: u32) -> Option<usize> {
    let off = off as usize;
    if (0x5000..SRAM_SIZE).contains(&off) { Some(SRAM + off) } else { None }
}

/// Resolve the AP's channel `idx` from the firmware's descriptors, refusing anything that points outside the SRAM or describes an implausible queue.
pub fn channel(idx: usize) -> Result<Channel, &'static str> {
    let ipc_channels = rd(INITDATA + 0x8);
    let ap_max = rd(INITDATA + 0x18);
    if idx as u32 >= ap_max {
        return Err("channel index beyond ipc_ap_max");
    }
    let d = in_sram(ipc_channels).ok_or("ipc_channels outside SRAM")? + idx * DESC_SIZE;
    let w = |i: usize| rd(d + i * 4);
    // channel_info starts at word 4: rx_rear, rx_front, rx_base, rx_indr_buf, rx_indr_buf_base, rx_indr_buf_size, tx_rear, tx_front, tx_base, q_len, q_elem_size, credit, mbox_addr; ap_poll is word 17.
    let ch = Channel {
        id: w(0),
        poll: w(17),
        len: w(13),
        size: w(14),
        tx_rear: in_sram(w(4)).ok_or("rx_rear outside SRAM")?,
        tx_front: in_sram(w(5)).ok_or("rx_front outside SRAM")?,
        tx_base: in_sram(w(6)).ok_or("rx_base outside SRAM")?,
        rx_rear: in_sram(w(10)).ok_or("tx_rear outside SRAM")?,
        rx_front: in_sram(w(11)).ok_or("tx_front outside SRAM")?,
        rx_base: in_sram(w(12)).ok_or("tx_base outside SRAM")?,
    };
    if ch.len < 3 || ch.len > 256 || ch.size < 16 || ch.size > 64 || ch.size % 4 != 0 || ch.id > 31 {
        return Err("implausible queue geometry");
    }
    Ok(ch)
}

/// A one-line description of a channel and its queue indices, for DIAG.
pub fn describe(idx: usize) -> alloc::string::String {
    match channel(idx) {
        Err(e) => alloc::format!("ACPM ch{idx}: {e}\n"),
        Ok(c) => alloc::format!(
            "ACPM ch{idx}: id {} poll {} len {} elem {} B; ap tx front {} rear {}; ap rx front {} rear {}\n",
            c.id, c.poll, c.len, c.size, rd(c.tx_front), rd(c.tx_rear), rd(c.rx_front), rd(c.rx_rear)
        ),
    }
}

static mut SEQ: u32 = 0;

/// Error codes: 1 queue full, 2 no reply in time, 3 the firmware returned a failure (its code in bits [15:8]), 4 bad channel.
pub const E_FULL: u32 = 1;
pub const E_TIMEOUT: u32 = 2;
pub const E_FW: u32 = 3;
pub const E_CHANNEL: u32 = 4;

/// One transaction on channel `idx`: `cmd` goes out with this call's sequence number in word 0's bits [21:16] (as `__acpm_ipc_send_data` stamps it); with `reply`, the firmware's answer (the four words of the matching reply) comes back, otherwise the call returns once the firmware has the request.
pub fn call(idx: usize, mut cmd: [u32; 4], reply: bool) -> Result<[u32; 4], u32> {
    let c = channel(idx).map_err(|_| E_CHANNEL)?;
    let freq = crate::platform::cntfrq();
    let deadline = |ms: u64| crate::platform::cntpct() + freq / 1000 * ms;
    // Wait for room exactly as the kernel does: the AP's tx front two ahead of its rx front means the firmware is behind.
    let t = deadline(10);
    let mut front = rd(c.tx_front);
    while (front + 2) % c.len == rd(c.rx_front) {
        if crate::platform::cntpct() > t {
            return Err(E_FULL);
        }
        front = rd(c.tx_front);
    }
    if (front + 1) % c.len == rd(c.tx_rear) {
        return Err(E_FULL);
    }
    let seq = unsafe {
        SEQ = if SEQ >= 63 { 1 } else { SEQ + 1 };
        SEQ
    };
    cmd[0] = (cmd[0] & !(0x3F << SEQ_SHIFT)) | seq << SEQ_SHIFT;
    let slot = c.tx_base + (c.size as usize) * front as usize;
    for (i, &w) in cmd.iter().enumerate() {
        wr(slot + i * 4, w);
    }
    for i in cmd.len()..(c.size as usize / 4) {
        wr(slot + i * 4, 0);
    }
    unsafe { core::arch::asm!("dsb sy") };
    wr(c.tx_front, (front + 1) % c.len);
    wr(MBOX + INTGR1, 1 << c.id);
    if !reply {
        return Ok([0; 4]);
    }
    // Poll the reply queue; drop anything that is not ours (ferros is the only client).
    let t = deadline(100);
    loop {
        let rx_front = rd(c.rx_front);
        let mut i = rd(c.rx_rear);
        let mut found = None;
        while i != rx_front {
            let e = c.rx_base + (c.size as usize) * i as usize;
            if (rd(e) >> SEQ_SHIFT) & 0x3F == seq {
                found = Some([rd(e), rd(e + 4), rd(e + 8), rd(e + 12)]);
            }
            i = (i + 1) % c.len;
        }
        wr(c.rx_rear, rx_front);
        wr(MBOX + INTCR0, 1 << c.id);
        if let Some(r) = found {
            return Ok(r);
        }
        if crate::platform::cntpct() > t {
            return Err(E_TIMEOUT);
        }
    }
}

/// One PMIC transaction on the PMIC channel: `func` 0 read, 1 write, 2 update (value under mask). Returns the reply's word 1.
fn transact(speedy: u32, typ: u32, reg: u32, func: u32, val: u32, mask: u32) -> Result<u32, u32> {
    let cmd = [(reg & 0xFF) | (typ & 0xF) << 8 | (speedy & 0xF) << 12, (func & 0xFF) | (val & 0xFF) << 8 | (mask & 0xFF) << 16, 0, 0];
    let w1 = call(PMIC_CHANNEL, cmd, true)?[1];
    let code = w1 >> 24;
    if code == 0 { Ok(w1) } else { Err(E_FW | code << 8) }
}

/// The DVFS channel (`acpm-ipc-channel = <0>` on zuma's `cal_if` node, which acpm_dvfs.c borrows): rates for MIF, INT, the three CPU clusters, the DSU, the GPU and the rest, by the index of zuma's `acpm_dvfs_id` (cal-if/zuma/acpm_dvfs_zuma.h).
pub const DVFS_CHANNEL: usize = 0;
/// zuma's DVFS domains in `acpm_dvfs_id` order (ACPM_VCLK_TYPE + index).
pub const DVFS_DOMAINS: [&str; 17] = ["MIF", "INT", "CPUCL0", "CPUCL1", "CPUCL2", "DSU", "BCI", "G3D", "G3DL2", "TPU", "INTCAM", "TNR", "CAM", "MFC", "DISP", "AOC", "BW"];
const FREQ_REQ: u32 = 0;
const FREQ_GET: u32 = 1;

/// The rate the firmware holds for a DVFS domain, in kHz (`exynos_acpm_get_rate`: word 3 non-zero is an error code, else word 1 is the rate).
pub fn dvfs_get(domain: u32) -> Result<u32, u32> {
    let r = call(DVFS_CHANNEL, [domain, 0, FREQ_GET, 0], true)?;
    if r[3] != 0 { Err(E_FW | (r[3] & 0xFF) << 8) } else { Ok(r[1]) }
}

/// Ask the firmware for a DVFS domain's rate, in kHz (`exynos_acpm_set_rate`). The CPU clusters, MIF and INT take no reply in Linux; the rest return word 3 as the result.
pub fn dvfs_set(domain: u32, khz: u32) -> Result<u32, u32> {
    let quiet = domain <= 4;
    let r = call(DVFS_CHANNEL, [domain, khz, FREQ_REQ, 0], !quiet)?;
    Ok(r[3])
}

/// Read one PMIC register.
pub fn read(speedy: u32, typ: u32, reg: u32) -> Result<u8, u32> {
    transact(speedy, typ, reg, 0, 0, 0).map(|w1| (w1 >> 8) as u8)
}

/// Write one PMIC register.
pub fn write(speedy: u32, typ: u32, reg: u32, val: u8) -> Result<(), u32> {
    transact(speedy, typ, reg, 1, val as u32, 0).map(|_| ())
}

/// Update the bits of `mask` in one PMIC register to `val`.
pub fn update(speedy: u32, typ: u32, reg: u32, val: u8, mask: u8) -> Result<(), u32> {
    transact(speedy, typ, reg, 2, val as u32, mask as u32).map(|_| ())
}

/// The thermal channel (`acpm-ipc-channel = <9>` on zuma's `acpm_tmu` node; exynos_acpm_tmu.c). The firmware owns the TMU and its trim fuses; the AP asks.
pub const TMU_CHANNEL: usize = 9;
/// zuma's thermal zones by their `id` (zuma-b0-*.dts `tmuctrl_N`): the three CPU clusters, the GPU, the camera ISP, the TPU and the AUR (the AoC's audio/sensor core).
pub const TMU_ZONES: [&str; 7] = ["BIG", "MID", "LITTLE", "G3D", "ISP", "TPU", "AUR"];
const TMU_IPC_INIT: u32 = 0x01;
const TMU_IPC_READ_TEMP: u32 = 0x02;

/// TMU_IPC_INIT (`exynos_acpm_tmu_set_init`): Linux sends it once at probe; the reply's `ret` byte carries the firmware's capability bits (1 APM interrupt, 2 APM divider).
pub fn tmu_init() -> Result<u8, u32> {
    let r = call(TMU_CHANNEL, [0, TMU_IPC_INIT, 0, 0], true)?;
    Ok((r[1] >> 8) as u8)
}

/// One zone's temperature (`exynos_acpm_tmu_set_read_temp`): request word 1 = type | tzid << 16; reply word 1 = type | ret << 8 | tzid << 16 | temp << 24 (whole °C, the zone's hottest probe), word 2 byte 0 the zone's status and bytes 1-3 + word 3 the zone's probes one by one (zero past the last). Returns (temp, stat, probes).
pub fn tmu_read(zone: u32) -> Result<(u8, u8, [u8; 7]), u32> {
    let r = call(TMU_CHANNEL, [0, TMU_IPC_READ_TEMP | (zone & 0xFF) << 16, 0, 0], true)?;
    let ret = (r[1] >> 8) as u8 as i8;
    if ret < 0 {
        return Err(E_FW | (ret as u8 as u32) << 8);
    }
    let p = [(r[2] >> 8) as u8, (r[2] >> 16) as u8, (r[2] >> 24) as u8, r[3] as u8, (r[3] >> 8) as u8, (r[3] >> 16) as u8, (r[3] >> 24) as u8];
    Ok(((r[1] >> 24) as u8, r[2] as u8, p))
}
