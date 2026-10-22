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
    unsafe {
        let mut con = c.panel.console(color);
        con.set_scale(SCALE);
        con.goto(LABEL_COL, row);
        con.puts(label);
        let (text, len) = &c.suffix[item as usize];
        if *len > 0 {
            con.putc(b' ');
            for &b in &text[..*len] {
                con.putc(b);
            }
        }
    }
}

fn redraw(c: &Checklist, item: Item) {
    light(c, item);
    c.panel.present_nowait();
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
    ring_start(&c.panel);
    c.panel.present_nowait();
    unsafe { *CHECKLIST.0.get() = Some(c) };
}

// ---------------------------------------------------------------------------
// The boot ring (owner's design, 2026-10-22): the moment the press lands the screen goes black with one solid dark-brick disc in the middle, no stroke; around it each boot step lights a slice, clockwise from 12 o'clock, styled on logo.svg's phase-diagram slices of iron.
// - Angle is time: the full circle is the 1 s press-to-ready budget, and each slice spans its step, from the previous step's end to its own (a minimum width so the sub-millisecond steps still show). What stays black at the end is headroom.
// - Bottom and top (inner and outer radius) are the kind of step, banded like phases: hardware bring-up low against the disc, the disk next, trust (vault, ira, entropy) higher, the link and the processes outermost.
// - Colour is order: dark brick, brick, amber, then a bright lime, along the warm side only (nothing between magenta and green).
// All integer: a Q14 quarter-wave sine table and cross products, each slice drawn only inside its own bounding box.

/// Quarter-wave sine, Q14, 257 entries over 0..=90 degrees.
const SIN_Q14: [i32; 257] = [0, 101, 201, 302, 402, 503, 603, 704, 804, 904, 1005, 1105, 1205, 1306, 1406, 1506, 1606, 1706, 1806, 1906, 2006, 2105, 2205, 2305, 2404, 2503, 2603, 2702, 2801, 2900, 2999, 3098, 3196, 3295, 3393, 3492, 3590, 3688, 3786, 3883, 3981, 4078, 4176, 4273, 4370, 4467, 4563, 4660, 4756, 4852, 4948, 5044, 5139, 5235, 5330, 5425, 5520, 5614, 5708, 5803, 5897, 5990, 6084, 6177, 6270, 6363, 6455, 6547, 6639, 6731, 6823, 6914, 7005, 7096, 7186, 7276, 7366, 7456, 7545, 7635, 7723, 7812, 7900, 7988, 8076, 8163, 8250, 8337, 8423, 8509, 8595, 8680, 8765, 8850, 8935, 9019, 9102, 9186, 9269, 9352, 9434, 9516, 9598, 9679, 9760, 9841, 9921, 10001, 10080, 10159, 10238, 10316, 10394, 10471, 10549, 10625, 10702, 10778, 10853, 10928, 11003, 11077, 11151, 11224, 11297, 11370, 11442, 11514, 11585, 11656, 11727, 11797, 11866, 11935, 12004, 12072, 12140, 12207, 12274, 12340, 12406, 12472, 12537, 12601, 12665, 12729, 12792, 12854, 12916, 12978, 13039, 13100, 13160, 13219, 13279, 13337, 13395, 13453, 13510, 13567, 13623, 13678, 13733, 13788, 13842, 13896, 13949, 14001, 14053, 14104, 14155, 14206, 14256, 14305, 14354, 14402, 14449, 14497, 14543, 14589, 14635, 14680, 14724, 14768, 14811, 14854, 14896, 14937, 14978, 15019, 15059, 15098, 15137, 15175, 15213, 15250, 15286, 15322, 15357, 15392, 15426, 15460, 15493, 15525, 15557, 15588, 15619, 15649, 15679, 15707, 15736, 15763, 15791, 15817, 15843, 15868, 15893, 15917, 15941, 15964, 15986, 16008, 16029, 16049, 16069, 16088, 16107, 16125, 16143, 16160, 16176, 16192, 16207, 16221, 16235, 16248, 16261, 16273, 16284, 16295, 16305, 16315, 16324, 16332, 16340, 16347, 16353, 16359, 16364, 16369, 16373, 16376, 16379, 16381, 16383, 16384, 16384];
/// Angles are in 1/1024 turns, 0 at 12 o'clock, clockwise.
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
/// The screen direction of angle `t`, Q14: x right, y down, 0 straight up.
fn dir(t: i32) -> (i32, i32) {
    (sin_t(t), -sin_t(t + 256))
}

const DISC: u32 = 0x003C_0200;
/// The disc's radius and the slices' bands, in panel pixels (1344 wide).
const R_DISC: i32 = 230;
const BUDGET_US: u64 = 1_000_000;
const MIN_TURN: i32 = 11;
const GAP_TURN: i32 = 2;
/// The colour ramp's stops, warm side only.
const RAMP: [u32; 4] = [0x003C_0200, 0x00BB_3700, 0x00E4_B200, 0x00B4_FF00];

