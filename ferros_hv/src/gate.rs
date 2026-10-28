//! The boot gate in the hv (BOOT.md "Dual Boot"): ABL has prepared Android completely (kernel, ramdisks, bootconfig, DTB, and the secure world's boot state, which only the bootloader can set), and before Linux runs the hv asks.
//!
//! Screen: ABL's splash surface, saved first, then black with a green triangle at the power key and a Google-coloured chevron at volume down (the same picture as ferros's own gate, ready.rs). No words: nothing here can know the person's language (owner, 2026-10-27: "wordless").
//! Keys: power = ferros; volume down = Android; volume up = hold (cancels the one-second auto-choice of dev builds); both volume keys = the kill chord.
//! Android: the splash pixels and the layer registers are put back exactly as ABL left them, and the hv carries on to Linux as before. ferros: the cold ferros image packed at the end of the boot blob is copied to its link address and entered at EL2 with ABL's DTB, the way ABL would; a marker in the hv's report page tells it the gate has been answered so it doesn't ask again.

use core::arch::asm;
use core::ptr::{read_volatile, write_volatile};


#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Android,
    Ferros,
}

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}
fn now() -> u64 {
    let t: u64;
    unsafe { asm!("mrs {t}, cntpct_el0", t = out(reg) t) };
    t
}
fn freq() -> u64 {
    let f: u64;
    unsafe { asm!("mrs {f}, cntfrq_el0", f = out(reg) f) };
    f
}

/// The packed ferros image: at Linux's entry + its image_size rounded up to 2 MiB (tools/husky-hv.sh), a 4 KiB header {"FERROSGT", u64 length} and the flat image after it.
const LINUX_ENTRY: usize = 0x8020_0000;
const FERROS_LINK: usize = 0x8008_0000;
const FERROS_MAGIC: &[u8; 8] = b"FERROSGT";
/// In the hv's report page: "gate answered, ferros" for ferros's ready.rs; ferros clears it.
pub const GATE_MARK_ADDR: usize = 0xA_FFFF_FF10;
pub const GATE_MARK: u64 = 0x4741_5445_464F_5252; // "GATEFORR"
/// In the ramoops result block, the end of its first page (the hv report page at G#A_FFFF_0000 does not survive a PSCI reset; this block does, measured 2026-10-21): ferros's `diag android` leaves this word before resetting, so the next gate answers "Android" by itself, once, with the screen untouched: the host can switch between the two without anyone at the keys.
pub const ANDROID_ONCE_ADDR: usize = 0xFD60_0FF8;
pub const ANDROID_ONCE: u64 = 0x414E_4452_4F49_4431; // "ANDROID1"
/// Where ABL's splash pixels wait while the gate is on screen: inside the hv's own reservation, above its code and stacks.
const SPLASH_SAVE: usize = 0xA_FD00_0000;
const SPLASH_SAVE_MAX: usize = 0x200_0000;

/// The packed ferros image, if this boot image carries one: (address, length).
pub fn ferros_image() -> Option<(usize, usize)> {
    let image_size = unsafe { read_volatile((LINUX_ENTRY + 16) as *const u64) } as usize;
    if image_size == 0 || image_size > 0x400_0000 {
        return None;
    }
    let chunk = LINUX_ENTRY + ((image_size + 0x1F_FFFF) & !0x1F_FFFF);
    let magic = unsafe { read_volatile(chunk as *const [u8; 8]) };
    let len = unsafe { read_volatile((chunk + 8) as *const u64) } as usize;
    if &magic != FERROS_MAGIC || len == 0 || len > 0x100_0000 {
        return None;
    }
    Some((chunk + 0x1000, len))
}

