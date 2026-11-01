//! The USB device driver as a process (USB.md: USB out of the kernel): the DWC3 controller on HSI0, driven through its registers mapped for EL0 (`usb-regs`) and DMA memory it owns (`usb-dma`), woken by its interrupt (`usb-dwc3`), serving Photon Transport to the bridge. Being built step by step; for now the input `p` probes the grants and exits.
//!
//! Spawn: ferros-bridge spawn usb.bin p --name usb --dma usb-regs --dma usb-dma --irq usb-dwc3 --pt --power usb
//! Grant slots, in that order: 0 console, 1 usb-regs, 2 usb-dma, 3 the interrupt, 4 the kernel's PT commands, 5 the `usb` power switch.
//!
//! Inputs: `p` probes the grants; `d` hands the kernel a plain `diag` through the PT endpoint (USB.md step 3) and prints the reply's length and its first lines. `h` takes the controller from the kernel for 2 s and gives it back; `k` takes it and exits, for the kernel to take it back from a driver that is gone. `s` serves: the controller from the kernel, reset and run here, Photon Transport to the bridge, every command handed to the kernel (step 4).
#![no_std]
#![no_main]

use core::fmt::Write;
use ferros_sys::{cap, console};

/// No heap: ferros_hal links vsf, which needs an allocator to exist; nothing here allocates, and an allocation would fail (null) rather than take memory.
struct NoAlloc;
unsafe impl core::alloc::GlobalAlloc for NoAlloc {
    unsafe fn alloc(&self, _: core::alloc::Layout) -> *mut u8 {
        core::ptr::null_mut()
    }
    unsafe fn dealloc(&self, _: *mut u8, _: core::alloc::Layout) {}
}
#[global_allocator]
static NO_ALLOC: NoAlloc = NoAlloc;

const S_REGS: u64 = 1;
const S_DMA: u64 = 2;
const S_PT: u64 = 4;
const S_POWER: u64 = 5;

/// GSNPSID: the controller's ID and release (G#5533xxxx for a DWC_usb3).
const GSNPSID: usize = 0xC120;

