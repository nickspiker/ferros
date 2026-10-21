//! Display adoption after a live handoff: take over the DECON0 pipeline Linux left running and put ferros's own pixels on the panel.
//!
//! Cold boot (slot b): ABL leaves DECON running its splash with the DPU SYSMMUs' translation off, so the layer's DMA address is physical; `show` handles both.
//!
//! Preconditions after a Linux handoff (tools/husky-hotload.sh --live): Android was running with the screen on and DECON hibernation off, so DECON0 is powered, configured (command mode, DSC, panel initialized) and holding the last frame. After a normal `adb reboot ferros` the DPU is powered down and DECON reads all zeros; `show` then reports that and touches nothing.
//!
//! What Linux leaves (measured on husky): DECON v7.6 and a screen-dependent stack of windows (seen: win2 on DPP0 + win3 on DPP8; win0..3,5..7 with win0 on DPP8), 1008x2244 (the default FHD+ render size, scaled by the DPU to the 1344x2992 panel), RGBA8888 with AFBC compression, at DPU-SYSMMU IOVA G#F700_0000. The SYSMMU (dpuf0, v9, VM0) maps that IOVA as 1 MiB sections onto contiguous DRAM from G#9F40_0000 for the first 8 MiB, the tail through a level-2 table.
//!
//! Recipe: keep the base window's IOVA and its mapping, switch the layer to linear 0x00RRGGBB pixels (Samsung BGRX8888, AFBC off), write pixels through the mapping (MMU is off, so CPU stores land in DRAM directly), disable every other window, request the window + global shadow update, unmask the hardware (TE) trigger for one frame, wait for FRAME_COUNT to move, mask the trigger again (cal_9865 decon_reg_start / decon_reg_set_trigger).

use core::ptr::{read_volatile, write_volatile};

const DECON0: usize = 0x1947_0000;
const DECON_WINCON: usize = 0x194A_0000;
/// Window geometry block (DECON_WIN): START_POSITION at +G#C, END_POSITION at +G#10 per window (G#1000 apart), both (y << 16) | x.
const DECON_WIN: usize = 0x1948_0000;
/// Window channel map value -> (DPP RDMA register base, the DPU SYSMMU in front of it). The 4-bit WIN_CHMAP is (DPUF << 3) | index: 0..6 = DPUF0 L0..L6 at G#1990_0000 + index * G#1000, 8..14 = DPUF1 L7..L13 at G#19D0_0000 + index * G#1000 (measured: chmap 8 is the layer at G#19D0_0000). Index 7 in each half is not a plain layer DPP. SYSMMU dpuf0/dpuf1 are samsung v9; VM0 FLPT base PPN at G#8404.
fn dpp(ch: u32) -> Option<(usize, usize)> {
    match ch {
        0..=6 => Some((0x1990_0000 + ch as usize * 0x1000, 0x1984_0000)),
        8..=14 => Some((0x19D0_0000 + (ch as usize - 8) * 0x1000, 0x19C4_0000)),
        _ => None,
    }
}

// DECON main (cal_9865 regs-decon.h)
const DECON_VERSION: usize = 0x0000;
const FRAME_COUNT: usize = 0x0004;
const GLOBAL_CON: usize = 0x0020;
const TRIG_CON: usize = 0x0030;
const HW_TRIG_EN: u32 = 1 << 0;
const HW_TRIG_MASK_DECON: u32 = 1 << 4;
const SHD_REG_UP_REQ: usize = 0x0050;
const SHD_REG_UP_REQ_GLOBAL: u32 = 1 << 31;
const SHD_REG_UP_REQ_CMP: u32 = 1 << 20;
const DECON_INT_PEND: usize = 0x0070;