// ---- the panel (the subset of ferros_kernel/src/display.rs a cold, untranslated DPU needs) ----
const DECON0: usize = 0x1947_0000;
const DECON_WINCON: usize = 0x194A_0000;
const DECON_WIN: usize = 0x1948_0000;
const FRAME_COUNT: usize = 0x0004;
const TRIG_CON: usize = 0x0030;
const HW_TRIG_EN: u32 = 1 << 0;
const HW_TRIG_MASK_DECON: u32 = 1 << 4;
const SHD_REG_UP_REQ: usize = 0x0050;
const SHD_REG_UP_REQ_GLOBAL: u32 = 1 << 31;
const SHD_REG_UP_REQ_CMP: u32 = 1 << 20;
const RDMA_IN_CTRL_0: usize = 0x0008;
const IDMA_IMG_FORMAT_MASK: u32 = 0x3F << 8;
const IDMA_IMG_FORMAT_BGRX8888: u32 = 4 << 8;
const IDMA_ROT_MASK: u32 = 0x7 << 4;
const IDMA_COMP_MASK: u32 = 0xF;
const RDMA_SRC_WIDTH: usize = 0x0010;
const RDMA_SRC_OFFSET: usize = 0x0018;
const RDMA_IMG_SIZE: usize = 0x001C;
const RDMA_BASEADDR_P0: usize = 0x0040;
const DECON_V7_6: u32 = 0x0706_0000;

fn dpp(ch: u32) -> Option<(usize, usize)> {
    match ch {
        0..=6 => Some((0x1990_0000 + ch as usize * 0x1000, 0x1984_0000)),
        8..=14 => Some((0x19D0_0000 + (ch as usize - 8) * 0x1000, 0x19C4_0000)),
        _ => None,
    }
}

fn priv_read(addr: u64) -> u64 {
    let r: u64;
    unsafe {
        asm!("smc #0", inout("x0") 0x8200_0504u64 => r, inout("x1") addr => _, inout("x2") 0u64 => _, inout("x3") 0u64 => _,
            out("x4") _, out("x5") _, out("x6") _, out("x7") _, out("x8") _, out("x9") _, out("x10") _, out("x11") _, out("x12") _, out("x13") _, out("x14") _, out("x15") _, out("x16") _, out("x17") _);
    }
    r
}

/// What the gate changed, to put back for Android.
struct Panel {
    fb: usize,
    width: usize,
    height: usize,
    stride: usize,
    dpp: usize,
    in_ctrl: u32,
    off_mask: u32,
    up: u32,
    trig: u32,
    saved: bool,
}

/// Take ABL's base layer as ferros's display.adopt does on a cold boot (DPU powered, DECON v7.6, a full-screen unscaled window, translation off), save its pixels, switch it to BGRX8888 with every other window off. None = leave the screen alone (the gate still works by keys).
fn adopt() -> Option<Panel> {
    if [0x1546_1D00u64, 0x1546_1D80, 0x1546_1E00].iter().any(|&a| priv_read(a) != 1) {
        return None;
    }
    if rd(DECON0) != DECON_V7_6 {
        return None;
    }
    let (mut enabled, mut screen_end) = (0u32, 0u32);
    for w in 0..8 {
        if rd(DECON_WINCON + w * 0x1000) & 1 == 1 {
            enabled |= 1 << w;
            let end = rd(DECON_WIN + w * 0x1000 + 0x10);
            if (end & 0x3FFF) >= (screen_end & 0x3FFF) && (end >> 16) >= (screen_end >> 16) {
                screen_end = end;
            }
        }
    }
    let mut base = None;
    for w in 0..8 {
        if enabled & (1 << w) == 0 {
            continue;
        }
        let ch = (rd(DECON_WINCON + w * 0x1000) >> 4) & 0xF;
        let (start, end) = (rd(DECON_WIN + w * 0x1000 + 0xC), rd(DECON_WIN + w * 0x1000 + 0x10));
        if let Some((b, mmu)) = dpp(ch) {
            let img = rd(b + RDMA_IMG_SIZE);
            let unscaled = (img & 0xFFFF) == (end & 0x3FFF) + 1 && (img >> 16) == ((end >> 16) & 0x3FFF) + 1;
            if start == 0 && end == screen_end && unscaled && rd(mmu + 0x8000) & 1 == 0 {
                base = Some((w, b));
                break;
            }
        }
    }
    let (win, dpp_base) = base?;
    let img = rd(dpp_base + RDMA_IMG_SIZE);
    let (width, height) = ((img & 0xFFFF) as usize, (img >> 16) as usize);
    let stride = rd(dpp_base + RDMA_SRC_WIDTH) as usize;
    let src_off = rd(dpp_base + RDMA_SRC_OFFSET);
    if stride < width || width == 0 || height == 0 {
        return None;
    }
    let first_px = ((src_off >> 16) as usize) * stride + (src_off & 0xFFFF) as usize;
    let fb = rd(dpp_base + RDMA_BASEADDR_P0) as usize + first_px * 4;
    let bytes = stride * height * 4;
    let saved = bytes <= SPLASH_SAVE_MAX;
    if saved {
        copy(fb, SPLASH_SAVE, bytes);
    }
    let in_ctrl = rd(dpp_base + RDMA_IN_CTRL_0);
    let off_mask = enabled & !(1 << win);
    let mut p = Panel { fb, width, height, stride, dpp: dpp_base, in_ctrl, off_mask, up: SHD_REG_UP_REQ_GLOBAL | SHD_REG_UP_REQ_CMP | (1 << win) | off_mask, trig: rd(DECON0 + TRIG_CON), saved };
    for i in 0..stride * height {
        wr(fb + i * 4, 0);
    }
    wr(dpp_base + RDMA_IN_CTRL_0, (in_ctrl & !(IDMA_IMG_FORMAT_MASK | IDMA_ROT_MASK | IDMA_COMP_MASK)) | IDMA_IMG_FORMAT_BGRX8888);
    for w in 0..8 {
        if off_mask & (1 << w) != 0 {
            let a = DECON_WINCON + w * 0x1000;
            wr(a, rd(a) & !1);
        }
    }
    Some(p)
}

