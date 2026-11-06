//! The bring-up checklist on the panel: every step from power-on to ferros's first process, red until it happens on this boot, green once it has (BOOT.md, "Ready?" gate).
//!
//! Steps that do not exist yet stay red — the screen is the road map. Three states: green filled = up and the real thing; yellow filled = up through a stand-in (the ira and wairua until PIPE); red open = not up yet. Cold boot only: the handoff path has no live panel unless it came up with `--live`.

use crate::display::Panel;
use core::cell::UnsafeCell;

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum Item {
    Killswitch,
    CacheMap,
    Panel,
    Disk,
    VaultOpen,
    VaultVerified,
    Ira,
    Wairua,
    Slot,
    Usb,
    El0,
    Syscall,
    Arcs,
    Caps,
    Ipc,
    Spawn,
    Toka,
    FirstProcess,
}

const GROUPS: [(&str, &[(Item, &str)]); 3] = [
    ("plumbing", &[
        (Item::Killswitch, "killswitch armed"),
        (Item::CacheMap, "cache map: DRAM cached"),
        (Item::Panel, "panel adopted"),
    ]),
    ("boot", &[
        (Item::Disk, "disk: UFS link"),
        (Item::VaultOpen, "vault opened"),
        (Item::VaultVerified, "vault verified"),
        (Item::Ira, "ira"),
        (Item::Wairua, "wairua"),
        (Item::Slot, "slot marked"),
        (Item::Usb, "usb: PT link"),
    ]),
    ("kernel", &[
        (Item::El0, "EL0 entry + arc registers"),
        (Item::Syscall, "syscall boundary"),
        (Item::Arcs, "arc allocator"),
        (Item::Caps, "capabilities: CSpace + grants"),
        (Item::Ipc, "IPC: call / reply"),
        (Item::Spawn, "spawn + oracle ihi"),
        (Item::Toka, "toka VM process"),
        (Item::FirstProcess, "first process: hello"),
    ]),
];

const TITLE: u32 = 0x00FF_8000;
const HEADER: u32 = 0x00FF_FF00;
const RED: u32 = 0x00FF_0000;
const YELLOW: u32 = 0x00FF_FF00;
const GREEN: u32 = 0x0000_FF00;
const SCALE: usize = 3;
// Console geometry at SCALE (console margins: 160 top, 48 left; 8x16 font).
const TOP: usize = 160;
const LEFT: usize = 48;
const CHAR_W: usize = 8 * SCALE;
const LINE_H: usize = 16 * SCALE;
/// The label starts after the circle.
const LABEL_COL: usize = 3;

/// Text shown after an item's label (a fingerprint); same color as the item.
const SUFFIX_MAX: usize = 16;

struct Checklist {
    panel: Panel,
    done: u32,
    /// Up through a stand-in (subset of `done`): drawn yellow.
    interim: u32,
    suffix: [([u8; SUFFIX_MAX], usize); 32],
}

struct Global(UnsafeCell<Option<Checklist>>);
// Single core, never touched from the IRQ handler.
unsafe impl Sync for Global {}
static CHECKLIST: Global = Global(UnsafeCell::new(None));

fn row_of(item: Item) -> usize {
    // Title row 0, blank, then per group: header row, its items, blank.
    let mut row = 2;
    for (_, items) in GROUPS.iter() {
        row += 1;
        for &(it, _) in items.iter() {
            if it as u8 == item as u8 {
                return row;
            }
            row += 1;
        }
        row += 1;
    }
    row
}

/// Up: a filled circle. Not yet: an open one. Drawn straight into the surface, centred in the first character cells of the row.
fn draw_circle(panel: &Panel, row: usize, filled: bool, color: u32) {
    let cx = (LEFT + CHAR_W * 3 / 2) as isize;
    let cy = (TOP + row * LINE_H + LINE_H / 2) as isize;
    let r = (LINE_H * 3 / 10) as isize;
    let inner = r - 4;
    for dy in -r..=r {
        for dx in -r..=r {
            let d2 = dx * dx + dy * dy;
            let on = d2 <= r * r && (filled || d2 > inner * inner);
            let (x, y) = ((cx + dx) as usize, (cy + dy) as usize);
            if y < panel.rows && x < panel.width {
                unsafe { core::ptr::write_volatile((panel.fb as *mut u32).add(y * panel.stride + x), if on { color } else { 0 }) };
            }
        }
    }
}