// DPP RDMA (cal_9865 regs-dpp.h)
const RDMA_IN_CTRL_0: usize = 0x0008;
const IDMA_IMG_FORMAT_MASK: u32 = 0x3F << 8;
// Samsung names RDMA formats by memory byte order (cal_9865 regs-dpp.h): our pixels are u32 0x00RRGGBB, i.e. bytes B, G, R, X — their BGRX8888 (4). Their "XRGB8888" (7) is bytes X, R, G, B: with it white came out yellow and red/green swapped (measured on husky 2026-10-14).
const IDMA_IMG_FORMAT_BGRX8888: u32 = 4 << 8;
const IDMA_ROT_MASK: u32 = 0x7 << 4;
const IDMA_COMP_MASK: u32 = (1 << 3) | (1 << 2) | (1 << 1) | (1 << 0); // AFBC, SBWC, SAJC, BLOCK
const RDMA_SRC_WIDTH: usize = 0x0010;
const RDMA_SRC_OFFSET: usize = 0x0018;
const RDMA_IMG_SIZE: usize = 0x001C;
const RDMA_BASEADDR_P0: usize = 0x0040;

const DECON_V7_6: u32 = 0x0706_0000;

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}

/// Translate a DPU IOVA through a DPU SYSMMU's VM0 (samsung-iommu.h: level-1 entries cover 1 MiB, flag 2 = section, 1 = level-2 table; level-2 flag 2 = 4 KiB page, 1 = 64 KiB page; entry addresses are stored >> 4).
fn iova_to_phys(sysmmu: usize, iova: u64) -> Option<u64> {
    // VM0 translation off (MMU_CTRL_VM bit 0 clear) = DMA addresses are physical. That is what ABL leaves on a cold boot (ctrl G#24, no page table); Linux turns translation on.
    if rd(sysmmu + 0x8000) & 1 == 0 {
        return Some(iova);
    }
    let flpt = ((rd(sysmmu + 0x8404) & 0xFF_FFFF) as u64) << 12;
    if flpt == 0 {
        return None;
    }
    let l1 = rd((flpt + (iova >> 20) * 4) as usize) as u64;
    match l1 & 7 {
        2 => Some(((l1 & !0xFFFF) << 4) | (iova & 0xF_FFFF)),
        1 => {
            let l2_base = (l1 & !0x3F) << 4;
            let l2 = rd((l2_base + ((iova & 0xF_FFFF) >> 12) * 4) as usize) as u64;
            match l2 & 3 {
                2 => Some(((l2 & !0xFF) << 4) | (iova & 0xFFF)),
                1 => Some(((l2 & !0xFFF) << 4) | (iova & 0xFFFF)),
                _ => None,
            }
        }
        _ => None,
    }
}

pub fn append_hex(out: &mut alloc::vec::Vec<u8>, label: &[u8], v: u32) {
    out.extend_from_slice(label);
    let hex = b"0123456789ABCDEF";
    for i in (0..8).rev() {
        out.push(hex[((v >> (i * 4)) & 0xF) as usize]);
    }
    out.push(b'\n');
}

/// Paint `text` (lines separated by '\n') on the panel. Returns a register report (one LABEL=G#value per line) whatever happens, so each step can be checked without seeing the screen.
/// The adopted base layer: a physically contiguous surface of u32 0x00RRGGBB pixels the CPU writes directly, and what it takes to push a frame.
pub struct Panel {
    /// Physical address of the first pixel.
    pub fb: usize,
    pub width: usize,
    /// Rows reachable through `fb` (the physically contiguous prefix of the surface).
    pub rows: usize,
    /// Row pitch in pixels.
    pub stride: usize,
    up: u32,
    cntfrq: u64,
    /// The layer's DMA block, for its IRQ status after a present (read errors show there).
    dpp: usize,
}

/// Where the painter's pixels live: 0 = ABL's splash surface (the layer's own base, through the SYSMMU if it translates); otherwise the physical base of ferros's own framebuffer (cachemap DISPLAY_FB), which `adopt` points the layer at. The layer's original base, stride and offset are kept for the way back.
static FB_OVERRIDE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static ABL_LAYER: [core::sync::atomic::AtomicU32; 3] = [core::sync::atomic::AtomicU32::new(0), core::sync::atomic::AtomicU32::new(0), core::sync::atomic::AtomicU32::new(0)];

/// Paint on ferros's own framebuffer (true) or ABL's surface (false). Takes effect at the next `adopt` (every show/fill).
pub fn use_own_fb(own: bool) -> bool {
    use core::sync::atomic::Ordering::Relaxed;
    let (f0, f1) = crate::cachemap::DISPLAY_FB;
    if own && f0 == f1 {
        return false;
    }
    FB_OVERRIDE.store(if own { f0 } else { 0 }, Relaxed);
    true
}

