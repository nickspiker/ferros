//! The cellular modem's driver as a process (Phase 2 of the owner's plan, 2026-10-29: the modem out of the kernel into its own driver): it brings the Shannon S5300 up over PCIe channel 0, registers, sets up the internet data call, and serves the call's IP packets on the `rmnet` endpoint to the IP process (payloads/ip). modem.rs is the kernel's bring-up and service (ferros_kernel/src/modem.rs, where it was proven) on this process's grants: hw.rs routes its registers and shared memory, partfs.rs its file reads.
//!
//! `rmnet` requests (first byte), as the kernel answered them in Phase 1:
//! - 1 SEND: an IP packet → [0] sent, [1] the UL queue is full (retry), [2] no data call, [3] bad request.
//! - 2 RECV: → the oldest DL packet of the call, or nothing.
//! - 3 INFO: → flags (bit 0 up, bit 1 IPv4, bit 2 IPv6), MTU (u16 BE), IPv4 (4), IPv6 (16), the first DNS server's IPv4 (4) and IPv6 (16): 43 bytes.
//! - 4 STATE: → the service's state as text (registration, the call without its addresses, packet counts).
//! - 5 DOWN: the CP powered off; the driver exits (its report on the console).
//! - 6 RESET: the data call deactivated; the service sees it gone and sets it up again (a test of the lost-call path).
//! - 7 RESTART: the CP powered down and brought up again (as after a CP crash, which the driver recovers from by itself).
//! - 8 TRACE [n]: the next n packets each way (16 if not given) on the console by shape: the far end, ports, checksums, never the line's own address.
//!
//! Input: empty or `up` brings the modem up and serves until killed; `off` powers the CP down and exits; `sN` runs the staged diagnostic, stages 0..=N, N one or two hex digits (modem.rs `stages`), and exits. The stages (from the kernel, where they were built): 0 survey, 1 power-on, 2 PHY, 3 link, 4 endpoint, 5 ROM boot, 6 link cycle, 7 the boot-channel download, 8 START and a ten-second watch, 9 START and the bare IPC handshake (PIF_INIT_DONE -> PHONE_START -> INIT_END), a the handshake with the pktproc DL rings and the legacy link, then 90 s watching what the online CP sends, b the same with one RIL probe (SIT_GET_RADIO_POWER on channel 245), c also the radio on (SIT_SET_RADIO_POWER, not for emergency), d also the data registration polled every 5 s (SIT_GET_PS_REG_STATE) and the SIM status read once, e also the preferred data modem and allow-data on stack 1, f also the initial-attach APN, 10 instead of the attach APN the carrier-config manifest handshake, the UICC subscription, the data profiles, the allowed networks (the data stack registers: 2026-10-29; each step on the CP's answer to the one before since 2026-11-12, modem.rs `ril_step`) and the stack-status and carrier-restriction queries, 11 also the AP capabilities G#3 with the UL packet rings and the gipc service on channel 132, 12 also the internet data call once registered (VZWINTERNET), then one IPv6 router solicitation and one ICMPv6 echo request to a public resolver up its rmnet channel, the queues' pointers every 4 s and every DL packet by IP version and protocol only; from b on, the modem service on channel 130 (properties, UE-capability files from the modem image) and the remote-file service on channel 41 (writes pulled and dropped) are answered (the whole radio is open during experimentation: owner, 2026-10-29).
//!
//! Spawn (tools/husky-modem.sh): ferros-bridge spawn modem.bin --name modem --serve rmnet --hw pcie0-elbi --hw pcie0-soc --hw pcie0-pcs --hw pcie0-udbg --hw pcie0-phy --hw pcie0-dbi --hw pcie0-cfg --hw pcie0-mem --hw cmu-hsi1 --hw hsi1-sysreg --hw gpio-hsi1 --hw sysmmu-hsi1 --hw s2mpu-hsi1 --hw gpio-alive --hw pmu --dma modem-pktproc --dma modem-ipc --dma modem-msi --part modem_a --part efs --part persist --part modem_userdata --part vendor_a --serve cell
//! Grant slots: 0 console, 1 serve rmnet, 2-16 the hardware blocks (hw.rs `BLOCKS`), 17-19 the shared-memory windows, 20-24 the partitions (partfs.rs `PARTS`), and 25 serve cell when spawned with `--serve cell` last (tools/husky-modem.sh; init has no slot for it yet).
//!
//! `cell` (2026-11-11, docs/LOCATION.md "Cellular"): the data stack's serving cell for the place service, from its registration reply (SIT_GET_PS_REG_STATE, decoded by the vendor's own reader's offsets) and its network (SIT_GET_OPERATOR, asked as the stack registers); the registration asked again at each IND_NTW_STATE_CHANGED, besides the backstop poll. Location data: served only here, never logged. Requests:
//! - 0 GET → the 62-byte reply (modem.rs `CELL_LEN`): valid, change count, age, state, technology, MCC, MNC and its digits, LAC, cell id, TAC, ECI, PCI, EARFCN, timing advance, NCI, NR TAC, NR PCI, NR-ARFCN.
//! - 1 WATCH `count u32` (the change count the caller last saw) → the same reply, at once when the count differs, else held until the cell, the network or the service changes: the doorbell, no polling.

