//! The kernel's PT commands, fed by the USB driver process (USB.md step 3).
//!
//! The process receives Photon Transport from the bridge and hands each assembled command here (`SYS_PT` op 0). The service loop runs it through the same dispatch the kernel's own USB feeds (service.rs `run`) and leaves the raw reply here for the process to take (op 1) and send. Reload, reboot and `diag android` answer first and act only when the process asks again with op 3, after its reply has gone out: "reply, then act".
//!
//! The controller itself changes hands with the `usb` power switch: on, the kernel stops touching DWC3 and readies the link for the process (the Type-C switch, the eUSB2 PHY, the DMA path); off, or the process gone, the kernel brings its own USB back.
//!
//! Every function here runs with the kernel lock held: from a syscall, or from the service loop through `with_lock`.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

/// The kernel drives the USB controller (true), or the USB process does (false, between `power usb on` and `off`).
pub static KERNEL_USB: AtomicBool = AtomicBool::new(true);
/// Set when the controller comes back to the kernel: the service loop re-initialises it before touching it again.
pub static TAKE_BACK: AtomicBool = AtomicBool::new(false);

/// A submitted command waits in the inbox: read without the lock by the service loop, which then neither sleeps nor hands out a time slice before running it.
pub static INBOX_FULL: AtomicBool = AtomicBool::new(false);

pub fn kernel_usb() -> bool {
    KERNEL_USB.load(Ordering::Relaxed)
}

/// Commands up to 1 MiB are copied to the heap; larger ones, up to the receive region's 6 MiB, into that region, which is free while the process owns the controller (the kernel's own PT receive is what else uses it).
const SMALL: usize = 1 << 20;

/// The receive region (service.rs PT_BIG).
pub const BIG: (usize, usize) = (0x81A0_0000, 0x8200_0000);

pub enum Payload {
    Heap(Vec<u8>),
    Big(usize),
}

impl Payload {
    pub fn bytes(&self) -> &[u8] {
        match self {
            Payload::Heap(v) => v,
            // SAFETY: BIG is kernel memory outside the heap, mapped, and nothing else writes it while a submitted command holds it (`submit` refuses a large command while the kernel owns the controller).
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
    /// The reply, and whether the command waits for its act.
    outbox: Option<(Vec<u8>, bool)>,
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
    } else if len <= BIG.1 - BIG.0 && !kernel_usb() {
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

/// The service loop's side: the command's reply.
pub fn reply(bytes: Vec<u8>, wants_act: bool) {
    link().outbox = Some((bytes, wants_act));
}

/// Op 1: the reply from `offset`, copied by `put`. None while the command is still running; Some((total length, wants act)) when done. The reply is dropped once a take reaches its end.
pub fn take(offset: usize, cap: usize, put: impl FnOnce(&[u8])) -> Option<(usize, bool)> {
    let l = link();
    let (bytes, act) = l.outbox.as_ref()?;
    let (total, act) = (bytes.len(), *act);
    // The offset and length are the process's: held to the reply, or a bad pair would panic the kernel slicing it.
    let from = offset.min(total);
    let to = (offset + cap).min(total);
    put(&bytes[from..to]);
    if to == total {
        l.outbox = None;
        l.busy = false;
    }
    Some((total, act))
}

/// Whether a command is in (op 1 tells "still running" from "nothing submitted" by it).
pub fn busy() -> bool {
    link().busy
}

/// The `usb` power switch (process.rs POWERS). On: the kernel lets go of the controller, then readies the link for the process: the Type-C port's USB switch closed, the eUSB2 PHY brought up again (which drops the host's view of the kernel's device), the DMA path to physical addresses open. The process then resets and runs the controller itself. Off: the kernel takes it back on the service loop's next pass.
pub fn power(on: bool) -> bool {
    if on {
        KERNEL_USB.store(false, Ordering::Relaxed);
        crate::service::tcpc_usb_switch(true);
        crate::pixel8_eusb_phy_init();
        let _ = crate::platform::open_usb_dma_path();
        crate::flashlog::line("usb: the controller goes to the USB process");
    } else if !kernel_usb() {
        TAKE_BACK.store(true, Ordering::Relaxed);
        crate::flashlog::line("usb: the controller comes back to the kernel");
    }
    true
}

/// The controller back with the kernel: a command the process left half-done (submitted, its reply never taken) is dropped, so the next USB process starts clean.
pub fn reset() {
    let l = link();
    l.inbox = None;
    l.outbox = None;
    l.busy = false;
    INBOX_FULL.store(false, Ordering::Relaxed);
}