/// Put `text` on the panel (bridge DISPLAY): adopt, amber console, one frame. Returns the adoption report.
pub fn show(text: &[u8], cntfrq: u64) -> alloc::vec::Vec<u8> {
    let mut rep = alloc::vec::Vec::new();
    let Some(panel) = adopt(cntfrq, &mut rep) else {
        return rep;
    };
    unsafe {
        let mut con = panel.console(0x00FF_B000);
        for &b in text {
            con.putc(b);
        }
    }
    panel.present(&mut rep);
    rep
}

/// A solid G#00RRGGBB over the whole surface, one frame: the controlled scene power profiling needs (bridge `display fill`).
pub fn fill(rgb: u32, cntfrq: u64) -> alloc::vec::Vec<u8> {
    let mut rep = alloc::vec::Vec::new();
    let Some(panel) = adopt(cntfrq, &mut rep) else {
        return rep;
    };
    for y in 0..panel.rows {
        let row = panel.fb + y * panel.stride * 4;
        for x in 0..panel.width {
            unsafe { core::ptr::write_volatile((row + x * 4) as *mut u32, rgb) };
        }
    }
    unsafe { core::arch::asm!("dsb sy") };
    panel.present(&mut rep);
    rep
}

const DECON_INT_EN: usize = 0x0060;
const INT_EN_EXTRA: u32 = 1 << 4;
const DECON_INT_EN_EXTRA: usize = 0x0064;
const INT_EN_TE_RISE: u32 = 1 << 16;
const DECON_INT_PEND_EXTRA: usize = 0x0074;
const INT_PEND_TE_RISE: u32 = 1 << 16;

/// The panel's TE pin (gpp0-3, pinctrl@1084_0000 bank 0: DAT at +4, bit 3): the TE pulse itself, before any DPU logic.
pub const GPP0_DAT: usize = 0x1084_0004;

/// The panel's refresh rate and TE pulse width over half a second: rising edges on the TE pin (Hz), DECON's TE-rise interrupt with the hardware trigger armed but masked (Hz), and the mean high time of the pulse in µs. Ground truth for mode switches and the TE timing profile, no eyes needed.
pub fn te_rate(cntfrq: u64) -> (u32, u32) {
    te_profile(cntfrq).0
}

pub fn te_profile(cntfrq: u64) -> ((u32, u32), u32) {
    let en = rd(DECON0 + DECON_INT_EN);
    let en_x = rd(DECON0 + DECON_INT_EN_EXTRA);
    let trig = rd(DECON0 + TRIG_CON);
    wr(DECON0 + DECON_INT_EN_EXTRA, en_x | INT_EN_TE_RISE);
    wr(DECON0 + DECON_INT_EN, en | INT_EN_EXTRA);
    wr(DECON0 + DECON_INT_PEND_EXTRA, INT_PEND_TE_RISE);
    wr(DECON0 + TRIG_CON, trig | HW_TRIG_EN | HW_TRIG_MASK_DECON);
    let tick = || {
        let t: u64;
        unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) t) };
        t
    };
    let start = tick();
    let (mut pin_rises, mut irq_rises) = (0u32, 0u32);
    let (mut high_ticks, mut rose_at) = (0u64, 0u64);
    let mut last = unsafe { core::ptr::read_volatile(GPP0_DAT as *const u32) } >> 3 & 1;
    loop {
        let now = tick();
        if now.wrapping_sub(start) >= cntfrq / 2 {
            break;
        }
        let pin = unsafe { core::ptr::read_volatile(GPP0_DAT as *const u32) } >> 3 & 1;
        if pin == 1 && last == 0 {
            pin_rises += 1;
            rose_at = now;
        } else if pin == 0 && last == 1 && rose_at != 0 {
            high_ticks += now.wrapping_sub(rose_at);
        }
        last = pin;
        if rd(DECON0 + DECON_INT_PEND_EXTRA) & INT_PEND_TE_RISE != 0 {
            wr(DECON0 + DECON_INT_PEND_EXTRA, INT_PEND_TE_RISE);
            irq_rises += 1;
        }
    }
    wr(DECON0 + TRIG_CON, trig);
    wr(DECON0 + DECON_INT_EN_EXTRA, en_x);
    wr(DECON0 + DECON_INT_EN, en);
    let width_us = if pin_rises > 0 { (high_ticks * 1_000_000 / cntfrq / pin_rises as u64) as u32 } else { 0 };
    ((pin_rises * 2, irq_rises * 2), width_us)
}