fn draw_item(c: &Checklist, item: Item, label: &str) {
    let done = c.done & (1 << item as u8) != 0;
    let color = if !done { RED } else if c.interim & (1 << item as u8) != 0 { YELLOW } else { GREEN };
    let row = row_of(item);
    draw_circle(&c.panel, row, done, color);
    // The label in Oxanium (text.rs), the row's text area cleared first so a redraw does not blend over the last one.
    let sf = surface(&c.panel);
    let (x, top) = ((LEFT + LABEL_COL * CHAR_W) as i32, (TOP + row * LINE_H) as i32);
    sf.fill(x as usize, top as usize, c.panel.width.saturating_sub(x as usize + LEFT), LINE_H, 0);
    let pen = sf.draw(x, top, label, color);
    let (text, len) = &c.suffix[item as usize];
    if *len > 0 {
        let pen = sf.draw(pen, top, " ", color);
        sf.draw(pen, top, core::str::from_utf8(&text[..*len]).unwrap_or("?"), color);
    }
}

/// The panel as a surface for text.rs.
fn surface(p: &Panel) -> crate::text::Surface {
    crate::text::Surface { fb: p.fb as *mut u32, width: p.width, height: p.rows, stride: p.stride }
}

/// Adopt the panel and draw the whole list; `done` items start green. No-op (and every later `mark` too) if there is no panel to drive.
pub fn start(cntfrq: u64, done: &[Item]) {
    let mut rep = alloc::vec::Vec::new();
    let Some(panel) = crate::display::adopt_once(cntfrq, &mut rep) else {
        return;
    };
    let _ = rep;
    let mut c = Checklist { panel, done: 1 << Item::Panel as u8, interim: 0, suffix: [([0u8; SUFFIX_MAX], 0); 32] };
    for &d in done {
        c.done |= 1 << d as u8;
    }
    load_scale();
    ring_start(&c.panel);
    c.panel.present_nowait();
    unsafe { *CHECKLIST.0.get() = Some(c) };
}

// ---------------------------------------------------------------------------
// The boot ring (owner's design 2026-10-22, redrawn to the owner's direction 2026-11-05): the attest screen's circle (ahau `mark_at`: the same centre and outer radius), a solid disc in its middle for how this boot began, and the boot's work around it.
// - Angle is a share of the boot: the full circle is the whole boot, from the press to init having run, clockwise from 12 o'clock, and each wedge spans its step from when it began to when it ended, so its share of the circle is its share of the boot. The boot's length is known only when it ends, so the wedges are drawn to the previous boot's length (kept in the report page, which survives a reset): this boot closes the circle as far as it matches the last. After a power-on there is no previous length, and the ring is drawn once, exactly, when init has run. No timer anywhere: every wedge is drawn when its step ends. The black between wedges is the ring's own drawing.
// - Lanes are cores: the boot path in the inner lane, work running beside it on another core (KOHI's TRNG words on worker 3) in the outer lane, at the same angles while they overlap. The two lanes have the same area, so a wedge's area is its duration whichever lane it is in.
// - Colour is order: dark brick, brick, amber, then a bright lime, along the warm side only (nothing between magenta and green).
// - The centre disc's colour is the boot's reason (`Reason`).
// All integer: a Q14 quarter-wave sine table interpolated to 1/2^18 turn, and cross products, each wedge drawn only inside its own bounding box.

/// How this boot began, for the centre disc (main.rs `set_reason`, before the ring is drawn).
#[derive(Clone, Copy)]
#[repr(u8)]
pub enum Reason {
    /// Nothing survived in RAM: the power came on.
    Cold,
    /// RAM survived a reset (the previous session's report is still there), or the hv's chord reset into ferros. A watchdog bite reads as this too.
    Restart,
    /// The hv's jump from a running Android.
    Jump,
    /// A RELOAD from the host: a new kernel on a device that has already booted.
    Reload,
    /// The previous session ended in a panic (its report's crumb G#EExx).
    Crash,
}
const REASON_NAME: [&str; 5] = ["cold", "restart", "jump from Android", "reload", "after a panic"];
const REASON_RGB: [u32; 5] = [0x003C_0200, 0x003C_2C00, 0x0000_1A3C, 0x002C_003C, 0x009C_0000];
static REASON: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

pub fn set_reason(r: Reason) {
    REASON.store(r as u8, core::sync::atomic::Ordering::Relaxed);
}

