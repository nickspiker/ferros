//! Shared regions (2026-11-24, docs/KERNEL.md "Shared regions"; the owner: waves and beams at the lowest latency, "q removal"): a server's pool of memory, runs of which it gives to the processes it answers, so a stream's samples or a socket's datagrams pass between the two thru memory both map, with no call per item.
//! - A pool is the server's own: a GiB of addresses in the shared window (cachemap `SHARE_VA`), frames of the pool mapped from its start as it grows, in place (`pool_grow`), reached thru a handle slot the server names (masked code, SANDBOX.md "Handles") or by its address (native).
//! - A view is a run of a pool, a power of two from a page (4 KiB) at an offset aligned to it, given to the process waiting on one of the server's reply tokens (the caller of the OPEN it answers, as the camera's view is given): its handle written at the holder's first free pair past the ones its build wrote, its address the same as the server's.
//! - The doorbell is SYS_WAIT and SYS_WAKE on a word of a run either side holds: waiters are kept under the word's address, the same in every process, so the server's wake finds every view's waiter in the one scan of the threads a wake costs, without counting them.
//! - The server's bell: a word of its own arc it names with its pool. A holder's wake on its run also counts that word up by one and wakes its waiters, so a server with many holders waits on one word for all of them (a futex word that moved, so a wake between its look at the rings and its wait is never lost) and looks at its rings when it rings.
//! - A view ends at the server's TAKE, when its holder ends or when the server ends; a pool ends with its server. Every thread waiting on a word of what ended is woken, and finds it gone at its next wait (ERR).

use super::*;

/// The most bytes a run or a pool is: a pool's GiB of addresses (`cachemap::SHARE_POOL_MOST`), the window's slot. A run's floor: a page.
const RUN_LEAST_LOG: u64 = 12;
const POOL_MOST_LOG: u64 = 30;
const _: () = assert!(1 << POOL_MOST_LOG == crate::cachemap::SHARE_POOL_MOST);

/// A server's pool: its owner's epoch (0: the slot is free), the frames mapped from its start, the handle slot the owner named for it.
struct Pool {
    owner: u64,
    frames: Vec<u32>,
    slot: usize,
    /// The server's bell: a word of its arc (the kernel's address of it), counted up and rung at each holder's wake; 0 for none.
    bell: usize,
}

/// A run of a pool given to a holder: its id (the server names it to TAKE and ALIVE), the pool, its offset and size (log2), the holder's epoch and the handle slot written in its grants page (None: none was free, so a masked holder cannot reach it; a native one still can).
#[derive(Clone, Copy)]
struct View {
    id: u64,
    pool: usize,
    off: usize,
    log: u32,
    holder: u64,
    slot: Option<usize>,
}

/// The pools by window slot, and the views given: as many as are made, no table of fixed size (the window's GiBs bound the pools, `cachemap::SHARE_VA`). Under the kernel lock.
static mut POOLS: Vec<Pool> = Vec::new();
static mut VIEWS: Vec<View> = Vec::new();
static mut NEXT_ID: u64 = 1;

fn pools() -> &'static mut Vec<Pool> {
    unsafe { &mut *(&raw mut POOLS) }
}
fn views() -> &'static mut Vec<View> {
    unsafe { &mut *(&raw mut VIEWS) }
}
fn pool_va(j: usize) -> usize {
    crate::cachemap::SHARE_VA.0 + j * crate::cachemap::SHARE_POOL_MOST
}
fn say(p: &Process, what: &str) {
    k_console(&alloc::format!("share: {}: {what}\n", core::str::from_utf8(&p.name).unwrap_or("?")));
}

/// A handle pair into a process's grants page at `slot`, the page opened for the write and closed after (`write_pref_all`'s way).
fn write_pair(q: &Process, slot: usize, base: u64, mask: u64) {
    crate::cachemap::grants_page(q.arc.base, true);
    q.write_grants((HANDLES + 16 * slot) as u64, &base.to_le_bytes());
    q.write_grants((HANDLES + 16 * slot + 8) as u64, &mask.to_le_bytes());
    crate::cachemap::grants_page(q.arc.base, false);
}

/// The first pair of `q`'s handle table that is free past the slots its build wrote (as a mailbox's view finds one, `mbox_open`).
fn free_pair(q: &Process) -> Option<usize> {
    // SAFETY: the grants page is the arc's first page, the kernel's to read at its own address; the count and the pairs are words the kernel wrote.
    let built = unsafe { core::ptr::read_volatile((q.arc.base + q.fast.tokens) as *const u32) } as usize;
    (built..(TOKENS - HANDLES) / 16).find(|&s| unsafe { core::ptr::read_volatile((q.arc.base + HANDLES + 16 * s) as *const u64) } == 0)
}