/// DECON0's state as Android runs it at WQHD 120 Hz, where ABL leaves it unset (the live diff against oracle/display/husky-hk3/W120_noidle, 2026-10-16): SRAM for every window Android allocates, the out-FIFO QoS, the interrupt enables. The main registers are shadowed: the next present's global shadow update applies them.
///
/// NOT set here: `GLOBAL_CON.TEN_BPC_MODE` with the enhance-path dither alone wedges DECON (2026-10-16: the next frame timed out and the shadow request never cleared). The dither bit on DECON0 means "the DQE output is dithered", so the DQE must be in the path and sized first; dpu::modeset's 10 bpc option does that and works.
const OF_URGENT_EN: usize = 0x02C0;
const OF_RD_URGENT_0: usize = 0x02C4;
const OF_RD_URGENT_1: usize = 0x02C8;
const OF_DTA_CONTROL: usize = 0x02D0;
const OF_DTA_THRESHOLD: usize = 0x02D4;
const SRAM_EN_OF_PRI_0: usize = 0x0300;
const SRAM_EN_OF_PRI_1: usize = 0x0304;
const SRAM_EN_OF_SEC_0: usize = 0x0320;

pub fn configure_like_android(rep: &mut alloc::vec::Vec<u8>) {
    wr(DECON0 + OF_URGENT_EN, 1);
    wr(DECON0 + OF_RD_URGENT_0, 0x0800_0400);
    wr(DECON0 + OF_RD_URGENT_1, 0x10);
    wr(DECON0 + OF_DTA_CONTROL, 1);
    wr(DECON0 + OF_DTA_THRESHOLD, 0x3200_0600);
    wr(DECON0 + SRAM_EN_OF_PRI_0, 0x1111_1111);
    wr(DECON0 + SRAM_EN_OF_PRI_1, 0x111);
    wr(DECON0 + SRAM_EN_OF_SEC_0, 0);
    wr(DECON0 + DECON_INT_EN, 0x0030_3011);
    append_hex(rep, b"GLOBAL_CON=", rd(DECON0 + GLOBAL_CON));
}

const DECON_SUB: usize = 0x1949_0000;
const DMA0: usize = 0x1990_0000;
const DPP0: usize = 0x1993_0000;
const SRAMC0: usize = 0x1995_0000;
const HDRCOMM0: usize = 0x1996_0000;
const DSIM0: usize = 0x1944_0000;