fn band(item: Item) -> (i32, i32) {
    match item {
        Item::Killswitch | Item::CacheMap | Item::Panel => (250, 360),
        Item::Disk | Item::Slot => (300, 470),
        Item::VaultOpen | Item::VaultVerified | Item::Ira | Item::Wairua => (360, 560),
        _ => (420, 640),
    }
}

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
    // The gate left the screen black but for its own word and arrows: clear just those.
    crate::ready::erase(p);
    let (cx, cy) = ((p.width / 2) as i32, (p.rows / 2) as i32);
    for y in -R_DISC..=R_DISC {
        for x in -R_DISC..=R_DISC {
            if x * x + y * y <= R_DISC * R_DISC {
                put(p, cx + x, cy + y, DISC);
            }
        }
    }
}

/// One slice: the annulus band [r1, r2) between angles t0 and t1 (1/1024 turns), split into pieces of at most a quarter turn so each is a convex sector the cross-product test can take.
fn slice(p: &Panel, r1: i32, r2: i32, t0: i32, t1: i32, rgb: u32) {
    let (cx, cy) = ((p.width / 2) as i32, (p.rows / 2) as i32);
    let mut a = t0;
    while a < t1 {
        let b = (a + 256).min(t1);
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
        for q in [0, 256, 512, 768] {
            let qa = if q < a { q + TURN } else { q };
            if qa > a && qa < b {
                let d = dir(q);
                xs[n] = d.0 * r2 >> 14;
                ys[n] = d.1 * r2 >> 14;
                n += 1;
            }
        }
        let (x0, x1) = (*xs[..n].iter().min().unwrap() - 1, *xs[..n].iter().max().unwrap() + 1);
        let (y0, y1) = (*ys[..n].iter().min().unwrap() - 1, *ys[..n].iter().max().unwrap() + 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let rr = x * x + y * y;
                if rr < r1 * r1 || rr >= r2 * r2 {
                    continue;
                }
                // Inside the piece: on the clockwise side of its start ray and the anticlockwise side of its end ray.
                let c0 = d0.0 as i64 * y as i64 - d0.1 as i64 * x as i64;
                let c1 = x as i64 * d1.1 as i64 - y as i64 * d1.0 as i64;
                if c0 >= 0 && c1 >= 0 {
                    put(p, cx + x, cy + y, rgb);
                }
            }
        }
        a = b;
    }
}

/// Where the ring has got to, in 1/1024 turns, and how many slices are lit.
static mut RING_AT: i32 = 0;
static mut RING_N: u32 = 0;
static mut RING_LIT: u32 = 0;
/// How many steps a cold boot lights after the press, so the ramp ends on lime at the last one: cache map, entropy, disk, vault opened, vault verified, ira, slot, USB (measured 2026-10-22).
const RING_STEPS: u32 = 8;

/// Light `item`'s slice: from where the ring has got to, to this moment's place in the 1 s budget.
fn light(c: &Checklist, item: Item) {
    use core::sync::atomic::Ordering::Relaxed;
    let press = PRESS.load(Relaxed);
    let now = crate::platform::cntpct();
    let f = crate::platform::cntfrq();
    let us = if press == 0 { 0 } else { now.saturating_sub(press) * 1_000_000 / f };
    let target = ((us.min(BUDGET_US) * TURN as u64) / BUDGET_US) as i32;
    unsafe {
        // Each step lights once: a later `suffix` or interim-to-done redraw of the same item draws nothing new.
        let lit = &raw mut RING_LIT;
        if *lit & (1 << item as u8) != 0 {
            return;
        }
        *lit |= 1 << item as u8;
        let (at, n) = (&raw mut RING_AT, &raw mut RING_N);
        let t0 = *at;
        let t1 = target.max(t0 + MIN_TURN).min(TURN);
        if t1 > t0 {
            let (r1, r2) = band(item);
            slice(&c.panel, r1, r2, t0, t1 - GAP_TURN, ramp(*n, RING_STEPS));
            *at = t1;
            *n += 1;
        }
    }
}

/// Turn one item green and push a frame.
/// Boot timing: the counter when the cold kernel was entered (set right after the killswitch arms) and, per item, the counter when it was first marked. Kept apart from the drawn list so items marked before the panel is adopted still get a time. Read by `diag boot`.
pub static ENTRY: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static PRESS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
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
    s
}

pub fn mark(item: Item) {
    stamp(item);
    let Some(c) = (unsafe { (*CHECKLIST.0.get()).as_mut() }) else {
        return;
    };
    if c.done & (1 << item as u8) != 0 {
        return;
    }
    c.done |= 1 << item as u8;
    redraw(c, item);
}

/// Mark an item up through a stand-in (yellow) and redraw it.
pub fn mark_interim(item: Item) {
    stamp(item);
    let Some(c) = (unsafe { (*CHECKLIST.0.get()).as_mut() }) else {
        return;
    };
    c.done |= 1 << item as u8;
    c.interim |= 1 << item as u8;
    redraw(c, item);
}

/// Put `text` after an item's label (truncated to 16 bytes) and redraw it.
pub fn suffix(item: Item, text: &[u8]) {
    let Some(c) = (unsafe { (*CHECKLIST.0.get()).as_mut() }) else {
        return;
    };
    let n = text.len().min(SUFFIX_MAX);
    c.suffix[item as usize].0[..n].copy_from_slice(&text[..n]);
    c.suffix[item as usize].1 = n;
    redraw(c, item);
}

/// Whether the checklist owns the panel on this boot (then nothing else should draw over it).
pub fn active() -> bool {
    unsafe { (*CHECKLIST.0.get()).is_some() }
}
