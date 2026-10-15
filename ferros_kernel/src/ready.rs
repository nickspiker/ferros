//! The "Ready?" gate (BOOT.md): everything before it is vendor plumbing we tolerate; ferros's own boot starts when a human presses the power button.
//!
//! Before the press only what it takes to ask runs — the killswitch, the cache map, the panel, the buttons. The USB controller is held stopped, so nothing listens on any interface; the disk and the identity are untouched. Gathering entropy is neither, so KOHI runs during the wait and the human hides its cost.
//!
//! The screen: black, "Ready?" in thin Oxanium (rasterised at build time, build.rs) in the top third, and a green triangle on the right edge pointing at the power button. The press — not the release — is the power key's falling-edge interrupt (killswitch.rs `arm_power_key`, `.Lirq_power`).
//!
//! Dev builds proceed as if pressed after one second, so unattended loops keep working and every line after the gate is the same code — unless a volume key goes down during that second, which cancels the timer: the gate then waits for the power key like a production build (FERROS_PRODUCTION set at build time), which always waits. There is no remote press: USB does not exist before the press.

static READY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ready.bin"));

/// Panel row the power button sits beside (the triangle's tip points here). Calibrated by eye on husky.
const POWER_Y: usize = 1050;
const WHITE: u32 = 0x00FF_FFFF;
const GREEN: u32 = 0x0000_FF00;
const DWC3_DCTL: usize = 0x1121_C704;

fn px(p: &crate::display::Panel, x: usize, y: usize, c: u32) {
    if x < p.width && y < p.rows {
        unsafe { core::ptr::write_volatile((p.fb as *mut u32).add(y * p.stride + x), c) };
    }
}

fn draw(p: &crate::display::Panel) {
    let w = u32::from_le_bytes([READY[0], READY[1], READY[2], READY[3]]) as usize;
    let h = u32::from_le_bytes([READY[4], READY[5], READY[6], READY[7]]) as usize;
    let alpha = &READY[8..];
    let x0 = p.width.saturating_sub(w) / 2;
    let y0 = (p.rows / 6).saturating_sub(h / 2);
    for y in 0..h {
        for x in 0..w {
            let a = alpha[y * w + x] as u32;
            if a != 0 {
                // White over black: each channel is just the coverage.
                px(p, x0 + x, y0 + y, a << 16 | a << 8 | a);
            }
        }
    }
    let _ = WHITE;
    // Triangle pointing right, tip near the right edge at the power button's height.
    let (tip, half, len) = (p.width - 28, 90usize, 140usize);
    for dx in 0..len {
        let reach = half * dx / len;
        for dy in 0..=reach {
            px(p, tip - dx, POWER_Y - dy, GREEN);
            px(p, tip - dx, POWER_Y + dy, GREEN);
        }
    }
}

/// Draw the screen without waiting (dev: `bridge diag ready`). Returns (text pixels, triangle pixels) as found in the surface afterwards, so it can be checked without eyes on the panel.
pub fn show() -> (u32, u32) {
    let mut rep = alloc::vec::Vec::new();
    let Some(panel) = crate::display::adopt(crate::platform::cntfrq(), &mut rep) else { return (0, 0) };
    draw(&panel);
    panel.present(&mut rep);
    let (mut text, mut tri) = (0u32, 0u32);
    for y in 0..panel.rows {
        for x in 0..panel.width {
            let v = unsafe { core::ptr::read_volatile((panel.fb as *const u32).add(y * panel.stride + x)) };
            if v == GREEN {
                tri += 1;
            } else if v != 0 && v & 0xFF == (v >> 8) & 0xFF {
                text += 1;
            }
        }
    }
    (text, tri)
}

/// Show the screen and wait for the press. Returns the generic-timer count at the press.
pub fn wait() -> u64 {
    // Nothing answers on USB before the press: stop the controller ABL may have left running.
    unsafe { core::ptr::write_volatile(DWC3_DCTL as *mut u32, core::ptr::read_volatile(DWC3_DCTL as *const u32) & !(1 << 31)) };
    let freq = crate::platform::cntfrq();
    let mut rep = alloc::vec::Vec::new();
    if let Some(panel) = crate::display::adopt(freq, &mut rep) {
        draw(&panel);
        panel.present(&mut rep);
    }
    crate::killswitch::arm_power_key();
    crate::killswitch::enable_ppi(26);
    let t0 = crate::platform::cntpct();
    // The wairua's KOHI while the human decides (it is lazy everywhere else).
    let _ = crate::wairua::draw();
    // Dev: a volume key cancels the one-second auto-press for good (the keys' edge interrupts wake the WFI below, so a tap is seen at once).
    let mut held = false;
    loop {
        crate::platform::pet_watchdogs();
        if unsafe { core::ptr::read_volatile(&raw const crate::__power_pressed) } != 0 {
            break;
        }
        held |= crate::killswitch::volume_key_down();
        if cfg!(not(ferros_production)) && !held && crate::platform::cntpct().wrapping_sub(t0) > freq {
            break;
        }
        // Sleep until the next interrupt: the press, or a 100 ms timer wake to pet the watchdogs.
        unsafe { core::arch::asm!("msr cnthp_tval_el2, {t}", "mov {c}, #1", "msr cnthp_ctl_el2, {c}", "isb", "wfi", t = in(reg) freq / 10, c = out(reg) _) };
    }
    unsafe { core::arch::asm!("msr cnthp_ctl_el2, xzr", "isb") };
    crate::platform::cntpct()
}