/// 16-byte copy (the hv's MMU is off: Device accesses, so pairs).
fn copy(src: usize, dst: usize, bytes: usize) {
    let mut o = 0;
    while o + 16 <= bytes {
        unsafe {
            asm!("ldp {a}, {b}, [{s}]", "stp {a}, {b}, [{d}]", s = in(reg) src + o, d = in(reg) dst + o, a = out(reg) _, b = out(reg) _);
        }
        o += 16;
    }
    unsafe { asm!("dsb sy") };
}

/// One frame: shadow update, unmask the hardware trigger, wait for FRAME_COUNT to move (at most 1/4 s), mask it again (display.rs `present`).
fn present(p: &Panel) {
    let f0 = rd(DECON0 + FRAME_COUNT);
    wr(DECON0 + SHD_REG_UP_REQ, rd(DECON0 + SHD_REG_UP_REQ) | p.up);
    let t = rd(DECON0 + TRIG_CON);
    wr(DECON0 + TRIG_CON, (t & !HW_TRIG_MASK_DECON) | HW_TRIG_EN);
    let t0 = now();
    while rd(DECON0 + FRAME_COUNT) == f0 && now() - t0 < freq() / 4 {}
    let t = rd(DECON0 + TRIG_CON);
    wr(DECON0 + TRIG_CON, (t & !HW_TRIG_EN) | HW_TRIG_MASK_DECON);
}

fn px(p: &Panel, x: usize, y: usize, c: u32) {
    if x < p.width && y < p.height {
        wr(p.fb + (y * p.stride + x) * 4, c);
    }
}

/// The same picture as ferros's ready.rs.
fn draw(p: &Panel) {
    const POWER_Y: usize = 1050;
    const VOL_DOWN_Y: usize = 1700;
    let (tip, half, len) = (p.width - 28, 90usize, 140usize);
    for dx in 0..len {
        let reach = half * dx / len;
        for dy in 0..=reach {
            px(p, tip - dx, POWER_Y - dy, 0x0000_FF00);
            px(p, tip - dx, POWER_Y + dy, 0x0000_FF00);
        }
    }
    const GOOGLE: [u32; 4] = [0x004285F4, 0x00EA4335, 0x00FBBC05, 0x0034A853];
    let (tip, half, len) = (p.width - 28, 48usize, 80usize);
    for dx in 0..len {
        let reach = half * dx / len;
        let c = GOOGLE[(dx * 4 / len).min(3)];
        for dy in 0..=reach {
            px(p, tip - dx, VOL_DOWN_Y - dy, c);
            px(p, tip - dx, VOL_DOWN_Y + dy, c);
        }
    }
    unsafe { asm!("dsb sy") };
}

