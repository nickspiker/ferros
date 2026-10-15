//! The bring-up checklist on the panel: every step from power-on to ferros's first process, red until it happens on this boot, green once it has (BOOT.md, "Ready?" gate).
//!
//! Steps that do not exist yet stay red — the screen is the road map. Cold boot only: the handoff path has no live panel unless it came up with `--live`.

use crate::display::Panel;
use core::cell::UnsafeCell;

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum Item {
    Killswitch,
    CacheMap,
    Panel,
    Ready,
    Disk,
    Vault,
    Ira,
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
        (Item::Ready, "Ready? pressed"),
    ]),
    ("boot", &[
        (Item::Disk, "disk: UFS link"),
        (Item::Vault, "vault opened + verified"),
        (Item::Ira, "ira + wairua"),
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

const WHITE: u32 = 0x00FF_FFFF;
const RED: u32 = 0x00FF_3030;
const GREEN: u32 = 0x0030_FF60;
const SCALE: usize = 3;

struct Checklist {
    panel: Panel,
    done: u32,
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

fn draw_item(c: &Checklist, item: Item, label: &str) {
    let done = c.done & (1 << item as u8) != 0;
    unsafe {
        let mut con = c.panel.console(if done { GREEN } else { RED });
        con.set_scale(SCALE);
        con.goto(0, row_of(item));
        con.puts(if done { " [x] " } else { " [ ] " });
        con.puts(label);
    }
}

/// Adopt the panel and draw the whole list; `done` items start green. No-op (and every later `mark` too) if there is no panel to drive.
pub fn start(cntfrq: u64, done: &[Item]) {
    let mut rep = alloc::vec::Vec::new();
    let Some(panel) = crate::display::adopt(cntfrq, &mut rep) else {
        return;
    };
    let mut c = Checklist { panel, done: 1 << Item::Panel as u8 };
    for &d in done {
        c.done |= 1 << d as u8;
    }
    unsafe {
        let mut con = c.panel.console(WHITE);
        con.set_scale(SCALE);
        con.goto(0, 0);
        con.puts("ferros");
        let mut row = 2;
        for (name, items) in GROUPS.iter() {
            con.set_fg(WHITE);
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
pub fn mark(item: Item) {
    let Some(c) = (unsafe { (*CHECKLIST.0.get()).as_mut() }) else {
        return;
    };
    if c.done & (1 << item as u8) != 0 {
        return;
    }
    c.done |= 1 << item as u8;
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

/// Whether the checklist owns the panel on this boot (then nothing else should draw over it).
pub fn active() -> bool {
    unsafe { (*CHECKLIST.0.get()).is_some() }
}