#![no_std]
#![no_main]

extern crate alloc;

mod hw;
mod modem;
mod partfs;

use core::arch::global_asm;

pub const S_RMNET: u64 = 1;
pub const S_HW: u64 = 2;
pub const S_DMA: u64 = 17;
pub const S_PART: u64 = 20;
/// Serve `cell` (docs/LOCATION.md): present only when spawned with it.
pub const S_CELL: u64 = 25;

global_asm!(
    r#"
    .section .text.entry, "ax"
    .globl _entry
    .type _entry, %function
_entry:
    b {main}
    .4byte 0
    .ascii "ferrosRT"
    .quad __ferros_tail
    .quad 0
    .quad 0
    .quad 0
    .quad 0
    .size _entry, . - _entry
"#,
    main = sym modem_main
);

/// The heap: the UE-capability files (half a megabyte, and their replies), the carrier configs, the DL queue and each pass's frames, freed as they go (ferros_heap, the kernel's own allocator).
const HEAP_SIZE: usize = 16 << 20;
#[repr(C, align(64))]
struct Heap([u8; HEAP_SIZE]);
static mut HEAP: Heap = Heap([0; HEAP_SIZE]);

#[global_allocator]
static ALLOC: ferros_heap::LockedHeap = ferros_heap::LockedHeap::empty();