/// The moment ferros is chosen: the gate's picture off (the rectangles ferros's ready.rs `erase` clears) and the boot ring's first frame, its dark-brick disc (checklist.rs `ring_start`: the same centre, radius and colour), so the press answers on the glass within a frame while ferros boots behind it. ferros then adopts the panel without clearing it (the hv left it black) and draws the same disc again. Owner, 2026-10-27: a third of a second from the press to black was "kind of embarrassing"; it was ferros's whole entry, the cache map and a 16 MB uncached clear, before anything moved.
fn answered(p: &Panel) {
    const POWER_Y: usize = 1050;
    const VOL_DOWN_Y: usize = 1700;
    let rect = |xa: usize, ya: usize, xb: usize, yb: usize| {
        for y in ya..yb {
            for x in xa..xb {
                px(p, x, y, 0);
            }
        }
    };
    let tip = p.width - 28;
    rect(tip - 140, POWER_Y - 91, tip + 1, POWER_Y + 92);
    rect(tip - 80, VOL_DOWN_Y - 49, tip + 1, VOL_DOWN_Y + 50);
    // The ring's disc: centred, in the top half's middle on a dev build (checklist.rs `centre`), radius 230, dark brick.
    const R_DISC: i64 = 230;
    const DISC: u32 = 0x003C_0200;
    let (cx, cy) = ((p.width / 2) as i64, if option_env!("FERROS_PRODUCTION").is_some() { (p.height / 2) as i64 } else { (p.height / 4) as i64 });
    for y in -R_DISC..=R_DISC {
        for x in -R_DISC..=R_DISC {
            if x * x + y * y <= R_DISC * R_DISC {
                px(p, (cx + x) as usize, (cy + y) as usize, DISC);
            }
        }
    }
    unsafe { asm!("dsb sy") };
    present(p);
}

/// Put back what `adopt` changed: the pixels, the layer format, the windows; one frame so the DPU holds ABL's state again; the trigger as ABL left it.
fn restore(p: &Panel) {
    if p.saved {
        copy(SPLASH_SAVE, p.fb, p.stride * p.height * 4);
    }
    wr(p.dpp + RDMA_IN_CTRL_0, p.in_ctrl);
    for w in 0..8 {
        if p.off_mask & (1 << w) != 0 {
            let a = DECON_WINCON + w * 0x1000;
            wr(a, rd(a) | 1);
        }
    }
    present(p);
    wr(DECON0 + TRIG_CON, p.trig);
}

// ---- keys (always-on GPIO, active low; the volume pins are already pulled-up inputs, chord::arm) ----
const ALIVE: usize = 0x154D_0000;
const GPA4_CON: usize = ALIVE + 0x80;
const GPA4_DAT: usize = ALIVE + 0x84;
const GPA6_DAT: usize = ALIVE + 0xA4;
const POWER_PIN: u32 = 0;
const VOL_DOWN_PIN: u32 = 1;
const VOL_UP_PIN: u32 = 2;
const WATCHDOGS: [usize; 2] = [0x1006_0000, 0x1007_0000];

fn power() -> bool {
    rd(GPA4_DAT) & (1 << POWER_PIN) == 0
}
fn vol_down() -> bool {
    rd(GPA4_DAT) & (1 << VOL_DOWN_PIN) == 0
}
fn vol_up() -> bool {
    rd(GPA6_DAT) & (1 << VOL_UP_PIN) == 0
}