struct Line {
    b: [u8; 256],
    n: usize,
}
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let k = s.len().min(self.b.len() - self.n);
        self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
        self.n += k;
        Ok(())
    }
}
macro_rules! say {
    ($g:expr, $($t:tt)*) => {{
        let mut l = Line { b: [0; 256], n: 0 };
        let _ = write!(l, $($t)*);
        console($g, &l.b[..l.n]);
    }};
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let input = unsafe { core::slice::from_raw_parts((ferros_sys::arc_base() + input) as *const u8, input_len as usize) };
    if input.first() == Some(&b'd') {
        diag(grants);
    }
    if input.first() == Some(&b's') {
        serve(grants);
    }
    if let Some(&m @ (b'h' | b'k')) = input.first() {
        let on = ferros_sys::power(cap(grants, S_POWER), true);
        say!(grants, "usb: controller taken: {on}\n");
        if m == b'h' {
            ferros_sys::sleep_until(ferros_sys::now() + 2 * ferros_sys::freq());
            let off = ferros_sys::power(cap(grants, S_POWER), false);
            say!(grants, "usb: controller given back: {off}\n");
        }
        ferros_sys::exit(0);
    }
    let regs = ferros_sys::dma(cap(grants, S_REGS)) as usize;
    let dma = ferros_sys::dma(cap(grants, S_DMA)) as usize;
    say!(grants, "usb: regs at G#{regs:X}, DMA at G#{dma:X}\n");
    // The controller's ID, read straight from its registers at EL0.
    let id = unsafe { core::ptr::read_volatile((regs + GSNPSID) as *const u32) };
    // A word written and read back at the far end of the DMA region.
    let probe = (dma + (2 << 20) - 8) as *mut u64;
    unsafe { core::ptr::write_volatile(probe, 0x5553_4244_4D41_0001) };
    let back = unsafe { core::ptr::read_volatile(probe) };
    say!(grants, "usb: GSNPSID G#{id:08X}; DMA word back G#{back:X}\n");
    ferros_sys::exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

/// `d`: a plain `diag` through the kernel's PT endpoint, timed, its reply's length and first lines printed.
fn diag(grants: u64) -> ! {
    let pt = cap(grants, S_PT);
    let mut cmd = [0u8; 64];
    let n = ferros_pt::command::encode(&mut cmd, &ferros_pt::command::dev_cap(ferros_pt::command::caps::DIAG), ferros_pt::Op::Read, b"");
    let t0 = ferros_sys::now();
    if !ferros_sys::pt_submit(pt, &cmd[..n], false) {
        say!(grants, "usb: the kernel refused the command\n");
        ferros_sys::exit(1);
    }
    let mut buf = [0u8; 4096];
    loop {
        match ferros_sys::pt_take(pt, 0, &mut buf) {
            ferros_sys::PtReply::Running => ferros_sys::sleep_until(ferros_sys::now() + ferros_sys::freq() / 1000),
            ferros_sys::PtReply::None => {
                say!(grants, "usb: no command in\n");
                ferros_sys::exit(1);
            }
            ferros_sys::PtReply::Done(total, _, act) => {
                let us = ferros_sys::us_since(t0);
                say!(grants, "usb: diag reply A#{total} bytes in A#{us} us, act {act}\n");
                // The kernel copied at most the buffer: the reply can be longer.
                let shown = total.min(buf.len());
                let end = buf[..shown].iter().enumerate().filter(|(_, c)| **c == b'\n').nth(7).map_or(shown, |(i, _)| i + 1);
                ferros_sys::console(grants, &buf[..end]);
                ferros_sys::exit(0);
            }
        }
    }
}

/// The DMA region's layout: the controller's block first, then the bitmaps, then 6 MiB that holds a command as it arrives and, once the kernel has it, the kernel's reply as it goes out (the host waits for one before it sends the next).
const MEM_AT: usize = 0;
const IN_BITMAP_AT: usize = 1 << 16;
const OUT_BITMAP_AT: usize = 3 << 15;
const AREA_AT: usize = 2 << 20;
const AREA_LEN: usize = 6 << 20;

/// A reply going out: the SPEC first, then each DATA packet, each in a 512-byte USB packet as the kernel's own USB sends them.
struct Out {
    ob: ferros_pt::OutboundTransfer<'static>,
    reply: &'static [u8],
    spec: [u8; 512],
    spec_sent: bool,
}

/// `s`: the USB driver. Takes the controller from the kernel (the `usb` power switch readies the PHY and the DMA path), resets and runs it with ferros_hal's DWC3 code over this process's grants, and serves Photon Transport: each command received whole goes to the kernel (SYS_PT), its reply back to the host. Polled for now; the interrupt and the receive ring come next (USB.md step 4).
fn serve(grants: u64) -> ! {
    use ferros_hal::usb::{Dwc3Dev, UsbEvent, UsbMem};
    use ferros_pt::packet;
    let regs = ferros_sys::dma(cap(grants, S_REGS)) as usize;
    let dma = ferros_sys::dma(cap(grants, S_DMA)) as usize;
    let pt = cap(grants, S_PT);
    let power = cap(grants, S_POWER);
    // SAFETY: the DMA grant is this process's alone, 8 MiB, mapped Normal non-cacheable at its physical address; these regions do not overlap.
    let area: &'static mut [u8] = unsafe { core::slice::from_raw_parts_mut((dma + AREA_AT) as *mut u8, AREA_LEN) };
    let in_bitmap: &'static mut [u64] = unsafe { core::slice::from_raw_parts_mut((dma + IN_BITMAP_AT) as *mut u64, (OUT_BITMAP_AT - IN_BITMAP_AT) / 8) };
    let out_bitmap: &'static mut [u64] = unsafe { core::slice::from_raw_parts_mut((dma + OUT_BITMAP_AT) as *mut u64, (AREA_AT - OUT_BITMAP_AT) / 8) };
    let area = area as *mut [u8];
    let in_bitmap = in_bitmap as *mut [u64];
    let out_bitmap = out_bitmap as *mut [u64];

    if !ferros_sys::power(power, true) {
        say!(grants, "usb: the kernel kept the controller\n");
        ferros_sys::exit(1);
    }
    ferros_hal::usb::set_dwc3_base(regs);
    ferros_hal::usb::set_usb_mem((dma + MEM_AT) as *mut UsbMem);
    let Some(mut usb) = Dwc3Dev::init() else {
        say!(grants, "usb: DWC3 init failed\n");
        ferros_sys::power(power, false);
        ferros_sys::exit(1);
    };
    say!(grants, "usb: serving\n");
    let freq = ferros_sys::freq();
    let mut t_init = ferros_sys::now();
    let mut inbound: Option<ferros_pt::InboundTransfer<'static>> = None;
    let mut seq_width = 0usize;
    let mut out: Option<Out> = None;
    // A command with the kernel, its reply not yet taken.
    let mut waiting = false;
    // A command that acts once its reply (or its COMPLETE) is out: a copy of it, to hand over again.
    let mut act: Option<([u8; 256], usize)> = None;
    let mut held = ([0u8; 256], 0usize);
    let mut t_submit = 0u64;
    let (mut commands, mut attempts) = (0u64, 1u64);
    loop {
        let ev = usb.poll_event();
        let idle = matches!(ev, UsbEvent::None);
        match ev {
            UsbEvent::None => {}
            UsbEvent::Reset => {
                usb.handle_reset();
                inbound = None;
                out = None;
            }
            UsbEvent::ConnectDone { .. } => {
                usb.handle_connect_done();
                usb.ep0_start_setup();
                usb.bulk_out_arm();
            }
            UsbEvent::Disconnect => {
                usb.handle_disconnect();
                inbound = None;
                out = None;
            }
            UsbEvent::Ep0Setup { request } => {
                if !usb.handle_setup(&request) {
                    usb.ep0_stall();
                }
            }
            UsbEvent::TransferComplete { ep } => {
                if ep == 2 {
                    let mut pkt = [0u8; 512];
                    let n = match usb.bulk_out_read() {
                        Some(d) => {
                            let n = d.len().min(512);
                            pkt[..n].copy_from_slice(&d[..n]);
                            n
                        }
                        None => 0,
                    };
                    if n > 0 && ferros_pt::is_data_packet(pkt[0]) {
                        if let Some(x) = inbound.as_mut() {
                            if let Some((_sid, seq, hash, payload)) = packet::decode_data(&pkt[..n], seq_width) {
                                x.handle_data(seq, &hash, payload);
                                if x.all_received() {
                                    let mut done = [0u8; 512];
                                    let l = x.finish(&mut done);
                                    if l > 0 {
                                        usb.bulk_in_send(&done[..l]);
                                    }
                                    let cmd = x.payload();
                                    commands += 1;
                                    if cmd.len() <= held.0.len() {
                                        held.0[..cmd.len()].copy_from_slice(cmd);
                                        held.1 = cmd.len();
                                    } else {
                                        held.1 = 0;
                                    }
                                    // Straight from the area: the kernel copies it out of this process's DMA region.
                                    waiting = ferros_sys::pt_submit_at(pt, ferros_sys::DMA_ADDR | cmd.as_ptr() as u64, cmd.len(), false);
                                    t_submit = ferros_sys::now();
                                    if !waiting {
                                        give_back(grants, power, "the kernel refused a command");
                                    }
                                    inbound = None;
                                }
                            }
                        }
                    } else if n > 0 && ferros_pt::is_control_packet(pkt[0]) {
                        if let Some(spec) = packet::Spec::decode(&pkt[..n]) {
                            inbound = None;
                            out = None;
                            seq_width = ferros_ledger::ewe::seq_width(spec.count);
                            let total = spec.total as usize;
                            let words = ferros_pt::transfer::bitmap_words(spec.count);
                            // The host's size is external input: over the area, refused as the kernel refuses it (COMPLETE-failed).
                            if total > AREA_LEN || words > (OUT_BITMAP_AT - IN_BITMAP_AT) / 8 {
                                let c = packet::Complete { sid: spec.sid, success: false, data_hash: [0; 32] };
                                let mut b = [0u8; 64];
                                let l = c.encode(&mut b);
                                usb.bulk_in_send(&b[..l]);
                            } else {
                                // SAFETY: the area and the bitmap are this process's DMA memory (above), used by one transfer at a time.
                                let x = unsafe { ferros_pt::InboundTransfer::new(&spec, &mut (&mut *area)[..total], &mut (&mut *in_bitmap)[..words]) };
                                if let Some(x) = x {
                                    let ack = packet::Ack { sid: spec.sid, seq: u64::MAX };
                                    let mut b = [0u8; 64];
                                    let l = ack.encode(&mut b);
                                    usb.bulk_in_send(&b[..l]);
                                    inbound = Some(x);
                                }
                            }
                        }
                    }
                    usb.bulk_out_arm();
                }
            }
            UsbEvent::TransferNotReady { .. } => {}
        }
        // The kernel's reply, taken into the area (the command there is the kernel's now) and started on its way out.
        if waiting {
            match ferros_sys::pt_take_at(pt, 0, ferros_sys::DMA_ADDR | (dma + AREA_AT) as u64, AREA_LEN) {
                // While the driver settles: a command the kernel never answers gives the controller back, so the bridge keeps a way in.
                ferros_sys::PtReply::Running if ferros_sys::now().wrapping_sub(t_submit) > 16 * freq => give_back(grants, power, "no reply from the kernel in 16 s"),
                ferros_sys::PtReply::Running => {}
                ferros_sys::PtReply::None => waiting = false,
                ferros_sys::PtReply::Done(total, replied, wants_act) => {
                    waiting = false;
                    if wants_act && held.1 > 0 {
                        act = Some(held);
                    }
                    if replied && total <= AREA_LEN {
                        let reply: &'static [u8] = unsafe { &(&*area)[..total] };
                        let mut spec = [0u8; 512];
                        let bm = unsafe { &mut (&mut *out_bitmap)[..ferros_pt::transfer::outbound_bitmap_words(total)] };
                        if let Some((ob, _)) = ferros_pt::OutboundTransfer::start(ferros_pt::StreamId::FIRST, reply, bm, &mut spec) {
                            out = Some(Out { ob, reply, spec, spec_sent: false });
                        }
                    }
                }
            }
        }
        // The reply out one 512-byte packet at a time, as the bulk endpoint frees.
        let mut sent = false;
        if let Some(o) = out.as_mut() {
            if usb.bulk_in_is_idle() {
                if !o.spec_sent {
                    usb.bulk_in_send(&o.spec);
                    o.spec_sent = true;
                } else {
                    let mut p = [0u8; 512];
                    if o.ob.next_data_packet(o.reply, &mut p) > 0 {
                        usb.bulk_in_send(&p);
                    }
                }
                sent = true;
            }
            if o.spec_sent && o.ob.all_sent() {
                out = None;
            }
        }
        // A command that acts (reload, reboot, `diag android`): once everything before it is out, handed over again to act.
        if out.is_none() && !waiting && usb.bulk_in_is_idle() {
            if let Some((c, l)) = act.take() {
                say!(grants, "usb: acting on command A#{commands}\n");
                ferros_sys::pt_submit(pt, &c[..l], true);
            }
        }
        // Enumeration watchdog, as the kernel's: not configured 4 s after an init, the link is brought up again.
        if !usb.is_configured() && ferros_sys::now().wrapping_sub(t_init) > 4 * freq {
            attempts += 1;
            // While the driver settles: never configured after four tries, the controller goes back to the kernel, so the bridge always has a way in.
            if attempts > 4 {
                give_back(grants, power, "not configured after four tries");
            }
            ferros_sys::power(power, true);
            if let Some(u) = Dwc3Dev::init() {
                usb = u;
            }
            inbound = None;
            out = None;
            t_init = ferros_sys::now();
            say!(grants, "usb: re-initialised (attempt A#{attempts})\n");
        }
        if idle && !sent && inbound.is_none() && !waiting {
            ferros_sys::sleep_until(ferros_sys::now() + freq / 8192);
        }
    }
}

/// The controller back to the kernel, and this process gone: the way out while the driver settles, so the bridge always has a way in.
fn give_back(grants: u64, power: u64, why: &str) -> ! {
    say!(grants, "usb: {why}; giving the controller back\n");
    ferros_sys::power(power, false);
    ferros_sys::exit(2)
}