/// The boot's steps on the ring, in the order the boot path runs them (the last two on the service core: the wairua, which waits for KOHI's words, then init), and KOHI beside them on worker 3.
#[derive(Clone, Copy)]
#[repr(u8)]
pub enum Step {
    Repeater,
    Panel,
    Board,
    Disk,
    Log,
    Ira,
    Vault,
    Verify,
    Protect,
    Slot,
    Settings,
    Wairua,
    Init,
    Kohi,
}
const STEPS: usize = 14;
const STEP_NAME: [&str; STEPS] = ["eUSB repeater tuned", "panel adopted", "board read", "disk: UFS link", "flash log", "ira", "vault opened", "vault verified", "flash write-protect", "slot marked", "settings + service", "wairua", "init", "KOHI: TRNG, worker 3"];
/// 0: the boot path (inner lane). 1: beside it on another core (outer lane).
const STEP_LANE: [u8; STEPS] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
static BEGIN: [core::sync::atomic::AtomicU64; STEPS] = [const { core::sync::atomic::AtomicU64::new(0) }; STEPS];
static END: [core::sync::atomic::AtomicU64; STEPS] = [const { core::sync::atomic::AtomicU64::new(0) }; STEPS];
/// Where the boot path's last step ended (after its wedge was drawn): the next one begins there.
static PATH_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The ring's own drawing on the boot path (the disc, the wedges, the frame pushes), summed in counter ticks, for `diag boot`.
static DRAW_TICKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The ring's zero: the press, or the kernel's entry when there was none (a RELOAD).
fn zero() -> u64 {
    use core::sync::atomic::Ordering::Relaxed;
    let p = PRESS.load(Relaxed);
    if p != 0 { p } else { ENTRY.load(Relaxed) }
}

/// The last boot's length (press to init, µs) in the report page, beside the hand-over stamp: DRAM, so it survives a reset and not a power-on. The top half is a tag, so whatever a power-on left there is not taken for a length.
const SPAN_AT: usize = crate::platform::RAMOOPS_RESULT + 0x2D0;
const SPAN_TAG: u64 = 0x5350_414E; // "SPAN"
/// The full circle in counter ticks: the last boot's length, or 0 while unknown (the wedges wait for `close`).
static CIRCLE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// What the ring was scaled to and this boot's own length (µs), for `diag boot`.
static SCALE_US: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static SPAN_US: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The circle is closed (init has run): later steps (a wairua drawn again, say) are not the boot's.
static CLOSED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Read the last boot's length for the scale (`start`, before the first wedge).
fn load_scale() {
    use core::sync::atomic::Ordering::Relaxed;
    let w = unsafe { core::ptr::read_volatile(SPAN_AT as *const u64) };
    // The page is DRAM nothing initialises: after a power-on the word is noise, so it counts only under its tag, and a zero length would divide by zero.
    if w >> 32 == SPAN_TAG && w as u32 != 0 {
        let us = (w as u32) as u64;
        SCALE_US.store(us, Relaxed);
        CIRCLE.store(us * crate::platform::cntfrq() / 1_000_000, Relaxed);
    }
}

/// init has run: the boot is over. Keep its length for the next boot's scale, and if this one had none (a power-on), draw the whole ring now, exactly to this boot's length.
pub fn close() {
    use core::sync::atomic::Ordering::Relaxed;
    if CLOSED.swap(true, Relaxed) {
        return;
    }
    let e = END[Step::Init as usize].load(Relaxed);
    let z = zero();
    if e <= z {
        return;
    }
    let ticks = e - z;
    let us = ticks * 1_000_000 / crate::platform::cntfrq();
    SPAN_US.store(us, Relaxed);
    unsafe {
        core::ptr::write_volatile(SPAN_AT as *mut u64, SPAN_TAG << 32 | (us as u32) as u64);
        core::arch::asm!("dc civac, {a}", "dsb sy", a = in(reg) SPAN_AT as u64);
    }
    if CIRCLE.load(Relaxed) == 0 {
        CIRCLE.store(ticks, Relaxed);
        SCALE_US.store(us, Relaxed);
        draw_new();
    }
}

/// The boot's length on the disk too, for a power-on's scale (the report page does not survive one): a spec VSF document under the vault key `boot`, one section `boot` with `span_us`, written once init has run (service.rs) and read right after the vault opens (main.rs). A power-on draws the wedges so far once it is read, then each as its step ends.
pub const SPAN_KEY: &str = "boot";
fn span_schema() -> vsf::schema::SectionSchema {
    vsf::schema::SectionSchema::new("boot").field("span_us", vsf::schema::TypeConstraint::AnyUnsigned)
}

