//! The kernel's PT commands, fed by the USB driver process (USB.md). The kernel has no USB of its own since 2026-11-01: the process is the bridge's only way in, and init's keeper starts it again should it end.
//!
//! The process receives Photon Transport from the bridge and hands each assembled command here (`SYS_PT` op 0). The service loop runs it through its dispatch (service.rs `run`) and leaves the raw reply here for the process to take (op 1) and send. Reload, reboot and `diag android` answer first and act only when the process asks again with op 3, after its reply has gone out: "reply, then act".
//!
//! The `usb` power switch readies the link for the process: the Type-C switch, the eUSB2 PHY, the DMA path, the parts that are not the controller's.
//!
//! Every function here runs with the kernel lock held: from a syscall, or from the service loop through `with_lock`.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

/// A submitted command waits in the inbox: read without the lock by the service loop, which then neither sleeps nor hands out a time slice before running it.
pub static INBOX_FULL: AtomicBool = AtomicBool::new(false);

/// Commands up to 1 MiB are copied to the heap; larger ones, up to 6 MiB, into a fixed region outside it, so a large store put cannot run the heap out by fragmentation.
const SMALL: usize = 1 << 20;

/// That region: 6 MiB between the DMA pool and the reload stub, inside the kernel's cacheable window (cachemap WINDOW).
pub const BIG: (usize, usize) = (0x81A0_0000, 0x8200_0000);

pub enum Payload {
    Heap(Vec<u8>),
    Big(usize),
}

impl Payload {
    pub fn bytes(&self) -> &[u8] {
        match self {
            Payload::Heap(v) => v,
            // SAFETY: BIG is kernel memory outside the heap, mapped, and nothing else writes it while a submitted command holds it (one command at a time).
            Payload::Big(n) => unsafe { core::slice::from_raw_parts(BIG.0 as *const u8, *n) },
        }
    }
}

/// A submitted command and whether this is the act (op 3) of one that answered first.
pub struct Submitted {
    pub payload: Payload,
    pub act: bool,
}

struct Link {
    inbox: Option<Submitted>,
    /// The reply, whether one was given at all, and whether the command waits for its act.
    outbox: Option<(Vec<u8>, bool, bool)>,
    /// A command is in: submitted, and its reply not yet taken in full.
    busy: bool,
}

static mut LINK: Link = Link { inbox: None, outbox: None, busy: false };

fn link() -> &'static mut Link {
    // SAFETY: only under the kernel lock (module doc).
    unsafe { &mut *&raw mut LINK }
}

/// Op 0 (act false) or 3 (true): take a command of `len` bytes, copied by `fill` into the slice given. False when one is already in, or it is too large for where it would go.
pub fn submit(len: usize, act: bool, fill: impl FnOnce(&mut [u8])) -> bool {
    let l = link();
    if l.busy {
        return false;
    }
    let payload = if len <= SMALL {
        let mut v = alloc::vec![0u8; len];
        fill(&mut v);
        Payload::Heap(v)
    } else if len <= BIG.1 - BIG.0 {
        // SAFETY: as Payload::bytes.
        fill(unsafe { core::slice::from_raw_parts_mut(BIG.0 as *mut u8, len) });
        Payload::Big(len)
    } else {
        return false;
    };
    l.inbox = Some(Submitted { payload, act });
    INBOX_FULL.store(true, Ordering::Relaxed);
    l.busy = true;
    true
}

/// The service loop's side: the command waiting, if any.
pub fn take_inbox() -> Option<Submitted> {
    INBOX_FULL.store(false, Ordering::Relaxed);
    link().inbox.take()
}

/// The service loop's side: the command's reply (`replied` false: none, as after a reload or a reboot, which only act).
pub fn reply(bytes: Vec<u8>, replied: bool, wants_act: bool) {
    link().outbox = Some((bytes, replied, wants_act));
}

/// Op 1: the reply from `offset`, copied by `put`. None while the command is still running; Some((total length, replied, wants act)) when done. The reply is dropped once a take reaches its end.
pub fn take(offset: usize, cap: usize, put: impl FnOnce(&[u8])) -> Option<(usize, bool, bool)> {
    let l = link();
    let (bytes, replied, act) = l.outbox.as_ref()?;
    let (total, replied, act) = (bytes.len(), *replied, *act);
    // The offset and length are the process's: held to the reply, or a bad pair would panic the kernel slicing it.
    let from = offset.min(total);
    let to = (offset + cap).min(total);
    put(&bytes[from..to]);
    if to == total {
        l.outbox = None;
        l.busy = false;
    }
    Some((total, replied, act))
}

/// Whether a command is in (op 1 tells "still running" from "nothing submitted" by it).
pub fn busy() -> bool {
    link().busy
}

/// The `usb` power switch (process.rs POWERS). On, as a USB process starts: the link readied — the Type-C port's USB switch closed, the eUSB2 PHY brought up again (the host sees a fresh attach), the DMA path to physical addresses open — and a command an earlier process left half-done dropped (`reset`), so this one starts clean. The process then resets and runs the controller itself. Off: nothing to give back; the process is going.
pub fn power(on: bool) -> bool {
    if on {
        // akita bring-up: a square on the glass after each step (checklist::dot), the last one lit is where it stopped.
        crate::checklist::dot(1);
        crate::board::probe(81);
        reset();
        crate::checklist::dot(2);
        crate::board::probe(82);
        crate::service::tcpc_usb_switch(true);
        crate::checklist::dot(3);
        crate::board::probe(83);
        crate::pixel8_eusb_phy_init();
        crate::checklist::dot(4);
        crate::board::probe(84);
        let _ = crate::platform::open_usb_dma_path();
        crate::checklist::dot(5);
        crate::board::probe(85);
        crate::flashlog::line("usb: the link readied for the USB process");
        crate::board::probe_log(8);
    }
    true
}

/// A command an earlier USB process left half-done (submitted, its reply never taken) dropped.
fn reset() {
    let l = link();
    l.inbox = None;
    l.outbox = None;
    l.busy = false;
    INBOX_FULL.store(false, Ordering::Relaxed);
}

/// When the USB process last said a transfer is moving (`SYS_PT` op 2), on the counter: the clock policy counts it as activity, (service.rs `clock_policy`). Without it a transfer through the process ran at the rest clocks, the workers' cluster at 402 MHz (2026-10-31).
pub static MOVING_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