/// Switch the pipe between the panel's WQHD raster and its FHD raster (DISPLAY-HK3.md §1: the panel scales FHD up itself). The registers and values are the oracle's W120_noidle ⇄ F120_noidle diff (2026-10-16). `steps` selects what to do, to find what the hardware tolerates live: bit 0 the panel (DSC on, the PPS, the raster select — proven harmless alone), bit 1 DECON main sizes, both DSC encoders' group count and PPS words, the layer (DMA0/DPP0/SRAMC0/HDRCOMM0) and window 5, bit 2 DSIM0's resolution, compressed width, slice widths and stable-VFP timer. Then one present to apply the shadow registers, and the panel's raster register and TE rate as the report. A one-shot switch of everything (steps 7) took one frame and then wedged the link; Android does a full modeset for a resolution change.
pub fn resolution(fhd: bool, steps: u8, cntfrq: u64, rep: &mut alloc::vec::Vec<u8>) {
    let cfg = if fhd { ferros_dsc::HUSKY_FHD } else { ferros_dsc::HUSKY_WQHD };
    let (w, h) = (cfg.pic_width as u32, cfg.pic_height as u32);
    let comp = cfg.slice_width as u32 / 3;
    let pps = ferros_dsc::pps(&cfg);
    if steps & 1 != 0 {
        if crate::dsim::send(crate::dsim::DCS_SHORT_WRITE_1, &[0x9D, 0x01], cntfrq).is_err() || crate::dsim::send(crate::dsim::PPS_LONG_WRITE, &pps, cntfrq).is_err() {
            rep.extend_from_slice(b"PPS: FIFO stuck\n");
            return;
        }
        let (sent, fail) = crate::dsim::sequence(if fhd { crate::hk3::RES_FHD } else { crate::hk3::RES_WQHD }, cntfrq);
        if fail.is_some() {
            append_hex(rep, b"raster select stuck after ", sent as u32);
            return;
        }
    }
    if steps & 2 != 0 {
        wr(DECON0 + 0x0220, h << 16 | w);
        wr(DECON0 + 0x0290, h << 16 | comp);
        wr(DECON0 + 0x0294, comp);
        wr(DECON0 + 0x0298, (cfg.slice_height as u32) << 16 | comp);
        for dsc in [DECON_SUB, DECON_SUB + 0x1000] {
            wr(dsc + 0x000C, 3 << 12 | comp);
            for i in 0..18 {
                wr(dsc + 0x0040 + 4 * i, u32::from_be_bytes([pps[4 * i], pps[4 * i + 1], pps[4 * i + 2], pps[4 * i + 3]]));
            }
        }
        wr(DMA0 + 0x0010, w);
        wr(DMA0 + 0x0014, h);
        wr(DMA0 + 0x001C, h << 16 | w);
        wr(DPP0 + 0x003C, h << 16 | w);
        wr(SRAMC0 + 0x0014, (h - 1) << 16);
        wr(HDRCOMM0 + 0x0020, h << 16 | w);
        wr(DECON_WIN + 5 * 0x1000 + 0x000C, 0);
        wr(DECON_WIN + 5 * 0x1000 + 0x0010, (h - 1) << 16 | (w - 1));
    }
    if steps & 4 != 0 {
        let dsim = |off: usize, v: u32| unsafe { core::ptr::write_volatile((DSIM0 + off) as *mut u32, v) };
        dsim(0x0030, h);
        dsim(0x0034, if fhd { 0x0ED8 } else { 0x063A });
        dsim(0x0038, comp);
        dsim(0x003C, h << 16 | comp);
        dsim(0x0078, (cfg.slice_width as u32) << 16 | cfg.slice_width as u32);
        dsim(0x0084, w * 2 / 100);
    }
    let r = show(if fhd { b"ferros FHD 1008x2244" } else { b"ferros WQHD 1344x2992" }, cntfrq);
    rep.extend_from_slice(&r);
    let _ = crate::dsim::dcs(&[0xF0, 0x5A, 0x5A], cntfrq);
    match crate::dsim::read(0xC3, 1, cntfrq) {
        Ok(v) => append_hex(rep, b"PANEL_C3=", v[0] as u32),
        Err(e) => append_hex(rep, b"PANEL_C3_READ_FAILED=", e),
    }
    let _ = crate::dsim::dcs(&[0xF0, 0xA5, 0xA5], cntfrq);
    let (te, _) = te_rate(cntfrq);
    append_hex(rep, b"TE_HZ=", te);
}

/// Whether the DPU power domains are on, read through the EL3 monitor (never touch a powered-down DPU: see `adopt`).
pub fn dpu_on() -> bool {
    [0x1546_1D00u64, 0x1546_1D80, 0x1546_1E00].iter().all(|&a| crate::platform::priv_reg(a, false, 0) == 1)
}