/// This boot's length as the document to keep, once the ring is closed.
pub fn span_document() -> Option<alloc::vec::Vec<u8>> {
    let us = SPAN_US.load(core::sync::atomic::Ordering::Relaxed);
    if us == 0 {
        return None;
    }
    let section = span_schema().build().set("span_us", vsf::VsfType::u(us as usize, false)).ok()?.encode().ok()?;
    vsf::VsfBuilder::new().creation_time_oscillations(crate::settings::eagle_now()).provenance_only().add_unboxed("boot", section).build().ok()
}

/// The last boot's length from the disk, when the report page had none: the ring is scaled now and draws what it has.
pub fn span_from_disk(doc: &[u8]) {
    use core::sync::atomic::Ordering::Relaxed;
    if CIRCLE.load(Relaxed) != 0 {
        return;
    }
    let Ok(section) = vsf::schema::SectionBuilder::parse_document(span_schema(), doc, None) else { return };
    let Some(us) = section.get_fields("span_us").first().and_then(|f| f.values.first()).and_then(|v| v.as_u64()) else { return };
    // Read from the disk: a zero would divide by zero.
    if us == 0 {
        return;
    }
    SCALE_US.store(us, Relaxed);
    CIRCLE.store(us * crate::platform::cntfrq() / 1_000_000, Relaxed);
    draw_new();
}

/// A boot-path step has ended: it began where the last one ended (the ring's zero for the first). Drawn now if the ring is up and scaled; the drawing's time is left out of the next step, so it shows as the black between wedges.
pub fn step(s: Step) {
    use core::sync::atomic::Ordering::{Relaxed, Release};
    if CLOSED.load(Relaxed) {
        return;
    }
    let b = PATH_AT.load(Relaxed);
    BEGIN[s as usize].store(if b != 0 { b } else { zero() }, Relaxed);
    END[s as usize].store(crate::platform::cntpct(), Release);
    draw_new();
    PATH_AT.store(crate::platform::cntpct(), Relaxed);
}

/// A step on another core began (stamped from any core; drawn by the boot path's or the service loop's next `draw_new` after its `end`).
pub fn begin(s: Step) {
    BEGIN[s as usize].store(crate::platform::cntpct(), core::sync::atomic::Ordering::Relaxed);
}

/// A step on another core ended.
pub fn end(s: Step) {
    END[s as usize].store(crate::platform::cntpct(), core::sync::atomic::Ordering::Release);
}

/// Quarter-wave sine, Q14, 257 entries over 0..=90 degrees.
const SIN_Q14: [i32; 257] = [0, 101, 201, 302, 402, 503, 603, 704, 804, 904, 1005, 1105, 1205, 1306, 1406, 1506, 1606, 1706, 1806, 1906, 2006, 2105, 2205, 2305, 2404, 2503, 2603, 2702, 2801, 2900, 2999, 3098, 3196, 3295, 3393, 3492, 3590, 3688, 3786, 3883, 3981, 4078, 4176, 4273, 4370, 4467, 4563, 4660, 4756, 4852, 4948, 5044, 5139, 5235, 5330, 5425, 5520, 5614, 5708, 5803, 5897, 5990, 6084, 6177, 6270, 6363, 6455, 6547, 6639, 6731, 6823, 6914, 7005, 7096, 7186, 7276, 7366, 7456, 7545, 7635, 7723, 7812, 7900, 7988, 8076, 8163, 8250, 8337, 8423, 8509, 8595, 8680, 8765, 8850, 8935, 9019, 9102, 9186, 9269, 9352, 9434, 9516, 9598, 9679, 9760, 9841, 9921, 10001, 10080, 10159, 10238, 10316, 10394, 10471, 10549, 10625, 10702, 10778, 10853, 10928, 11003, 11077, 11151, 11224, 11297, 11370, 11442, 11514, 11585, 11656, 11727, 11797, 11866, 11935, 12004, 12072, 12140, 12207, 12274, 12340, 12406, 12472, 12537, 12601, 12665, 12729, 12792, 12854, 12916, 12978, 13039, 13100, 13160, 13219, 13279, 13337, 13395, 13453, 13510, 13567, 13623, 13678, 13733, 13788, 13842, 13896, 13949, 14001, 14053, 14104, 14155, 14206, 14256, 14305, 14354, 14402, 14449, 14497, 14543, 14589, 14635, 14680, 14724, 14768, 14811, 14854, 14896, 14937, 14978, 15019, 15059, 15098, 15137, 15175, 15213, 15250, 15286, 15322, 15357, 15392, 15426, 15460, 15493, 15525, 15557, 15588, 15619, 15649, 15679, 15707, 15736, 15763, 15791, 15817, 15843, 15868, 15893, 15917, 15941, 15964, 15986, 16008, 16029, 16049, 16069, 16088, 16107, 16125, 16143, 16160, 16176, 16192, 16207, 16221, 16235, 16248, 16261, 16273, 16284, 16295, 16305, 16315, 16324, 16332, 16340, 16347, 16353, 16359, 16364, 16369, 16373, 16376, 16379, 16381, 16383, 16384, 16384];
/// Angles in 1/1024 turns for the table, 0 at 12 o'clock, clockwise.
const TURN: i32 = 1024;
fn sin_t(t: i32) -> i32 {
    let t = t.rem_euclid(TURN);
    match t / 256 {
        0 => SIN_Q14[t as usize],
        1 => SIN_Q14[(512 - t) as usize],
        2 => -SIN_Q14[(t - 512) as usize],
        _ => -SIN_Q14[(TURN - t) as usize],
    }
}
/// Fine angles: 1/2^18 turn (a quarter is 2^16), the table interpolated across its 256ths, so a wedge's angle follows its µs rather than whole milliseconds.
const FINE: i32 = 1 << 18;
const QUARTER: i32 = 1 << 16;
fn sin_f(a: i32) -> i32 {
    let (t, f) = (a >> 8, a & 255);
    let (s0, s1) = (sin_t(t), sin_t(t + 1));
    s0 + ((s1 - s0) * f >> 8)
}
/// The screen direction of fine angle `a`, Q14: x right, y down, 0 straight up.
fn dir(a: i32) -> (i32, i32) {
    (sin_f(a), -sin_f(a + QUARTER))
}