/// Ask. Without a packed ferros image there is nothing to choose: Android, screen untouched.
pub fn ask() -> Choice {
    if ferros_image().is_none() {
        return Choice::Android;
    }
    unsafe {
        if read_volatile(ANDROID_ONCE_ADDR as *const u64) == ANDROID_ONCE {
            write_volatile(ANDROID_ONCE_ADDR as *mut u64, 0);
            asm!("dsb sy");
            // FERROS_GATE_PANEL_TEST builds: the marker path also paints the gate and restores the splash, exactly as the key path does — so the key path's effect on Linux's display handover can be tested from the host (`bridge diag android`), with no finger (2026-10-23: Android chosen by key came up with its display stuck on the logo; by marker it was fine).
            if option_env!("FERROS_GATE_PANEL_TEST").is_some() {
                let c = rd(GPA4_CON) & !(0xF << (POWER_PIN * 4));
                wr(GPA4_CON, c);
                if let Some(p) = adopt() {
                    draw(&p);
                    present(&p);
                    let t = now();
                    while now().wrapping_sub(t) < freq() {}
                    restore(&p);
                }
            }
            return Choice::Android;
        }
    }
    // The power pin as a plain input (ABL may leave it as a wakeup interrupt; Linux sets it up again).
    let c = rd(GPA4_CON) & !(0xF << (POWER_PIN * 4));
    wr(GPA4_CON, c);
    let panel = adopt();
    if let Some(p) = &panel {
        draw(p);
        present(p);
    }
    let f = freq();
    let t0 = now();
    let mut held = false;
    // Keys must be up once before a press counts (a key held from before the gate, e.g. power from switching on, is not an answer).
    let mut armed = false;
    // Telemetry for ferros's DIAG (GATE_ANSWER, GATE_MS; ready.rs reads them beside the mark): how the gate was answered — 1 power, 2 the dev timer, 3 volume down — and when, in ms from the screen, plus how long the power pin read pressed before it was first seen up (the owner, 2026-10-23: a press right after Ready? seems to take three quarters of a second unless volume up is held first).
    let mut pressed_at_start_ms = 0u64;
    let mut first_up: Option<u64> = None;
    let choice = loop {
        for &w in &WATCHDOGS {
            wr(w + 0x08, 0xFFFF);
        }
        let (pw, dn, up) = (power(), vol_down(), vol_up());
        if dn && up {
            super::chord::power_off();
        }
        if !armed {
            armed = !pw && !dn;
            if armed && first_up.is_none() {
                first_up = Some(now());
                pressed_at_start_ms = now().wrapping_sub(t0) * 1000 / f;
            }
        } else if pw {
            gate_telemetry(1, now().wrapping_sub(t0) * 1000 / f, pressed_at_start_ms);
            break Choice::Ferros;
        } else if dn {
            gate_telemetry(3, now().wrapping_sub(t0) * 1000 / f, pressed_at_start_ms);
            break Choice::Android;
        }
        held |= up;
        if !held && now().wrapping_sub(t0) > f {
            gate_telemetry(2, now().wrapping_sub(t0) * 1000 / f, pressed_at_start_ms);
            break Choice::Ferros;
        }
        let t = now();
        while now().wrapping_sub(t) < f / 100 {}
    };
    if let Some(p) = &panel {
        match choice {
            Choice::Android => restore(p),
            Choice::Ferros => answered(p),
        }
    }
    choice
}