/// Take over the layer ABL (cold) or Linux (--live handoff) left on screen: checks, picks the base window, switches it to linear 0x00RRGGBB pixels (BGRX8888), turns every other window off and clears the whole surface to black. Reports into `rep`; None if there is no panel ferros can drive.
pub fn adopt(cntfrq: u64, rep: &mut alloc::vec::Vec<u8>) -> Option<Panel> {
    // Never touch a powered-down DPU: on husky a CPU read of DECON with the DPU domain off returns 0 but leaves the next EL3 SMC_CMD_PRIV_REG call hanging forever (measured: the boot screen's single DECON_VERSION read in the reboot path made DIAG's PMU readback hang). The domain state is read through the EL3 monitor, which is safe: word 0 of each PMU power-domain block is 1 when on (pd-dpub G#1546_1D00, pd-dpuf0 G#1546_1D80, pd-dpuf1 G#1546_1E00; other offsets are not whitelisted and return -EINVAL).
    let pd = [0x1546_1D00u64, 0x1546_1D80, 0x1546_1E00].map(|a| crate::platform::priv_reg(a, false, 0));
    append_hex(rep, b"PD_DPUB=", pd[0] as u32);
    append_hex(rep, b"PD_DPUF0=", pd[1] as u32);
    append_hex(rep, b"PD_DPUF1=", pd[2] as u32);
    if pd.iter().any(|&v| v != 1) {
        rep.extend_from_slice(b"DPU powered down (hand off with --live so DECON is still running)\n");
        return None;
    }
    let version = rd(DECON0 + DECON_VERSION);
    append_hex(rep, b"DECON_VERSION=", version);
    if version != DECON_V7_6 {
        rep.extend_from_slice(b"DPU not powered (hand off with --live so DECON is still running)\n");
        return None;
    }
    append_hex(rep, b"GLOBAL_CON=", rd(DECON0 + GLOBAL_CON));
    append_hex(rep, b"TRIG_CON_BEFORE=", rd(DECON0 + TRIG_CON));

    // Base window = the first enabled window that covers the whole screen with an unscaled layer (DPP image size == window size), on a channel ferros can drive; every other window is switched off. Android's stack varies with what is on screen (e.g. a full-screen window can be a 209x466 wallpaper scaled up by the DPP), so this is chosen per call. The screen size is the largest window end seen.
    let mut enabled_mask = 0u32;
    let mut screen_end = 0u32;
    for w in 0..8 {
        if rd(DECON_WINCON + w * 0x1000) & 1 == 1 {
            enabled_mask |= 1 << w;
            let end = rd(DECON_WIN + w * 0x1000 + 0x10);
            if (end & 0x3FFF) >= (screen_end & 0x3FFF) && (end >> 16) >= (screen_end >> 16) {
                screen_end = end;
            }
        }
    }
    let mut base_win = None;
    for w in 0..8 {
        if enabled_mask & (1 << w) == 0 {
            continue;
        }
        let ch = (rd(DECON_WINCON + w * 0x1000) >> 4) & 0xF;
        let start = rd(DECON_WIN + w * 0x1000 + 0xC);
        let end = rd(DECON_WIN + w * 0x1000 + 0x10);
        if let Some((base, _)) = dpp(ch) {
            let img = rd(base + RDMA_IMG_SIZE);
            let unscaled = (img & 0xFFFF) == (end & 0x3FFF) + 1 && (img >> 16) == ((end >> 16) & 0x3FFF) + 1;
            if start == 0 && end == screen_end && unscaled {
                base_win = Some((w, ch));
                break;
            }
        }
    }
    append_hex(rep, b"WINDOWS_ENABLED=", enabled_mask);
    let (win, ch) = match base_win {
        Some(b) => b,
        None => {
            rep.extend_from_slice(b"no full-screen unscaled layer on a drivable channel\n");
            return None;
        }
    };
    append_hex(rep, b"BASE_WIN=", win as u32);
    append_hex(rep, b"BASE_CHANNEL=", ch);
    let (dpp_base, sysmmu) = match dpp(ch) {
        Some(d) => d,
        None => {
            rep.extend_from_slice(b"base window is on a channel ferros does not drive\n");
            return None;
        }
    };
    let img = rd(dpp_base + RDMA_IMG_SIZE);
    let width = (img & 0xFFFF) as usize;
    let height = (img >> 16) as usize;
    let src_off = rd(dpp_base + RDMA_SRC_OFFSET);
    let stride = rd(dpp_base + RDMA_SRC_WIDTH) as usize;
    let first_px = ((src_off >> 16) as usize) * stride + (src_off & 0xFFFF) as usize;
    let iova = rd(dpp_base + RDMA_BASEADDR_P0) as u64 + first_px as u64 * 4;
    let in_ctrl = rd(dpp_base + RDMA_IN_CTRL_0);
    append_hex(rep, b"WIDTH=", width as u32);
    append_hex(rep, b"HEIGHT=", height as u32);
    append_hex(rep, b"STRIDE=", stride as u32);
    append_hex(rep, b"SRC_OFFSET=", src_off);
    append_hex(rep, b"FB_IOVA=", iova as u32);
    append_hex(rep, b"IN_CTRL_0_BEFORE=", in_ctrl);
    if stride < width || width == 0 || height == 0 {
        rep.extend_from_slice(b"implausible geometry\n");
        return None;
    }

    // ferros's own framebuffer: the layer re-pointed at it (translation is off on a cold boot, so the DMA address is the physical one), stride = width, no offset; ABL's words kept the first time so `use_own_fb(false)` can put them back.
    use core::sync::atomic::Ordering::Relaxed;
    let own = FB_OVERRIDE.load(Relaxed);
    let (stride, iova) = if own != 0 {
        if ABL_LAYER[0].load(Relaxed) == 0 {
            ABL_LAYER[0].store(rd(dpp_base + RDMA_BASEADDR_P0), Relaxed);
            ABL_LAYER[1].store(rd(dpp_base + RDMA_SRC_WIDTH), Relaxed);
            ABL_LAYER[2].store(rd(dpp_base + RDMA_SRC_OFFSET), Relaxed);
        }
        wr(dpp_base + RDMA_BASEADDR_P0, own as u32);
        wr(dpp_base + RDMA_SRC_WIDTH, width as u32);
        wr(dpp_base + RDMA_SRC_OFFSET, 0);
        append_hex(rep, b"OWN_FB=", own as u32);
        (width, own as u64)
    } else {
        if ABL_LAYER[0].load(Relaxed) != 0 && rd(dpp_base + RDMA_BASEADDR_P0) == crate::cachemap::DISPLAY_FB.0 as u32 {
            wr(dpp_base + RDMA_BASEADDR_P0, ABL_LAYER[0].load(Relaxed));
            wr(dpp_base + RDMA_SRC_WIDTH, ABL_LAYER[1].load(Relaxed));
            wr(dpp_base + RDMA_SRC_OFFSET, ABL_LAYER[2].load(Relaxed));
            let stride = ABL_LAYER[1].load(Relaxed) as usize;
            let src_off = ABL_LAYER[2].load(Relaxed);
            let first_px = ((src_off >> 16) as usize) * stride + (src_off & 0xFFFF) as usize;
            (stride, ABL_LAYER[0].load(Relaxed) as u64 + first_px as u64 * 4)
        } else {
            (stride, iova)
        }
    };

    // Physically contiguous prefix of the surface (whole 1 MiB sections that follow on from the first).
    let fb_bytes = stride * height * 4;
    let phys0 = match iova_to_phys(sysmmu, iova) {
        Some(p) => p,
        None => {
            rep.extend_from_slice(b"FB IOVA not mapped\n");
            return None;
        }
    };
    let mut contig = 0usize;
    while contig < fb_bytes {
        let step = 0x10_0000 - ((iova as usize + contig) & 0xF_FFFF);
        match iova_to_phys(sysmmu, iova + contig as u64) {
            Some(p) if p == phys0 + contig as u64 => contig += step,
            _ => break,
        }
    }
    let contig = contig.min(fb_bytes);
    append_hex(rep, b"FB_PHYS=", phys0 as u32);
    append_hex(rep, b"FB_PHYS_HI=", (phys0 >> 32) as u32);
    append_hex(rep, b"FB_CONTIG_BYTES=", contig as u32);

    // Pixels: the whole surface to black — the contiguous prefix directly, the rest through the mapping.
    const BG: u32 = 0x0000_0000;
    let contig_rows = contig / (stride * 4);
    for i in 0..contig_rows * stride {
        wr(phys0 as usize + i * 4, BG);
    }
    let mut off = contig_rows * stride * 4;
    while off < fb_bytes {
        if let Some(p) = iova_to_phys(sysmmu, iova + off as u64) {
            let page_left = 0x1000 - (off & 0xFFF);
            let n = page_left.min(fb_bytes - off);
            for i in (0..n).step_by(4) {
                wr(p as usize + i, BG);
            }
            off += n;
        } else {
            off += 0x1000 - (off & 0xFFF);
        }
    }
    unsafe { core::arch::asm!("dsb sy") };

    // Linear 0x00RRGGBB (BGRX8888) on the base layer; every other window off.
    let new_ctrl = (in_ctrl & !(IDMA_IMG_FORMAT_MASK | IDMA_ROT_MASK | IDMA_COMP_MASK)) | IDMA_IMG_FORMAT_BGRX8888;
    wr(dpp_base + RDMA_IN_CTRL_0, new_ctrl);
    let mut up = SHD_REG_UP_REQ_GLOBAL | SHD_REG_UP_REQ_CMP | (1 << win);
    for w in 0..8 {
        if w != win && enabled_mask & (1 << w) != 0 {
            let a = DECON_WINCON + w * 0x1000;
            wr(a, rd(a) & !1);
            up |= 1 << w;
        }
    }
    append_hex(rep, b"IN_CTRL_0_AFTER=", rd(dpp_base + RDMA_IN_CTRL_0));

    Some(Panel { fb: phys0 as usize, width, rows: contig_rows, stride, up, cntfrq, dpp: dpp_base })
}