/// Op 0 POOL: the caller's pool, made when it has none, grown in place to 2^`log` bytes (a frame at least; asking for less than it holds leaves it), its handle at slot `slot` of the caller's table, its bell the word at arc offset `bell` (0: none; a later POOL may name it). → the pool's address. ERR, said on the console: past a GiB, the slot taken by something else or past the table, the bell not a word of the arc, the frame pool short.
fn pool(p: &Process, log: u64, slot: u64, bell: u64) -> u64 {
    let bell = match bell {
        0 => None,
        b => match p.word(b) {
            Some(a) => Some(a),
            None => {
                say(p, "the bell named is not a word of its arc");
                return ERR;
            }
        },
    };
    // WHY: `log` and `slot` are the caller's words.
    if log > POOL_MOST_LOG || slot >= ((TOKENS - HANDLES) / 16) as u64 {
        say(p, "a pool is at most a GiB, its handle inside the grants page's table");
        return ERR;
    }
    let slot = slot as usize;
    let j = match pools().iter().position(|x| x.owner == p.epoch) {
        Some(j) => j,
        None => {
            // A pair the build wrote, or one in use, is not the pool's to take.
            if free_pair(p).is_none_or(|f| slot < f) || unsafe { core::ptr::read_volatile((p.arc.base + HANDLES + 16 * slot) as *const u64) } != 0 {
                say(p, "the handle slot named is taken");
                return ERR;
            }
            let j = match pools().iter().position(|x| x.owner == 0 && x.frames.is_empty()) {
                Some(j) => j,
                None if pool_va(pools().len()) < crate::cachemap::SHARE_VA.1 => {
                    pools().push(Pool { owner: 0, frames: Vec::new(), slot: 0, bell: 0 });
                    pools().len() - 1
                }
                None => {
                    say(p, "every GiB of the shared window holds a pool");
                    return ERR;
                }
            };
            pools()[j].owner = p.epoch;
            pools()[j].slot = slot;
            pools()[j].bell = 0;
            j
        }
    };
    if let Some(b) = bell {
        pools()[j].bell = b;
    }
    let bytes = (1usize << log).max(crate::frames::FRAME);
    let want = bytes / crate::frames::FRAME;
    let had = pools()[j].frames.len();
    for n in had..want {
        let Some(f) = crate::frames::take_top() else {
            say(p, "the frame pool has no frame for the pool's growth");
            return if n == had && had == 0 { pool_drop(j) } else { ERR };
        };
        if !crate::cachemap::share_map(pool_va(j) + n * crate::frames::FRAME, crate::frames::pa(f)) {
            crate::frames::give(f);
            say(p, "the frame pool has no frame for the window's table");
            return if had == 0 && n == 0 { pool_drop(j) } else { ERR };
        }
        pools()[j].frames.push(f as u32);
    }
    let n = pools()[j].frames.len();
    if n > had {
        // Blocks a pool given back mapped to the blank frame before: every core told to forget them.
        crate::cachemap::surface_unmapped(pool_va(j) + had * crate::frames::FRAME, (n - had) * crate::frames::FRAME);
    }
    // The handle covers what the pool holds, a power of two from its start (the frames are 2 MiB, the size a power of two of them).
    let size = n * crate::frames::FRAME;
    let mask = if size.is_power_of_two() { size as u64 - 1 } else { (size.next_power_of_two() >> 1) as u64 - 1 };
    write_pair(p, pools()[j].slot, pool_va(j) as u64, mask);
    pool_va(j) as u64
}

/// A pool that never held a frame let go again (its first growth failed). ERR, for the caller to answer.
fn pool_drop(j: usize) -> u64 {
    pools()[j].owner = 0;
    ERR
}