pub extern "C" fn modem_main(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    unsafe { ALLOC.init((&raw mut HEAP) as usize, HEAP_SIZE) };
    hw::init(grants);
    // After the jump from a running Android (fact 31 = 6) the CP is still in Android's session, and this driver has no way yet to reset it: the first jump that let it run hung the bus on one of its register reads (core 7 in SYS_MMIO_READ for this thread, the APM watchdog reset the device, 2026-11-01). Stand down until the CP reset exists.
    // Fact 31 = 7, ferros's own restart, leaves the CP in the last session's state: the same.
    if matches!(ferros_sys::facts().0[31], 6 | 7) {
        hw::say(b"modem: started by the jump from Android; the CP is Android's, left alone (no CP reset yet)\n");
        ferros_sys::exit(0)
    }
    let base = ferros_sys::arc_base();
    let arg: alloc::vec::Vec<u8> = (0..input_len as usize).map(|k| unsafe { core::ptr::read_volatile((base + input + k as u64) as *const u8) }).collect();
    let mut rep = alloc::vec::Vec::new();
    // `off`: the CP powered down (a driver killed leaves it running), reported, then exit.
    if arg == b"off" {
        modem::off(&mut rep);
        hw::say(&rep);
        ferros_sys::exit(0)
    }
    // `sN`: a staged diagnostic run, reported, then exit.
    if let Some(d) = arg.strip_prefix(b"s").filter(|d| !d.is_empty() && d.len() <= 2) {
        if let Some(n) = d.iter().try_fold(0u8, |a, &c| Some(a << 4 | (c as char).to_digit(16)? as u8)) {
            modem::stages(&mut rep, n);
            hw::say(&rep);
            ferros_sys::exit(0)
        }
    }
    // `rmnet` on its own thread, from the start: it answers at once from the service's state ("no call" while the CP comes up), so a caller is never held by a bring-up that takes 20 s.
    static mut RMNET_STACK: [u8; 1 << 16] = [0; 1 << 16];
    ferros_sys::thread_spawn(rmnet_thread, unsafe { &mut *(&raw mut RMNET_STACK) }, 0);
    // The interrupt lines the kernel adds after the last grant (process.rs `LINES_WITH`: the CP's MSI, then CP2AP_WAKEUP), told from a `cell` grant by asking: a wait with a past deadline answers ERR for a slot that is no line.
    let (cell, msi, wake) = lines(grants);
    // `cell` on its own thread when it was granted (slot 25, after the partitions): the place service's serving cell (docs/LOCATION.md). init's list has no slot for it yet (its grants page is full), so under init the driver serves only `rmnet`.
    if cell {
        static mut CELL_STACK: [u8; 1 << 14] = [0; 1 << 14];
        ferros_sys::thread_spawn(cell_thread, unsafe { &mut *(&raw mut CELL_STACK) }, 0);
    }
    // Up, and up again whenever the service ends (the CP crashed, a ring went bad) or RESTART asks: the CP powered down first. The boot-channel download fails now and then on its first try (MAIN unanswered) and works the next; a CP that keeps failing is tried 16 s after, the wait doubling to 1024 s (the policy's cap: a broken modem is retried every 17 minutes), and back to 16 s once it comes up.
    let mut wait_s = 16u64;
    let mut first = true;
    let mut threads_up = false;
    loop {
        // The process watchdog's pet, once a pass (ferros_sys::pet; measuring, `diag pets`).
        ferros_sys::pet();
        if !first {
            // The service state cleared with it (`down`), or `up` would find the old one and do nothing.
            modem::down(&mut rep);
            hw::say(&rep);
            rep.clear();
        }
        first = false;
        modem::up(&mut rep);
        hw::say(&rep);
        rep.clear();
        if !modem::online() {
            hw::log(&alloc::format!("bring-up failed: the CP powered down, again in A#{wait_s} s"));
            ferros_sys::sleep_until(ferros_sys::now() + ferros_sys::freq() * wait_s);
            if wait_s < 1024 {
                wait_s *= 2;
            }
            continue;
        }
        wait_s = 16;
        // With both lines (a kernel from 2026-11-13): CP2AP_WAKEUP as an external interrupt (the bring-up left it an input), each line's thread started once, and the loop waits on them; without, it looks every millisecond as before.
        let irq = match (msi, wake) {
            (Some(m), Some(w)) => {
                modem::wakeup_setup();
                if !threads_up {
                    threads_up = true;
                    static mut MSI_STACK: [u8; 1 << 13] = [0; 1 << 13];
                    static mut WAKE_STACK: [u8; 1 << 13] = [0; 1 << 13];
                    ferros_sys::thread_spawn(irq_thread, unsafe { &mut *(&raw mut MSI_STACK) }, 0);
                    ferros_sys::thread_spawn(irq_thread, unsafe { &mut *(&raw mut WAKE_STACK) }, 1);
                    hw::log("modem: waiting on the CP's MSI and CP2AP_WAKEUP, no timer between its timed actions");
                }
                Some((m, w))
            }
            _ => {
                hw::log("modem: no interrupt lines granted (a kernel before 2026-11-13): looking every millisecond");
                None
            }
        };
        let mut passes = 0u32;
        let mut waits = 0u32;
        let mut said_at = ferros_sys::now();
        while modem::online() && !modem::RESTART.swap(false, core::sync::atomic::Ordering::Relaxed) {
            // The watchdog's pet in the service loop proper (the outer loop is the bring-up's, once per attempt).
            ferros_sys::pet();
            let seen = modem::RUNG.load(core::sync::atomic::Ordering::Acquire);
            modem::service_poll();
            modem::watch_poke();
            passes += 1;
            // The loop's wakes, said at the first pass 64 s or more after the last line (no timer brings one): passes, and waits on the lines.
            if ferros_sys::now().wrapping_sub(said_at) >= ferros_sys::freq() << 6 {
                hw::log(&alloc::format!("modem: A#{passes} passes, A#{waits} waits in the last A#{} ms", (ferros_sys::now() - said_at) * 1000 / ferros_sys::freq()));
                (passes, waits, said_at) = (0, 0, ferros_sys::now());
            }
            let Some((m, w)) = irq else {
                ferros_sys::sleep_until(ferros_sys::now() + ferros_sys::freq() / 1000);
                continue;
            };
            // A line its last interrupt left masked re-armed (its latch cleared, the line acked, its thread woken), then the pass again before any wait: what came before the clear is read by it, what comes after raises the line again. The MSI only while the link is up.
            use core::sync::atomic::Ordering::{Acquire, Release};
            let mut again = false;
            if ARMED[0].load(Acquire) == 0 && modem::msi_arm() {
                ferros_sys::irq_ack(hw::cap(m));
                ARMED[0].store(1, Release);
                ferros_sys::wake(&ARMED[0], 1);
                again = true;
            }
            if ARMED[1].load(Acquire) == 0 {
                modem::wakeup_arm();
                ferros_sys::irq_ack(hw::cap(w));
                ARMED[1].store(1, Release);
                ferros_sys::wake(&ARMED[1], 1);
                again = true;
            }
            if again {
                continue;
            }
            waits += 1;
            modem::ring_wait(seen);
        }
        hw::log("the service ended or a restart was asked: the CP down and up again");
    }
}

