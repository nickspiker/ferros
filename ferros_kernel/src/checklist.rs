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
    for (_, items) in GROUPS.iter() {
        for &(it, label) in items.iter() {
            if it as u8 == item as u8 {
                draw_item(c, it, label);
            }
        }
    }
    let mut rep = alloc::vec::Vec::new();
    c.panel.present(&mut rep);
}

/// Adopt the panel and draw the whole list; `done` items start green. No-op (and every later `mark` too) if there is no panel to drive.
pub fn start(cntfrq: u64, done: &[Item]) {
    let mut rep = alloc::vec::Vec::new();
    let Some(panel) = crate::display::adopt(cntfrq, &mut rep) else {
        return;
    };
    let mut c = Checklist { panel, done: 1 << Item::Panel as u8, interim: 0, suffix: [([0u8; SUFFIX_MAX], 0); 32] };
    for &d in done {
        c.done |= 1 << d as u8;
    }
    unsafe {
        let mut con = c.panel.console(TITLE);
        con.set_scale(SCALE);
        con.goto(0, 0);
        con.puts("ferros");
        let mut row = 2;
        for (name, items) in GROUPS.iter() {
            con.set_fg(HEADER);
            con.goto(0, row);
            con.puts(name);
            row += items.len() + 2;
        }
    }
    for (_, items) in GROUPS.iter() {
        for &(it, label) in items.iter() {
            draw_item(&c, it, label);
        }
    }
    c.panel.present(&mut rep);
    unsafe { *CHECKLIST.0.get() = Some(c) };
}

/// Turn one item green and push a frame.
/// Boot timing: the counter when the cold kernel was entered (set right after the killswitch arms) and, per item, the counter when it was first marked. Kept apart from the drawn list so items marked before the panel is adopted still get a time. Read by `diag boot`.
pub static ENTRY: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static PRESS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub static AT: [core::sync::atomic::AtomicU64; 18] = [const { core::sync::atomic::AtomicU64::new(0) }; 18];

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
    let mut v: alloc::vec::Vec<(u64, &str)> = alloc::vec::Vec::new();
    for (_, items) in GROUPS.iter() {
        for &(item, label) in items.iter() {
            let t = AT[item as usize].load(Relaxed);
            if t != 0 {
                v.push((t, label));
            }
        }
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