/// Integer square root (Newton), for the lanes' equal-area boundary.
fn isqrt(n: i32) -> i32 {
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// The ring's geometry, from the panel's width as the attest circle's (ahau `mark_at`): centre x, centre y, and the radii — the reason disc, the inner lane, the outer lane — each lane's (from, to).
struct Ring {
    cx: i32,
    cy: i32,
    disc: i32,
    lanes: [(i32, i32); 2],
    /// The dark line along each wedge's start, so neighbouring steps read apart.
    seam: i32,
}
fn ring(p: &Panel) -> Ring {
    let w = p.width as i32;
    let r = w / 3;
    let top = w * 473 / 5376 + w / 64;
    // The lanes span r/2 to r, split where the areas match (r_mid² is the mean of r_in² and r_out²), with a gap of r/64 between them.
    let r_in = r / 2;
    let mid = isqrt((r_in * r_in + r * r) / 2);
    let gap = r >> 6;
    Ring { cx: w / 2, cy: top + r, disc: r_in - (r >> 4), lanes: [(r_in, mid - gap / 2), (mid + gap - gap / 2, r)], seam: r >> 8 }
}

/// The colour ramp's stops, warm side only.
const RAMP: [u32; 4] = [0x003C_0200, 0x00BB_3700, 0x00E4_B200, 0x00B4_FF00];

fn ramp(i: u32, n: u32) -> u32 {
    let n = n.max(2);
    let pos = (i.min(n - 1) * 3 * 256) / (n - 1);
    let (seg, f) = ((pos / 256).min(2) as usize, if pos >= 768 { 256 } else { pos % 256 });
    let (a, b) = (RAMP[seg], RAMP[seg + 1]);
    let ch = |sh: u32| -> u32 {
        let (x, y) = (((a >> sh) & 0xFF) as i32, ((b >> sh) & 0xFF) as i32);
        ((x + (y - x) * f as i32 / 256) as u32) << sh
    };
    ch(16) | ch(8) | ch(0)
}

fn put(p: &Panel, x: i32, y: i32, rgb: u32) {
    if x >= 0 && y >= 0 && (x as usize) < p.width && (y as usize) < p.rows {
        unsafe { core::ptr::write_volatile((p.fb as *mut u32).add(y as usize * p.stride + x as usize), rgb) };
    }
}

fn ring_start(p: &Panel) {
    // The gate left the screen black but for its own word and arrows: clear just those (the gate is cold-only; ready.rs is not built for the handoff).
    #[cfg(not(shim_handoff))]
    crate::ready::erase(p);
    let t_d = crate::platform::cntpct();
    let g = ring(p);
    let reason = REASON.load(core::sync::atomic::Ordering::Relaxed) as usize;
    let rgb = REASON_RGB[reason];
    for y in -g.disc..=g.disc {
        for x in -g.disc..=g.disc {
            if x * x + y * y <= g.disc * g.disc {
                put(p, g.cx + x, g.cy + y, rgb);
            }
        }
    }
    DRAW_TICKS.fetch_add(crate::platform::cntpct() - t_d, core::sync::atomic::Ordering::Relaxed);
}

/// One wedge: the annulus [r1, r2) between fine angles a0 and a1, split into pieces of at most a quarter turn so each is a convex sector the cross-product test can take. The first `seam` pixels along the start ray stay dark.
fn wedge(p: &Panel, g: &Ring, r1: i32, r2: i32, a0: i32, a1: i32, rgb: u32) {
    let mut a = a0;
    while a < a1 {
        let b = (a + QUARTER).min(a1);
        let (d0, d1) = (dir(a), dir(b));
        // Bounding box: the four corners, plus any axis extreme the piece crosses.
        let mut xs = [0i32; 8];
        let mut ys = [0i32; 8];
        let mut n = 0;
        for &(d, r) in [(d0, r1), (d0, r2), (d1, r1), (d1, r2)].iter() {
            xs[n] = d.0 * r >> 14;
            ys[n] = d.1 * r >> 14;
            n += 1;
        }
        for q in [0, QUARTER, 2 * QUARTER, 3 * QUARTER] {
            let qa = if q < a { q + FINE } else { q };
            if qa > a && qa < b {
                let d = dir(q);
                xs[n] = d.0 * r2 >> 14;
                ys[n] = d.1 * r2 >> 14;
                n += 1;
            }
        }
        let (x0, x1) = (*xs[..n].iter().min().unwrap() - 1, *xs[..n].iter().max().unwrap() + 1);
        let (y0, y1) = (*ys[..n].iter().min().unwrap() - 1, *ys[..n].iter().max().unwrap() + 1);
        // The seam on the wedge's first piece only: c0 is the distance from the start ray in Q14 (the direction is unit length in Q14).
        let seam = if a == a0 { (g.seam as i64) << 14 } else { 0 };
        for y in y0..=y1 {
            for x in x0..=x1 {
                let rr = x * x + y * y;
                if rr < r1 * r1 || rr >= r2 * r2 {
                    continue;
                }
                // Inside the piece: on the clockwise side of its start ray and the anticlockwise side of its end ray.
                let c0 = d0.0 as i64 * y as i64 - d0.1 as i64 * x as i64;
                let c1 = x as i64 * d1.1 as i64 - y as i64 * d1.0 as i64;
                if c0 >= seam && c1 >= 0 {
                    put(p, g.cx + x, g.cy + y, rgb);
                }
            }
        }
        a = b;
    }
}

/// Diagnostic (2026-11-05: the owner sees two thirds of the ring, then about 80 ms of black): DECON's frame count and trigger register as the kernel sees them, a line whenever the trigger register changes or frames start again after a stop of more than 16 ms, until 64 lines. `diag boot` prints them.
static mut FTRACE: [(u64, u32, u32, u64); 64] = [(0, 0, 0, 0); 64];
static mut FTRACE_N: usize = 0;
/// The last frame count and trigger seen, and when the frame count last moved.
static mut FT_LAST: (u32, u32, u64) = (0, 0, 0);

/// One look at DECON (the boot path's draws and the service loop's every pass).
pub fn frame_trace() {
    unsafe {
        if FTRACE_N >= 64 {
            return;
        }
        let now = crate::platform::cntpct();
        let fc = core::ptr::read_volatile(0x1947_0004usize as *const u32);
        let trig = core::ptr::read_volatile(0x1947_0030usize as *const u32);
        let (fc0, trig0, moved) = FT_LAST;
        let stopped = now - moved > crate::platform::cntfrq() / 64;
        if trig != trig0 || (fc != fc0 && stopped) || moved == 0 {
            FTRACE[FTRACE_N] = (now, fc, trig, moved);
            FTRACE_N += 1;
        }
        FT_LAST = (fc, trig, if fc != fc0 || moved == 0 { now } else { moved });
    }
}

/// Which steps are on the ring, and how many.
static mut RING_DRAWN: u32 = 0;
static mut RING_N: u32 = 0;

/// Draw every step that has ended and is not on the ring yet, then push a frame if any was. The boot path calls it at each step, then the service loop on every pass until home (for the steps on other cores): one core at a time, never from an IRQ.
pub fn draw_new() {
    use core::sync::atomic::Ordering::{Acquire, Relaxed};
    let Some(c) = (unsafe { (*CHECKLIST.0.get()).as_mut() }) else {
        return;
    };
    // No scale yet (a power-on): everything waits for `close`.
    let f = CIRCLE.load(Relaxed);
    if f == 0 {
        return;
    }
    frame_trace();
    let t_draw = crate::platform::cntpct();
    let z = zero();
    let g = ring(&c.panel);
    let mut drew = false;
    for s in 0..STEPS {
        let e = END[s].load(Acquire);
        if e == 0 || unsafe { RING_DRAWN } & (1 << s) != 0 {
            continue;
        }
        unsafe { RING_DRAWN |= 1 << s };
        // A step that ended before the ring's zero (KOHI finishing during a long wait at the gate) took none of the boot's time: it has no wedge and no row.
        if e <= z {
            continue;
        }
        // One that began before the zero (KOHI starts before the gate) is drawn from the zero: the time before the press is the person's, not the boot's.
        let b = BEGIN[s].load(Relaxed);
        let b = if b > z { b } else { z };
        let fine = |t: u64| ((t - z) * FINE as u64 / f) as i64;
        let a0 = fine(b);
        // A boot longer than the last runs past the circle: that time is off the ring (`diag boot` still lists it).
        let a1 = if fine(e) < FINE as i64 { fine(e) } else { FINE as i64 };
        let n = unsafe { RING_N };
        let rgb = ramp(n, STEPS as u32);
        if a0 < a1 {
            let (r1, r2) = g.lanes[STEP_LANE[s] as usize];
            wedge(&c.panel, &g, r1, r2, a0 as i32, a1 as i32, rgb);
        }
        unsafe { RING_N = n + 1 };
        drew = true;
    }
    if drew {
        c.panel.present_nowait();
        DRAW_TICKS.fetch_add(crate::platform::cntpct() - t_draw, Relaxed);
        frame_trace();
    }
}

/// Boot timing: the counter when the cold kernel was entered (set right after the killswitch arms) and, per item, the counter when it was first marked. Kept apart from the drawn list so items marked before the panel is adopted still get a time. Read by `diag boot`.
pub static ENTRY: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static PRESS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// When the shell was started (the ring complete): press to here is the boot the owner waits for.
pub static HOME: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static AT: [core::sync::atomic::AtomicU64; 18] = [const { core::sync::atomic::AtomicU64::new(0) }; 18];

/// Named boot stamps between the checklist items (`at`), for finding where the time goes: up to 32, in order.
static NAMED: [core::sync::atomic::AtomicU64; 32] = [const { core::sync::atomic::AtomicU64::new(0) }; 32];
static NAMES: [core::sync::atomic::AtomicPtr<u8>; 32] = [const { core::sync::atomic::AtomicPtr::new(core::ptr::null_mut()) }; 32];
static NAME_LEN: [core::sync::atomic::AtomicUsize; 32] = [const { core::sync::atomic::AtomicUsize::new(0) }; 32];
static NAMED_N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Stamp the counter under `name` (first boot only matters; later calls past 32 are dropped).
pub fn at(name: &'static str) {
    use core::sync::atomic::Ordering::Relaxed;
    let i = NAMED_N.fetch_add(1, Relaxed);
    if i < 32 {
        NAMED[i].store(crate::platform::cntpct(), Relaxed);
        NAMES[i].store(name.as_ptr() as *mut u8, Relaxed);
        NAME_LEN[i].store(name.len(), Relaxed);
    }
}

fn stamp(item: Item) {
    let _ = AT[item as usize].compare_exchange(0, crate::platform::cntpct(), core::sync::atomic::Ordering::Relaxed, core::sync::atomic::Ordering::Relaxed);
}

/// `diag boot`: milliseconds from reset to ferros's entry, then each checklist item's time since entry, in the order they happened.
pub fn boot_times() -> alloc::string::String {
    use core::sync::atomic::Ordering::Relaxed;
    let f = crate::platform::cntfrq();
    let us = |t: u64| t * 1_000_000 / f;
    let e = ENTRY.load(Relaxed);
    let mut s = alloc::format!("entry A#{} ms after reset\n", us(e) / 1000);
    let p = PRESS.load(Relaxed);
    if p != 0 {
        s.push_str(&alloc::format!("  gate answered      +A#{} us\n", us(p.saturating_sub(e))));
    }
    let mut v: alloc::vec::Vec<(u64, &'static str)> = alloc::vec::Vec::new();
    for (_, items) in GROUPS.iter() {
        for &(item, label) in items.iter() {
            let t = AT[item as usize].load(Relaxed);
            if t != 0 {
                v.push((t, label));
            }
        }
    }
    let n = NAMED_N.load(Relaxed).min(32);
    for i in 0..n {
        let name = unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(NAMES[i].load(Relaxed), NAME_LEN[i].load(Relaxed))) };
        v.push((NAMED[i].load(Relaxed), name));
    }
    v.sort_by_key(|x| x.0);
    for (t, label) in v {
        s.push_str(&alloc::format!("  {label:<28} +A#{} us\n", us(t.saturating_sub(e))));
    }
    // The ring's steps (lane 0 the boot path, 1 another core): when each began and ended, from the ring's zero, and the ring's own drawing.
    let z = zero();
    s.push_str(&alloc::format!("ring, from the {} at counter A#{} (A#{} a second; processes log the same counter) (boot: {})\n", if PRESS.load(Relaxed) != 0 { "press" } else { "entry" }, z, f, REASON_NAME[REASON.load(Relaxed) as usize]));
    // DECON as the kernel saw it (frame_trace): when, the frame count, the trigger register, and when the count had last moved.
    for k in 0..unsafe { FTRACE_N } {
        let (t, fc, trig, moved) = unsafe { FTRACE[k] };
        let at = |t: u64| if t >= z { alloc::format!("+A#{}", us(t - z)) } else { alloc::format!("-A#{}", us(z - t)) };
        s.push_str(&alloc::format!("  decon {} us: frames A#{} trigger G#{:X}, frames last moved {} us\n", at(t), fc, trig, at(moved)));
    }
    for k in 0..STEPS {
        let (b, en) = (BEGIN[k].load(Relaxed), END[k].load(Relaxed));
        if en != 0 {
            let at = |t: u64| if t >= z { alloc::format!("+A#{}", us(t - z)) } else { alloc::format!("-A#{}", us(z - t)) };
            s.push_str(&alloc::format!("  lane {} {:<22} {} .. {} us, A#{} us\n", STEP_LANE[k], STEP_NAME[k], at(b), at(en), us(en - b)));
        }
    }
    s.push_str(&alloc::format!("  the ring's drawing A#{} us over A#{} steps; the circle A#{} us (0: none, drawn at the end), this boot A#{} us\n", us(DRAW_TICKS.load(Relaxed)), unsafe { RING_N }, SCALE_US.load(Relaxed), SPAN_US.load(Relaxed)));
    s
}