/// Android from whichever slot this gate is on. Only slot a holds an Android (slot b's own system partitions are stock or empty), and ABL names the booted slot to the kernel in bootconfig, `androidboot.slot_suffix = "_b"`, the blob at the end of the initrd (data, LE u32 size, LE u32 byte-sum checksum, "#BOOTCONFIG\n"). Rewrite the suffix to `_a` there and lower the checksum by one, so Android mounts slot a's partitions and the choice is a jump, not a slot switch and a reboot (owner, 2026-10-23: identical slots, no flags). The initrd bounds come from the DTB's /chosen. Returns whether a suffix was rewritten (already `_a`, or nothing found: untouched).
pub fn android_from_slot_a(dtb: usize) -> bool {
    let be32 = |a: usize| u32::from_be_bytes(unsafe { read_volatile(a as *const [u8; 4]) });
    if be32(dtb) != 0xD00D_FEED {
        return false;
    }
    let (off_struct, off_strings, size_struct) = (be32(dtb + 8) as usize, be32(dtb + 12) as usize, be32(dtb + 36) as usize);
    let strings = dtb + off_strings;
    let (mut p, end) = (dtb + off_struct, dtb + off_struct + size_struct);
    let (mut depth, mut in_chosen) = (0usize, false);
    let (mut start, mut stop) = (0u64, 0u64);
    let name_is = |a: usize, want: &[u8]| -> bool {
        for (i, &c) in want.iter().enumerate() {
            if unsafe { read_volatile((a + i) as *const u8) } != c {
                return false;
            }
        }
        let last = unsafe { read_volatile((a + want.len()) as *const u8) };
        last == 0
    };
    while p + 4 <= end {
        let tok = be32(p);
        p += 4;
        match tok {
            1 => {
                depth += 1;
                in_chosen = depth == 2 && name_is(p, b"chosen");
                let mut n = 0;
                while unsafe { read_volatile((p + n) as *const u8) } != 0 {
                    n += 1;
                }
                p += (n + 1 + 3) & !3;
            }
            2 => {
                if in_chosen {
                    break;
                }
                depth -= 1;
            }
            3 => {
                let (len, nameoff) = (be32(p) as usize, be32(p + 4) as usize);
                let data = p + 8;
                if in_chosen {
                    let val = match len {
                        4 => be32(data) as u64,
                        8 => (be32(data) as u64) << 32 | be32(data + 4) as u64,
                        _ => 0,
                    };
                    if name_is(strings + nameoff, b"linux,initrd-start") {
                        start = val;
                    } else if name_is(strings + nameoff, b"linux,initrd-end") {
                        stop = val;
                    }
                }
                p = data + ((len + 3) & !3);
            }
            4 => {}
            _ => break,
        }
    }
    if start == 0 || stop <= start + 20 {
        return false;
    }
    let foot = stop as usize - 12;
    if !name_is_n(foot, b"#BOOTCONFIG\n") {
        return false;
    }
    let le32 = |a: usize| u32::from_le_bytes(unsafe { read_volatile(a as *const [u8; 4]) });
    let size = le32(foot - 8) as usize;
    let csum_at = foot - 4;
    if size == 0 || size > stop as usize - start as usize {
        return false;
    }
    let data = foot - 8 - size;
    let key = b"androidboot.slot_suffix = \"_b\"";
    let mut at = data;
    while at + key.len() <= data + size {
        if name_is_n(at, key) {
            let b_at = at + key.len() - 2;
            unsafe {
                write_volatile(b_at as *mut u8, b'a');
                write_volatile(csum_at as *mut u32, le32(csum_at).wrapping_sub(1));
                asm!("dc civac, {a}", "dc civac, {c}", "dsb sy", a = in(reg) b_at as u64, c = in(reg) csum_at as u64);
            }
            return true;
        }
        at += 1;
    }
    false
}

fn name_is_n(a: usize, want: &[u8]) -> bool {
    for (i, &c) in want.iter().enumerate() {
        if unsafe { read_volatile((a + i) as *const u8) } != c {
            return false;
        }
    }
    true
}

/// The gate's answer for ferros's DIAG, beside the mark: how (1 power, 2 timer, 3 volume down), when (ms from the screen), and how long the power pin read pressed from the start (ms; 0 = it was up at once).
fn gate_telemetry(how: u64, ms: u64, pressed_ms: u64) {
    unsafe {
        write_volatile((GATE_MARK_ADDR + 8) as *mut u64, how | ms << 8 | pressed_ms << 32);
        asm!("dsb sy");
    }
}

/// Enter ferros: copy the packed image to its link address, mark the gate answered, clean the instruction side, jump at EL2 with ABL's DTB in x0 as ABL would have. Never returns.
pub fn start_ferros(dtb: u64) -> ! {
    let (src, len) = ferros_image().expect("packed ferros image");
    copy(src, FERROS_LINK, (len + 15) & !15);
    unsafe {
        write_volatile(GATE_MARK_ADDR as *mut u64, GATE_MARK);
        asm!("dsb sy", "ic iallu", "dsb sy", "isb");
        asm!("br {e}", e = in(reg) FERROS_LINK, in("x0") dtb, options(noreturn));
    }
}
