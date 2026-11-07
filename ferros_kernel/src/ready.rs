//! The "Ready?" gate (BOOT.md): everything before it is vendor plumbing we tolerate; ferros's own boot starts when a human presses the power button.
//!
//! Before the press only what it takes to ask runs — the killswitch, the cache map, the panel, the buttons. The USB controller is held stopped, so nothing listens on any interface; the disk and the identity are untouched. Gathering entropy is neither, so KOHI runs during the wait and the human hides its cost.
//!
//! The screen: black, a green triangle on the right edge pointing at the power button, and the Google-coloured chevron at volume down. No words: the gate is drawn before the vault opens, so it cannot know the person's language; a symbol reads in every one (owner, 2026-10-27: "wordless"). It was "Ready?" until then; the gate keeps the name. The press — not the release — is the power key's falling-edge interrupt (killswitch.rs `arm_power_key`, `.Lirq_power`).
//!
//! Dev builds proceed as if pressed after one second, so unattended loops keep working and every line after the gate is the same code — unless a volume key goes down during that second, which cancels the timer: the gate then waits for the power key like a production build (FERROS_PRODUCTION set at build time), which always waits. There is no remote press: USB does not exist before the press.

/// The gate's two marks from the board's measured keys (board::keys): (centre row, half height, length) of the power triangle, as tall as the power key, centred on the power key and as tall as it, and of the volume-down chevron, its tip on the rocker's bottom edge (owner, 2026-11-06: "the arrow tip touches the bottom of the vol rocker") and a quarter of the rocker tall. The proportions are those of husky's marks first placed by eye (a 90-high half and 140 long; 48 and 80).
fn marks() -> ((usize, usize, usize), (usize, usize, usize)) {
    let k = crate::board::keys();
    let ph = (k.power.1 - k.power.0) / 2;
    let vh = (k.rocker.1 - k.rocker.0) / 8;
    (((k.power.0 + k.power.1) / 2, ph, ph * 14 / 9), (k.rocker.1, vh, vh * 5 / 3))
}
/// The gate's answer: power = ferros; volume down = Android (slot a made active, then a reset; BOOT.md "Dual Boot"). Volume up only holds (the dev auto-press is cancelled; a production build waits for power anyway).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Ferros,
    Android,
}
const WHITE: u32 = 0x00FF_FFFF;
const GREEN: u32 = 0x0000_FF00;
const DWC3_DCTL: usize = 0x1121_C704;

fn px(p: &crate::display::Panel, x: usize, y: usize, c: u32) {
    if x < p.width && y < p.rows {
        unsafe { core::ptr::write_volatile((p.fb as *mut u32).add(y * p.stride + x), c) };
    }
}

/// Black over exactly what `draw` put on the screen (the triangle and the chevron), so the boot ring starts from black without rewriting all four million pixels: the full clear cost ~370 ms of the press-to-ready time on the uncached surface (2026-10-22).
pub fn erase(p: &crate::display::Panel) {
    let rect = |xa: usize, ya: usize, xb: usize, yb: usize| {
        for y in ya..yb.min(p.rows) {
            for x in xa..xb.min(p.width) {
                px(p, x, y, 0);
            }
        }
    };
    let tip = p.width - 28;
    let ((py, ph, pl), (vy, vh, vl)) = marks();
    rect(tip - pl, py - ph - 1, tip + 1, py + ph + 2);
    rect(tip - vl, vy - vh - 1, tip + 1, vy + vh + 2);
}