impl Panel {
    /// A console over the panel's contiguous rows (margins clear the rounded corners and the camera hole).
    ///
    /// # Safety
    /// The panel must still be the adopted, live surface.
    pub unsafe fn console(&self, fg: u32) -> ferros_hal::console::Console {
        unsafe { ferros_hal::console::Console::new(self.fb as *mut u32, self.width, self.rows, self.stride, fg, 0, (160, 48, 48, 48)) }
    }

    /// Switch the base layer between 8-bit BGRX8888 and 10-bit-per-channel 2:10:10:10 (cal_9865 regs-dpp.h: BGRX8888 is 4, ARGB2101010 is 19). In the packed format a pixel is one u32 with alpha in bits [31:30] and three 10-bit channels; green sits in bits [19:10] in both the ARGB and ABGR orderings, so only red and blue can be the other way round.
    pub fn set_10bit(&self, on: bool) {
        let v = rd(self.dpp + RDMA_IN_CTRL_0) & !IDMA_IMG_FORMAT_MASK;
        wr(self.dpp + RDMA_IN_CTRL_0, v | if on { 19 << 8 } else { IDMA_IMG_FORMAT_BGRX8888 });
    }

    /// Push one frame: shadow update, unmask the TE trigger, wait for FRAME_COUNT to move (or 1/4 s), mask it again.
    pub fn present(&self, rep: &mut alloc::vec::Vec<u8>) {
        let cntfrq = self.cntfrq;
        let up = self.up;
        let f0 = rd(DECON0 + FRAME_COUNT);
        append_hex(rep, b"FRAME_COUNT_BEFORE=", f0);
        wr(DECON0 + SHD_REG_UP_REQ, rd(DECON0 + SHD_REG_UP_REQ) | up);
        let t = rd(DECON0 + TRIG_CON);
        wr(DECON0 + TRIG_CON, (t & !HW_TRIG_MASK_DECON) | HW_TRIG_EN);
        let start: u64;
        unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) start) };
        loop {
            let now: u64;
            unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) now) };
            if rd(DECON0 + FRAME_COUNT) != f0 || now.wrapping_sub(start) > cntfrq / 4 {
                break;
            }
        }
        let t = rd(DECON0 + TRIG_CON);
        wr(DECON0 + TRIG_CON, (t & !HW_TRIG_EN) | HW_TRIG_MASK_DECON);
        append_hex(rep, b"FRAME_COUNT_AFTER=", rd(DECON0 + FRAME_COUNT));
        append_hex(rep, b"SHD_REG_UP_REQ_AFTER=", rd(DECON0 + SHD_REG_UP_REQ));
        append_hex(rep, b"DECON_INT_PEND=", rd(DECON0 + DECON_INT_PEND));
        append_hex(rep, b"TRIG_CON_AFTER=", rd(DECON0 + TRIG_CON));
        // The layer DMA's IRQ status (regs-dpp.h RDMA_IRQ): FRAME_DONE bit 16; READ_SLAVE_ERR bit 19 or AXI_ADDR_ERR bit 26 would mean the fence or the bus refused the surface. Cleared for the next present.
        let irq = rd(self.dpp + 0x0004);
        append_hex(rep, b"RDMA_IRQ=", irq);
        wr(self.dpp + 0x0004, irq & (0xDFB << 16));
    }
}