/// The interrupt lines' grant slots (`lines`): the CP's MSI, then CP2AP_WAKEUP.
static LINE_SLOT: [core::sync::atomic::AtomicU64; 2] = [const { core::sync::atomic::AtomicU64::new(0) }; 2];
/// 1 while a line's thread waits on it; 0 once it took an interrupt and left the line masked until the service loop has read what came (as payloads/pcie1: one interrupt an idle spell).
static ARMED: [core::sync::atomic::AtomicU64; 2] = [const { core::sync::atomic::AtomicU64::new(0) }; 2];

/// After the partitions: `cell` if granted (slot 25), then the kernel's interrupt lines (process.rs `LINES_WITH`, the MSI then the wakeup). Each slot asked with a wait whose deadline has passed: ERR for a slot that is no line. Returns whether `cell` is granted and the two lines' slots.
fn lines(grants: u64) -> (bool, Option<u64>, Option<u64>) {
    let n = grant_count(grants);
    let mut found: [Option<u64>; 2] = [None; 2];
    let mut k = 0;
    let mut cell = false;
    for slot in S_CELL..n {
        if ferros_sys::irq_wait(hw::cap(slot), 1) == ferros_sys::ERR {
            cell |= slot == S_CELL;
            continue;
        }
        // WHY the test: the kernel adds two lines; a third slot answering as a line is not one this driver knows.
        if k < 2 {
            LINE_SLOT[k].store(slot, core::sync::atomic::Ordering::Relaxed);
            found[k] = Some(slot);
            k += 1;
        }
    }
    (cell, found[0], found[1])
}

/// A line's thread (`k` 0 the MSI, 1 the wakeup): armed, it waits on the line with no deadline; the interrupt counts the service's doorbell up and wakes the loop, and the thread disarms itself, the line left masked (the kernel masks it until `irq_ack`) until the loop re-arms it.
extern "C" fn irq_thread(k: u64) -> ! {
    use core::sync::atomic::Ordering::{Acquire, Relaxed, Release};
    let k = k as usize;
    let cap = hw::cap(LINE_SLOT[k].load(Relaxed));
    loop {
        while ARMED[k].load(Acquire) == 0 {
            ferros_sys::wait_world(&ARMED[k], 0, 0);
        }
        ferros_sys::irq_wait_world(cap, 0);
        ARMED[k].store(0, Release);
        modem::RUNG.fetch_add(1, Release);
        ferros_sys::wake(&modem::RUNG, 1);
    }
}

/// The `rmnet` endpoint's server: one request at a time, each answered at once (modem.rs `rmnet_call`), but a WATCH, held until DL packets wait or the call changes.
extern "C" fn rmnet_thread(_: u64) -> ! {
    let mut msg = [0u8; 4096];
    let mut tok = [0u8; 32];
    loop {
        let n = ferros_sys::recv(hw::cap(S_RMNET), &mut msg, &mut tok, false);
        // A refused receive is a slot that is no endpoint this driver serves, for good: the thread ends, as `cell`'s does (until 2026-11-13 it retried every millisecond forever).
        if n == ferros_sys::ERR {
            hw::log("modem: `rmnet` refused a receive; not serving it");
            ferros_sys::thread_exit(0);
        }
        if let Some(r) = modem::rmnet_call(&msg[..(n as usize).min(msg.len())], &tok) {
            ferros_sys::reply(&tok, &r);
        }
    }
}

/// How many grants the process holds, the console's included (the grants page's u32 at G#200, kernel process.rs `TOKENS`).
fn grant_count(grants: u64) -> u64 {
    unsafe { core::ptr::read_volatile((ferros_sys::arc_base() + grants + 0x200) as *const u32) as u64 }
}

/// The `cell` endpoint's server (modem.rs `cell_call`): GET answered at once, WATCH answered at once or held until the serving cell changes, the service answering it then. A receive that fails ends the thread: the slot is not an endpoint this driver serves.
extern "C" fn cell_thread(_: u64) -> ! {
    let mut msg = [0u8; 1 << 4];
    let mut tok = [0u8; 32];
    loop {
        let n = ferros_sys::recv(hw::cap(S_CELL), &mut msg, &mut tok, false);
        if n == ferros_sys::ERR {
            hw::log("modem: `cell` refused a receive; not serving it");
            ferros_sys::thread_exit(0);
        }
        if let Some(r) = modem::cell_call(&msg[..(n as usize).min(msg.len())], &tok) {
            ferros_sys::reply(&tok, &r);
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    hw::log(&alloc::format!("modem: {info}"));
    ferros_sys::panic_exit()
}