fn draw(p: &crate::display::Panel) {
    let _ = WHITE;
    // Triangle pointing right, tip near the right edge at the power button's height.
    let ((py, ph, pl), (vy, vh, vl)) = marks();
    let (tip, half, len) = (p.width - 28, ph, pl);
    for dx in 0..len {
        let reach = half * dx / len;
        for dy in 0..=reach {
            px(p, tip - dx, py - dy, GREEN);
            px(p, tip - dx, py + dy, GREEN);
        }
    }
    // The Android choice at the volume-down key: a smaller chevron in Google's four colours, blue at the tip, so the key that leaves ferros is unmistakable.
    const GOOGLE: [u32; 4] = [0x004285F4, 0x00EA4335, 0x00FBBC05, 0x0034A853];
    let (tip, half, len) = (p.width - 28, vh, vl);
    for dx in 0..len {
        let reach = half * dx / len;
        let c = GOOGLE[(dx * 4 / len).min(3)];
        for dy in 0..=reach {
            px(p, tip - dx, vy - dy, c);
            px(p, tip - dx, vy + dy, c);
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

/// Show the screen and wait for the press. Returns the generic-timer count at the press and what was chosen. The keys answer it; a dev build answers itself after a second unless a volume key holds it. (Until 2026-11-01 a dev build brought USB up here so the host could answer too; the kernel has no USB of its own now.)
pub fn wait() -> (u64, Choice) {
    // Nothing answers on USB before the press: stop the controller ABL may have left running, so the host sees nothing until the USB process starts it.
    unsafe { core::ptr::write_volatile(DWC3_DCTL as *mut u32, core::ptr::read_volatile(DWC3_DCTL as *const u32) & !(1 << 31)) };
    let freq = crate::platform::cntfrq();
    let mut rep = alloc::vec::Vec::new();
    crate::checklist::at("gate: adopting panel");
    let panel = crate::display::adopt_once(freq, &mut rep);
    if let Some(panel) = &panel {
        crate::checklist::at("gate: panel adopted");
        draw(panel);
        crate::checklist::at("gate: drawn");
        let from = rep.len();
        panel.present(&mut rep);
        crate::checklist::at("gate: on screen");
        // What the present reported (frame counts, whether the shadow update was taken), kept for `diag boot`: on akita the gate is on the glass for a fraction of its second (owner, 2026-11-06), and this says whether its frame went out at all.
        unsafe { GATE_PRESENT = Some(rep[from..].to_vec()) };
        // akita's panel (AK3B, as ABL leaves it) does not hold a single frame: the one `present` sends goes out (shadow taken, the layer's FRAME_DONE) and the glass showed the gate for about 18 of its 60 frames (owner, 2026-11-06), the gate then answering unseen. DECON is left triggering on the panel's TE for the whole gate, a frame every refresh (62 in the second, measured), and the gate stays up its full second. husky's HK3 holds one frame, as before.
        if crate::board::akita() {
            panel.present_nowait();
        }
    }
    let held_at_arm = crate::killswitch::power_held();
    crate::killswitch::arm_power_key();
    crate::killswitch::enable_ppi(26);
    let t0 = crate::platform::cntpct();
    // The wairua's KOHI while the human decides (it is lazy everywhere else).
    let _ = crate::wairua::draw();
    crate::checklist::at("gate: KOHI done");
    // Dev: a volume key cancels the one-second auto-press for good (the keys' edge interrupts wake the WFI below, so a tap is seen at once).
    let mut held = false;
    let mut wiped = false;
    let mut how = 3u64;
    let mut choice = Choice::Ferros;
    loop {
        crate::platform::pet_watchdogs();
        if unsafe { core::ptr::read_volatile(&raw const crate::__power_pressed) } != 0 {
            how = 0;
            break;
        }
        // Volume down alone chooses Android (both keys together is the killswitch and never reaches here); volume up holds.
        if crate::killswitch::vol_down_held() && !crate::killswitch::vol_up_held() {
            choice = Choice::Android;
            how = 1;
            break;
        }
        // Volume up takes the marks off, once (owner, 2026-11-05: a clean screen to film the whole boot); the hv's gate does the same.
        if !wiped && crate::killswitch::vol_up_held() {
            wiped = true;
            if let Some(p) = &panel {
                erase(p);
                p.present_nowait();
            }
        }
        held |= crate::killswitch::volume_key_down();
        if cfg!(not(ferros_production)) && !held && crate::platform::cntpct().wrapping_sub(t0) > freq {
            how = 2;
            break;
        }
        // Sleep until the next interrupt: the press, or a timer wake every 100 ms to pet the watchdogs.
        let tick = freq / 10;
        unsafe { core::arch::asm!("msr cnthp_tval_el2, {t}", "mov {c}, #1", "msr cnthp_ctl_el2, {c}", "isb", "wfi", t = in(reg) tick, c = out(reg) _) };
    }
    if panel.is_some() {
        GATE_END_FRAMES.store(crate::display::decon_frames(), core::sync::atomic::Ordering::Relaxed);
    }
    GATE_HOW.store(how | (held_at_arm as u64) << 8 | (crate::killswitch::power_held() as u64) << 9, core::sync::atomic::Ordering::Relaxed);
    unsafe { core::arch::asm!("msr cnthp_ctl_el2, xzr", "isb") };
    (crate::platform::cntpct(), choice)
}

/// How this kernel's own gate ended, for `diag boot` (checklist.rs): bits 0-7 what answered it (0 the power key's press interrupt, 1 volume down, 2 the dev timer), bit 8 the power key already down when the gate armed, bit 9 the power key down when it answered. Measuring the owner's report of 2026-11-06 on akita, a gate gone in a third of a second where a boot nobody touched waited the full second.
pub static GATE_HOW: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The gate's present report (display::Panel::present), and DECON's frame count as the gate answered, for `diag boot`.
pub static mut GATE_PRESENT: Option<alloc::vec::Vec<u8>> = None;
pub static GATE_END_FRAMES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// The hv's boot gate (ferros_hv/src/gate.rs) leaves this word in its report page when it hands the machine to ferros: the human has already answered "ferros", so the gate is not shown twice. Read once and cleared, so a later boot that did not come through the hv never sees a stale answer.
const HV_GATE_MARK: u64 = 0x4741_5445_464F_5252; // "GATEFORR"

/// The hv gate's telemetry (gate.rs gate_telemetry): how it was answered (1 power, 2 timer, 3 volume down) in bits 0-7, ms from the screen in bits 8-31, ms the power pin read pressed from the start in bits 32-63. Kept for DIAG (GATE_ANSWER, GATE_MS, GATE_POWER_HELD_MS).
pub static GATE_TELEMETRY: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn answered_by_hv() -> bool {
    unsafe {
        let p = crate::board::hv_gate_mark() as *mut u64;
        let yes = core::ptr::read_volatile(p) == HV_GATE_MARK;
        if yes {
            GATE_TELEMETRY.store(core::ptr::read_volatile((crate::board::hv_gate_mark() + 8) as *const u64), core::sync::atomic::Ordering::Relaxed);
            core::ptr::write_volatile((crate::board::hv_gate_mark() + 8) as *mut u64, 0);
            core::ptr::write_volatile(p, 0);
            core::arch::asm!("dsb sy");
        }
        yes
    }
}