/// An item is up: its time for `diag boot`. The ring draws `Step`s, not items (since 2026-11-05: an item is often known only after the work that brought it up, so its mark is not where its step began or ended).
pub fn mark(item: Item) {
    stamp(item);
    let Some(c) = (unsafe { (*CHECKLIST.0.get()).as_mut() }) else {
        return;
    };
    c.done |= 1 << item as u8;
}

/// Mark an item up through a stand-in (yellow).
pub fn mark_interim(item: Item) {
    stamp(item);
    let Some(c) = (unsafe { (*CHECKLIST.0.get()).as_mut() }) else {
        return;
    };
    c.done |= 1 << item as u8;
    c.interim |= 1 << item as u8;
}

/// Put `text` after an item's label (truncated to 16 bytes).
pub fn suffix(item: Item, text: &[u8]) {
    let Some(c) = (unsafe { (*CHECKLIST.0.get()).as_mut() }) else {
        return;
    };
    let n = text.len().min(SUFFIX_MAX);
    c.suffix[item as usize].0[..n].copy_from_slice(&text[..n]);
    c.suffix[item as usize].1 = n;
}

/// Whether the checklist owns the panel on this boot (then nothing else should draw over it).
pub fn active() -> bool {
    unsafe { (*CHECKLIST.0.get()).is_some() }
}

/// akita bring-up (board.rs): square `n` (0 to 15) of a row under the ring, lit green and presented at once, so a step that hangs shows on the glass as the last square lit. On akita the panel is the one channel left while USB never comes up: its bootloader clears the RAM log at every reset. Nothing on husky.
pub fn dot(n: u32) {
    if !crate::board::akita() {
        return;
    }
    let Some(c) = (unsafe { (*CHECKLIST.0.get()).as_ref() }) else {
        return;
    };
    let p = &c.panel;
    let side = (p.width / 24) as i32;
    let pitch = side * 3 / 2;
    let x0 = (p.width as i32 - 16 * pitch) / 2 + n as i32 * pitch;
    let y0 = (p.rows * 7 / 8) as i32;
    for y in y0..y0 + side {
        for x in x0..x0 + side {
            put(p, x, y, 0x00_40_E0_40);
        }
    }
    p.present_nowait();
}