/// Op 1 GIVE: the run of 2^`log` bytes at `off` in the caller's pool given read-write to the process waiting on the reply token at arc offset `reply_off`. → the view's id (never 0). ERR, said on the console: no pool, the run outside what it holds or not aligned to its size, the token answering no other process.
fn give(p: &Process, reply_off: u64, off: u64, log: u64) -> u64 {
    let Some(j) = pools().iter().position(|x| x.owner == p.epoch) else {
        say(p, "GIVE without a pool");
        return ERR;
    };
    let held = (pools()[j].frames.len() * crate::frames::FRAME) as u64;
    // WHY: `off` and `log` are the caller's words; a run past what the pool maps would hand out unmapped addresses, and one not aligned to its size has no handle.
    if !(RUN_LEAST_LOG..=POOL_MOST_LOG).contains(&log) || off % (1 << log) != 0 || off >= held || held - off < 1 << log {
        say(p, "a run is a power of two from a page, aligned to its size, inside what the pool holds");
        return ERR;
    }
    let token = p.read(reply_off, 32);
    let holder = match p.lookup(&token).map(|i| p.cspace[i].object) {
        Some(Object::Reply(t)) if t != KERNEL_CALLER => proc_of(t).filter(|q| q.epoch != p.epoch && !q.dying),
        _ => None,
    };
    let Some(q) = holder else {
        say(p, "the reply token answers no other process");
        return ERR;
    };
    let base = pool_va(j) as u64 + off;
    let slot = free_pair(q);
    if let Some(s) = slot {
        write_pair(q, s, base, (1 << log) - 1);
    }
    let id = unsafe {
        let id = NEXT_ID;
        NEXT_ID += 1;
        id
    };
    views().push(View { id, pool: j, off: off as usize, log: log as u32, holder: q.epoch, slot });
    id
}

/// A view ended: the holder's pair cleared when it still lives, every thread waiting on a word of the run woken (it finds the run gone at its next wait). Under the lock.
fn end_view(v: &View, holder_lives: bool) {
    if holder_lives {
        if let (Some(s), Some(q)) = (v.slot, k().procs.iter().flatten().find(|q| q.epoch == v.holder)) {
            write_pair(q, s, 0, 0);
        }
    }
    let lo = pool_va(v.pool) + v.off;
    wake_range(lo, lo + (1 << v.log));
    // A holder gone: its server's bell rung, so it looks (ALIVE) and lets the run go without waiting for anything else to wake it.
    if !holder_lives {
        if let Some(x) = pools().get(v.pool).filter(|x| x.owner != 0 && x.bell != 0) {
            // SAFETY: as `wake`: the bell is a word of the live server's arc.
            unsafe { (*(x.bell as *const core::sync::atomic::AtomicU64)).fetch_add(1, core::sync::atomic::Ordering::AcqRel) };
            mbox_wake(x.bell, u64::MAX);
        }
    }
}

/// Op 2 TAKE: the caller's view `id` taken back. → 0, or ERR when it is not one of the caller's.
fn take(p: &Process, id: u64) -> u64 {
    let Some(i) = views().iter().position(|v| v.id == id && pools()[v.pool].owner == p.epoch) else { return ERR };
    let v = views().swap_remove(i);
    end_view(&v, true);
    0
}

/// Op 3 VIEW, the holder's: view `id`'s address, and at arc offset `out` its bytes and its handle slot (u64 LE each; u64::MAX: no slot was free). ERR: not a view the caller holds.
fn view(p: &Process, id: u64, out: u64) -> u64 {
    let Some(v) = views().iter().find(|v| v.id == id && v.holder == p.epoch).copied() else { return ERR };
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&(1u64 << v.log).to_le_bytes());
    b[8..].copy_from_slice(&v.slot.map_or(u64::MAX, |s| s as u64).to_le_bytes());
    p.write(out, &b);
    (pool_va(v.pool) + v.off) as u64
}

/// Op 4 ALIVE, the server's: 1 while view `id` is held, 0 once it ended (its holder gone, or taken back). ERR: no pool.
fn alive(p: &Process, id: u64) -> u64 {
    if !pools().iter().any(|x| x.owner == p.epoch) {
        return ERR;
    }
    views().iter().any(|v| v.id == id && pools()[v.pool].owner == p.epoch) as u64
}

/// SYS_SHARE: x0 the op, the rest as each says.
pub(super) fn syscall(f: &mut Frame, p: &Process) {
    f.x[0] = match f.x[0] {
        0 => pool(p, f.x[1], f.x[2], f.x[3]),
        1 => give(p, f.x[1], f.x[2], f.x[3]),
        2 => take(p, f.x[1]),
        3 => view(p, f.x[1], f.x[2]),
        4 => alive(p, f.x[1]),
        _ => ERR,
    };
}

