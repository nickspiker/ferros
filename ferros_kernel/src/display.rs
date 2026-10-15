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

fn append_hex(out: &mut alloc::vec::Vec<u8>, label: &[u8], v: u32) {
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

    Some(Panel { fb: phys0 as usize, width, rows: contig_rows, stride, up, cntfrq })
}

impl Panel {
    /// A console over the panel's contiguous rows (margins clear the rounded corners and the camera hole).
    ///
    /// # Safety
    /// The panel must still be the adopted, live surface.
    pub unsafe fn console(&self, fg: u32) -> ferros_hal::console::Console {
        unsafe { ferros_hal::console::Console::new(self.fb as *mut u32, self.width, self.rows, self.stride, fg, 0, (160, 48, 48, 48)) }
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
    }
}
