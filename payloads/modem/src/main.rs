//! The cellular modem's driver as a process (Phase 2 of the owner's plan, 2026-10-29: the modem out of the kernel into its own driver): it brings the Shannon S5300 up over PCIe channel 0, registers, sets up the internet data call, and serves the call's IP packets on the `rmnet` endpoint to the IP process (payloads/ip). modem.rs is the kernel's bring-up and service (ferros_kernel/src/modem.rs, where it was proven) on this process's grants: hw.rs routes its registers and shared memory, partfs.rs its file reads.
//!
//! `rmnet` requests (first byte), as the kernel answered them in Phase 1:
//! - 1 SEND: an IP packet → [0] sent, [1] the UL queue is full (retry), [2] no data call, [3] bad request.
//! - 2 RECV: → the oldest DL packet of the call, or nothing.
//! - 3 INFO: → flags (bit 0 up, bit 1 IPv4, bit 2 IPv6), MTU (u16 BE), IPv4 (4), IPv6 (16), the first DNS server's IPv4 (4) and IPv6 (16): 43 bytes.
//! - 4 STATE: → the service's state as text (registration, the call without its addresses, packet counts).
//! - 5 DOWN: the CP powered off; the driver exits (its report on the console).
//!
//! Input: empty or `up` brings the modem up and serves until killed; `off` powers the CP down and exits; `sN` runs the staged diagnostic, stages 0..=N, N one or two hex digits (modem.rs `stages`), and exits. The stages (from the kernel, where they were built): 0 survey, 1 power-on, 2 PHY, 3 link, 4 endpoint, 5 ROM boot, 6 link cycle, 7 the boot-channel download, 8 START and a ten-second watch, 9 START and the bare IPC handshake (PIF_INIT_DONE -> PHONE_START -> INIT_END), a the handshake with the pktproc DL rings and the legacy link, then 90 s watching what the online CP sends, b the same with one RIL probe (SIT_GET_RADIO_POWER on channel 245), c also the radio on (SIT_SET_RADIO_POWER, not for emergency), d also the data registration polled every 5 s (SIT_GET_PS_REG_STATE) and the SIM status read once, e also the preferred data modem and allow-data on stack 1, f also the initial-attach APN, 10 instead of the attach APN the carrier-config manifest handshake, the UICC subscription, a radio cycle, the data profiles, the allowed networks (the data stack registers: 2026-10-29) and the stack-status and carrier-restriction queries, 11 also the AP capabilities G#3 with the UL packet rings and the gipc service on channel 132, 12 also the internet data call once registered (VZWINTERNET), then one IPv6 router solicitation and one ICMPv6 echo request to a public resolver up its rmnet channel, the queues' pointers every 4 s and every DL packet by IP version and protocol only; from b on, the modem service on channel 130 (properties, UE-capability files from the modem image) and the remote-file service on channel 41 (writes pulled and dropped) are answered (the whole radio is open during experimentation: owner, 2026-10-29).
//!
//! Spawn (tools/husky-modem.sh): ferros-bridge spawn modem.bin --name modem --serve rmnet --hw pcie0-elbi --hw pcie0-soc --hw pcie0-pcs --hw pcie0-udbg --hw pcie0-phy --hw pcie0-dbi --hw pcie0-cfg --hw pcie0-mem --hw cmu-hsi1 --hw hsi1-sysreg --hw gpio-hsi1 --hw sysmmu-hsi1 --hw s2mpu-hsi1 --hw gpio-alive --hw pmu --dma modem-pktproc --dma modem-ipc --dma modem-msi --part modem_a --part efs --part persist --part modem_userdata --part vendor_a
//! Grant slots: 0 console, 1 serve rmnet, 2-16 the hardware blocks (hw.rs `BLOCKS`), 17-19 the shared-memory windows, 20-24 the partitions (partfs.rs `PARTS`).

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
    // Up, or down and again: the boot-channel download fails now and then on its first try (MAIN unanswered), and works the next.
    for attempt in 0..3 {
        modem::up(&mut rep);
        hw::say(&rep);
        rep.clear();
        if modem::online() {
            break;
        }
        hw::log(&alloc::format!("bring-up attempt A#{attempt} failed: the CP powered down, trying again"));
        modem::off(&mut rep);
        rep.clear();
        ferros_sys::sleep_until(ferros_sys::now() + ferros_sys::freq());
    }
    let mut msg = [0u8; 4096];
    let mut tok = [0u8; 32];
    loop {
        modem::service_poll();
        let mut busy = false;
        for _ in 0..64 {
            let n = ferros_sys::recv(hw::cap(S_RMNET), &mut msg, &mut tok, true);
            if n == ferros_sys::NOTHING || n == ferros_sys::ERR {
                break;
            }
            let r = modem::rmnet_call(&msg[..(n as usize).min(msg.len())]);
            ferros_sys::reply(&tok, &r);
            busy = true;
        }
        if !busy {
            ferros_sys::sleep_until(ferros_sys::now() + ferros_sys::freq() / 1000);
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    hw::log(&alloc::format!("modem: {info}"));
    ferros_sys::panic_exit()
}