/// The word a SYS_WAIT or SYS_WAKE names outside the caller's arc, when it lies in its pool or in a view it holds: its address, under which waiters of every process are kept. None otherwise (the wait traps and the wake wakes nothing, as before).
pub(super) fn word(p: &Process, off: u64) -> Option<usize> {
    let a = ((p.arc.base as u64).wrapping_add(off) & !DMA_ADDR) as usize;
    let (lo, hi) = crate::cachemap::SHARE_VA;
    // WHY: the address is the process's own; outside the window, or not a whole aligned word, it names no shared word.
    if a < lo || a >= hi || a % 8 != 0 {
        return None;
    }
    let j = (a - lo) / crate::cachemap::SHARE_POOL_MOST;
    let at = a - pool_va(j);
    let x = pools().get(j)?;
    // WHY: only the frames a pool holds are mapped; past them the kernel's own read of the word would fault.
    if at >= x.frames.len() * crate::frames::FRAME {
        return None;
    }
    let mine = x.owner == p.epoch || views().iter().any(|v| v.pool == j && v.holder == p.epoch && at >= v.off && at < v.off + (1 << v.log));
    mine.then_some(a)
}

/// The process with `epoch` is gone (its arc is freed next): every view it held ended, and when it served a pool, every view of it ended (each holder's pair cleared) and its frames back to the pool, its blocks mapping the blank frame first and every TLB told. Under the lock, beside `mailboxes_gone`.
pub(super) fn gone(epoch: u64) {
    let mut i = 0;
    while i < views().len() {
        let v = views()[i];
        let server_went = pools()[v.pool].owner == epoch;
        if v.holder == epoch || server_went {
            views().swap_remove(i);
            end_view(&v, v.holder != epoch);
        } else {
            i += 1;
        }
    }
    let Some(j) = pools().iter().position(|x| x.owner == epoch) else { return };
    let va = pool_va(j);
    let n = pools()[j].frames.len();
    // The blank frame is the surfaces' (`blank_frame`), taken once for the boot; with none to be had the blocks stay mapped to the frames, which are then kept from the pool rather than handed on under a stale map.
    let Some(blank) = blank_frame() else {
        k_console("share: a pool's server ended and the frame pool had no blank frame: its frames kept\n");
        pools()[j].owner = u64::MAX;
        return;
    };
    for m in 0..n {
        crate::cachemap::share_map(va + m * crate::frames::FRAME, crate::frames::pa(blank));
    }
    crate::cachemap::surface_unmapped(va, n * crate::frames::FRAME);
    for &f in &pools()[j].frames {
        crate::frames::give(f as usize);
    }
    wake_range(va, va + n * crate::frames::FRAME);
    pools()[j].frames.clear();
    pools()[j].owner = 0;
}

/// `diag arcs`'s pools: each held one, its server, its size and its views.
pub(super) fn report(s: &mut alloc::string::String) {
    for (j, x) in pools().iter().enumerate().filter(|(_, x)| x.owner != 0) {
        let name = k().procs.iter().flatten().find(|q| q.epoch == x.owner).map(|q| alloc::string::String::from_utf8_lossy(&q.name).into_owned()).unwrap_or_default();
        let nv = views().iter().filter(|v| v.pool == j).count();
        let _ = core::fmt::Write::write_fmt(s, format_args!("share pool A#{j} of {name} at G#{:X}: A#{} MiB, A#{nv} views\n", pool_va(j), x.frames.len() * 2));
    }
}

/// Whether a word a SYS_WAIT names outside the caller's arc lies in the shared window at all: one the caller does not hold (its view taken back, its server ended) answers ERR there instead of the trap a buffer nowhere earns.
pub(super) fn in_window(p: &Process, off: u64) -> bool {
    let a = ((p.arc.base as u64).wrapping_add(off) & !DMA_ADDR) as usize;
    a >= crate::cachemap::SHARE_VA.0 && a < crate::cachemap::SHARE_VA.1
}

/// A wake on a shared word by `p`: every process's waiters on it (`mbox_wake`), and when `p` holds the run as a view (not the pool's server), its server's bell counted up and rung (`Pool::bell`). How many were woken on the word itself. Under the lock.
pub(super) fn wake(p: &Process, a: usize, count: u64) -> u64 {
    let n = mbox_wake(a, count);
    let j = (a - crate::cachemap::SHARE_VA.0) / crate::cachemap::SHARE_POOL_MOST;
    if let Some(x) = pools().get(j).filter(|x| x.owner != p.epoch && x.owner != 0 && x.bell != 0) {
        // SAFETY: the bell is a word of the server's arc, resolved when it was named (`Process::word`), and the server lives (its pool's owner), so its arc is mapped; an atomic add, as the server's own threads touch it.
        unsafe { (*(x.bell as *const core::sync::atomic::AtomicU64)).fetch_add(1, core::sync::atomic::Ordering::AcqRel) };
        mbox_wake(x.bell, u64::MAX);
    }
    n
}
