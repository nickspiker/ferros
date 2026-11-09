//! The USB device driver as a process (USB.md: USB out of the kernel): the DWC3 controller on HSI0, driven through its registers mapped for EL0 (`usb-regs`) and DMA memory it owns (`usb-dma`), woken by its interrupt (`usb-dwc3`), serving Photon Transport to the bridge, each command handed to the kernel (SYS_PT). init starts it at boot from the vault capsule `usb`.
//!
//! By hand: ferros-bridge spawn usb.bin 73 --name usb --dma usb-regs --dma usb-dma --irq usb-dwc3 --pt --power usb
//! Grant slots, in that order: 0 console, 1 usb-regs, 2 usb-dma, 3 the interrupt, 4 the kernel's PT commands, 5 the `usb` power switch.
//!
//! Inputs (hex on the bridge): none or `s` serves (init starts it so); `p` probes the grants; `d` hands the kernel a plain `diag` through the PT endpoint (USB.md step 3) and prints the reply's length and its first lines. `h` takes the controller from the kernel for 2 s and gives it back; `k` takes it and exits, for the kernel to take it back from a driver that is gone.
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
const S_IRQ: u64 = 3;
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
pub extern "C" fn payload_main(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let input = unsafe { core::slice::from_raw_parts((ferros_sys::arc_base() + input) as *const u8, input_len as usize) };
    if input.first() == Some(&b'd') {
        diag(grants);
    }
    // No input (init's spawn) or `s`: the driver.
    if input.is_empty() || input.first() == Some(&b's') {
        serve(grants);
    }
    if let Some(&m @ (b'h' | b'k')) = input.first() {
        let on = ferros_sys::power(cap(grants, S_POWER), true);
        say!(grants, "usb: controller taken: {on}\n");
        // `h`: held two seconds, then this process ends; the kernel switches the controller off as it does (one shot, no switch-off of its own).
        if m == b'h' {
            ferros_sys::sleep_until(ferros_sys::now() + 2 * ferros_sys::freq());
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

/// The DMA region's layout:
/// - the controller's block;
/// - the inbound and outbound bitmaps;
/// - two run buffers, 512 KiB each: runs of 512-byte packets as they come in (a transfer's DATA: one run fills while the other is decoded) or go out (a reply's SPEC and DATA, through the first), 1024 at a time;
/// - the area, 6 MiB: a command as it is assembled and, once the kernel has it, the kernel's reply as it goes out (the host waits for one before it sends the next).
const MEM_AT: usize = 0;
const IN_BITMAP_AT: usize = 1 << 16;
const OUT_BITMAP_AT: usize = 3 << 15;
const OUT_BITMAP_LEN: usize = 1 << 15;
const RUN_AT: usize = 1 << 20;
/// Packets in one run buffer; there are two, back to back, so the next run fills while the last is decoded.
const RUN_PACKETS: usize = 1 << 10;
const AREA_AT: usize = 2 << 20;
const AREA_LEN: usize = 6 << 20;
const PACKET: usize = 512;

/// A reply going out, in runs: the SPEC first, then the DATA packets, each padded to 512 bytes as the kernel's own USB sends them.
struct Out {
    ob: ferros_pt::OutboundTransfer<'static>,
    reply: &'static [u8],
    spec: [u8; PACKET],
    spec_sent: bool,
}

/// `s`: the USB driver. Takes the controller from the kernel (the `usb` power switch readies the PHY and the DMA path), resets and runs it with ferros_hal's DWC3 code over this process's grants, and serves Photon Transport: each command received whole goes to the kernel (SYS_PT), its reply back to the host.
///
/// A transfer's DATA comes in runs: after the SPEC is acknowledged, the OUT endpoint is armed for up to 2048 packets at once straight into the run buffer, so a megabyte costs one completion instead of 2048. Replies go out the same way. Polled for now; the interrupt comes next (USB.md step 4).
fn serve(grants: u64) -> ! {
    use ferros_hal::usb::{Dwc3Dev, UsbEvent, UsbMem};
    use ferros_pt::packet;
    let regs = ferros_sys::dma(cap(grants, S_REGS)) as usize;
    let dma = ferros_sys::dma(cap(grants, S_DMA)) as usize;
    let pt = cap(grants, S_PT);
    let power = cap(grants, S_POWER);
    let irq = cap(grants, S_IRQ);
    // Wake-ups by the interrupt (the rest are the quarter-second deadline), for the console line.
    let mut irq_wakes = 0u64;
    let run = dma + RUN_AT;
    // SAFETY: the DMA grant is this process's alone, 8 MiB, mapped Normal non-cacheable at its physical address; these regions do not overlap, and each is used by one transfer at a time.
    let area = unsafe { core::slice::from_raw_parts_mut((dma + AREA_AT) as *mut u8, AREA_LEN) } as *mut [u8];
    let in_bitmap = unsafe { core::slice::from_raw_parts_mut((dma + IN_BITMAP_AT) as *mut u64, (OUT_BITMAP_AT - IN_BITMAP_AT) / 8) } as *mut [u64];
    let out_bitmap = unsafe { core::slice::from_raw_parts_mut((dma + OUT_BITMAP_AT) as *mut u64, OUT_BITMAP_LEN / 8) } as *mut [u64];
    // Run buffer `k` (0 or 1), as many packets as asked.
    let run_at = |k: usize| run + k * RUN_PACKETS * PACKET;
    let run_buf = |k: usize, n: usize| -> &'static mut [u8] { unsafe { core::slice::from_raw_parts_mut(run_at(k) as *mut u8, n * PACKET) } };

    if !ferros_sys::power(power, true) {
        say!(grants, "usb: the kernel kept the controller\n");
        ferros_sys::exit(1);
    }
    ferros_hal::usb::set_dwc3_base(regs);
    ferros_hal::usb::set_usb_mem((dma + MEM_AT) as *mut UsbMem);
    let Some(mut usb) = Dwc3Dev::init() else {
        give_up(grants, power, "DWC3 init failed");
    };
    say!(grants, "usb: serving\n");
    let freq = ferros_sys::freq();
    let mut t_init = ferros_sys::now();
    // When the link was last seen awake (not suspended): `host_gone` below.
    let mut t_awake = t_init;
    let mut usb_shown = (0i64, 0i64, 0u32);
    let mut inbound: Option<ferros_pt::InboundTransfer<'static>> = None;
    let mut seq_width = 0usize;
    // The inbound transfer's DATA packets not yet armed for, the run buffer armed now, and how many packets it was armed for (0: the endpoint takes single packets).
    let mut to_arm = 0usize;
    let mut cur = 0usize;
    let mut armed_run = 0usize;
    let mut t_run = 0u64;
    let mut out: Option<Out> = None;
    // A command with the kernel, its reply not yet taken.
    let mut waiting = false;
    let mut t_submit = 0u64;
    // A command that acts once everything before it is out (reload, reboot, `diag android`): a copy of it, to hand over again.
    let mut act: Option<([u8; 256], usize)> = None;
    let mut held = ([0u8; 256], 0usize);
    let mut attempts = 1u64;
    // Timing of the last command: its SPEC, its last packet, its reply from the kernel, and the reply's length.
    let (mut t_spec, mut t_whole, mut t_reply, mut reply_len) = (0u64, 0u64, 0u64, 0usize);
    // CPU time in the packet loops: decoding what came in, framing what goes out.
    let (mut cpu_in, mut cpu_out) = (0u64, 0u64);

    loop {
        // The process watchdog's pet, once a pass (ferros_sys::pet; measuring, `diag pets`).
        ferros_sys::pet();
        let ev = usb.poll_event();
        let idle = matches!(ev, UsbEvent::None);
        // Packets to take this pass: one from the single-packet buffer, or a whole run.
        let mut packets: &[u8] = &[];
        let mut single = Packet([0u8; PACKET]);
        match ev {
            UsbEvent::None | UsbEvent::TransferNotReady { .. } => {}
            UsbEvent::Reset => {
                usb.handle_reset();
                inbound = None;
                out = None;
                armed_run = 0;
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
                armed_run = 0;
            }
            UsbEvent::Ep0Setup { request } => {
                if !usb.handle_setup(&request) {
                    usb.ep0_stall();
                }
            }
            UsbEvent::TransferComplete { ep: 2 } => {
                if armed_run > 0 {
                    let n = usb.bulk_out_received().unwrap_or(0);
                    let done = cur;
                    armed_run = 0;
                    // The next run armed into the other buffer before this one is decoded, so the host keeps sending while the CPU works.
                    cur ^= 1;
                    if to_arm > 0 {
                        armed_run = to_arm.min(RUN_PACKETS);
                        to_arm -= armed_run;
                        usb.bulk_out_arm_at(run_at(cur), armed_run * PACKET);
                        t_run = ferros_sys::now();
                        ferros_sys::pt_moving(pt);
                    }
                    packets = &run_buf(done, RUN_PACKETS)[..n - n % PACKET];
                } else if let Some(d) = usb.bulk_out_read() {
                    let n = d.len().min(PACKET);
                    single.0[..n].copy_from_slice(&d[..n]);
                    packets = &single.0[..];
                }
            }
            UsbEvent::TransferComplete { .. } => {}
        }

        let t_cpu = ferros_sys::now();
        let mut local = Packet([0; PACKET]);
        for k in 0..packets.len() / PACKET {
            // Each packet into cached memory first, 16 bytes a load: decoding straight out of the non-cacheable run buffer cost a bus round trip per byte.
            copy_packet(local.0.as_mut_ptr(), packets[k * PACKET..].as_ptr());
            let pkt = &local.0[..];
            if ferros_pt::is_data_packet(pkt[0]) {
                let Some(x) = inbound.as_mut() else { continue };
                let Some((_sid, seq, hash, payload)) = packet::decode_data(pkt, seq_width) else { continue };
                x.handle_data(seq, &hash, payload);
                if !x.all_received() {
                    continue;
                }
                let mut done = [0u8; PACKET];
                let l = x.finish(&mut done);
                if l > 0 {
                    usb.bulk_in_send(&done[..l]);
                }
                let cmd = x.payload();
                t_whole = ferros_sys::now();
                if cmd.len() <= held.0.len() {
                    held.0[..cmd.len()].copy_from_slice(cmd);
                    held.1 = cmd.len();
                } else {
                    held.1 = 0;
                }
                // Straight from the area: the kernel copies it out of this process's DMA region.
                waiting = ferros_sys::pt_submit_at(pt, ferros_sys::DMA_ADDR | cmd.as_ptr() as u64, cmd.len(), false);
                t_submit = ferros_sys::now();
                inbound = None;
                to_arm = 0;
                if !waiting {
                    give_up(grants, power, "the kernel refused a command");
                }
            } else if ferros_pt::is_control_packet(pkt[0]) {
                let Some(spec) = packet::Spec::decode(pkt) else { continue };
                inbound = None;
                out = None;
                seq_width = ferros_ledger::ewe::seq_width(spec.count);
                let total = spec.total as usize;
                let words = ferros_pt::transfer::bitmap_words(spec.count);
                // The host's size is external input: over the area, refused as the kernel refuses it (COMPLETE-failed).
                let mut b = [0u8; 64];
                if total > AREA_LEN || words > (OUT_BITMAP_AT - IN_BITMAP_AT) / 8 {
                    let c = packet::Complete { sid: spec.sid, success: false, data_hash: [0; 32] };
                    let l = c.encode(&mut b);
                    usb.bulk_in_send(&b[..l]);
                    continue;
                }
                // SAFETY: as above.
                let x = unsafe { ferros_pt::InboundTransfer::new(&spec, &mut (&mut *area)[..total], &mut (&mut *in_bitmap)[..words]) };
                if let Some(x) = x {
                    let ack = packet::Ack { sid: spec.sid, seq: u64::MAX };
                    let l = ack.encode(&mut b);
                    usb.bulk_in_send(&b[..l]);
                    inbound = Some(x);
                    to_arm = spec.count as usize;
                    // The cores to their active clocks while it moves: the kernel's clock policy cannot see this process's transfers otherwise.
                    ferros_sys::pt_moving(pt);
                    t_spec = ferros_sys::now();
                }
            }
        }

        cpu_in += ferros_sys::now().wrapping_sub(t_cpu);
        // The OUT endpoint after a completion, unless a run is armed already: the first run of a transfer just acknowledged, or single packets (a SPEC, a command's end).
        if matches!(ev, UsbEvent::TransferComplete { ep: 2 }) && armed_run == 0 {
            if inbound.is_some() && to_arm > 0 {
                armed_run = to_arm.min(RUN_PACKETS);
                to_arm -= armed_run;
                usb.bulk_out_arm_at(run_at(cur), armed_run * PACKET);
                t_run = ferros_sys::now();
            } else {
                usb.bulk_out_arm();
            }
        }
        // A run that stops filling (the host gave up mid-transfer): dropped, back to single packets, so the host's next SPEC is heard.
        if armed_run > 0 && ferros_sys::now().wrapping_sub(t_run) > 2 * freq {
            armed_run = 0;
            inbound = None;
            to_arm = 0;
            usb.bulk_out_arm();
        }

        // The kernel's reply, taken into the area (the command there is the kernel's now) and started on its way out.
        if waiting {
            match ferros_sys::pt_take_at(pt, 0, ferros_sys::DMA_ADDR | (dma + AREA_AT) as u64, AREA_LEN) {
                // A command the kernel never answers: this driver gives up, and init's keeper starts a fresh one.
                ferros_sys::PtReply::Running if ferros_sys::now().wrapping_sub(t_submit) > 16 * freq => give_up(grants, power, "no reply from the kernel in 16 s"),
                ferros_sys::PtReply::Running => ferros_sys::sleep_until(ferros_sys::now() + freq / 16384),
                ferros_sys::PtReply::None => waiting = false,
                ferros_sys::PtReply::Done(total, replied, wants_act) => {
                    waiting = false;
                    t_reply = ferros_sys::now();
                    reply_len = total;
                    if wants_act && held.1 > 0 {
                        act = Some(held);
                    }
                    if replied && total <= AREA_LEN {
                        // SAFETY: as above; the area holds the reply until it is out.
                        let reply: &'static [u8] = unsafe { &(&*area)[..total] };
                        let mut spec = [0u8; PACKET];
                        let bm = unsafe { &mut (&mut *out_bitmap)[..ferros_pt::transfer::outbound_bitmap_words(total)] };
                        if let Some((ob, _)) = ferros_pt::OutboundTransfer::start(ferros_pt::StreamId::FIRST, reply, bm, &mut spec) {
                            out = Some(Out { ob, reply, spec, spec_sent: false });
                        }
                    }
                }
            }
        }

        // The reply out in runs of up to 2048 packets, each run framed into the run buffer while the endpoint is free.
        if let Some(o) = out.as_mut() {
            if usb.bulk_in_is_idle() {
                if o.spec_sent && o.ob.all_sent() {
                    out = None;
                    // Where a large command's time went (USB.md step 4 measurements), into the console: receiving it, the kernel, sending the reply.
                    if reply_len >= 1 << 16 || t_whole.wrapping_sub(t_spec) > freq / 64 {
                        let us = |a: u64, b: u64| b.wrapping_sub(a) * 1_000_000 / freq;
                        say!(grants, "usb: in A#{} us (CPU A#{} us), kernel A#{} us, reply A#{} bytes out in A#{} us (CPU A#{} us); interrupt wake-ups A#{}\n", us(t_spec, t_whole), us(0, cpu_in), us(t_submit, t_reply), reply_len, us(t_reply, ferros_sys::now()), us(0, cpu_out), irq_wakes);
                        (cpu_in, cpu_out) = (0, 0);
                    }
                } else {
                    let t_cpu = ferros_sys::now();
                    // Each packet framed in cached memory, then written to the run buffer 16 bytes a store.
                    let mut local = Packet([0; PACKET]);
                    let mut n = 0usize;
                    if !o.spec_sent {
                        local.0.copy_from_slice(&o.spec);
                        copy_packet(run as *mut u8, local.0.as_ptr());
                        o.spec_sent = true;
                        n = 1;
                    }
                    while n < RUN_PACKETS && !o.ob.all_sent() {
                        local.0.fill(0);
                        if o.ob.next_data_packet(o.reply, &mut local.0) == 0 {
                            break;
                        }
                        copy_packet((run + n * PACKET) as *mut u8, local.0.as_ptr());
                        n += 1;
                    }
                    usb.bulk_in_send_at(run, n * PACKET);
                    ferros_sys::pt_moving(pt);
                    cpu_out += ferros_sys::now().wrapping_sub(t_cpu);
                }
            }
        }

        // A command that acts (reload, reboot, `diag android`): once everything before it is out, handed over again to act.
        if out.is_none() && !waiting && usb.bulk_in_is_idle() {
            if let Some((c, l)) = act.take() {
                ferros_sys::pt_submit(pt, &c[..l], true);
            }
        }

        // The USB circle in the corners (SYS_STATUS): connected once configured, with the link's signalling rate; sent when it changes.
        let st = if usb.is_configured() { 2 } else { 1 };
        let rate: i64 = match usb.connected_speed() {
            0 => 480_000_000,
            1 => 12_000_000,
            2 => 1_500_000,
            4 => 5_000_000_000,
            5 => 10_000_000_000,
            _ => -1,
        };
        // The link's power state too (DSTS.USBLNKST: 0 U0 or on, 1 U1, 2 U2 or L1 sleep, 3 U3 or L2 suspended; above 3 the link is not up, and no power state is shown).
        let lp = usb.link_state();
        if (st, rate, lp) != usb_shown {
            usb_shown = (st, rate, lp);
            ferros_sys::status(0, ferros_sys::measure_word(ferros_sys::measure::STATE, st));
            ferros_sys::status(1, if st == 2 && rate > 0 { ferros_sys::measure_word(ferros_sys::measure::RATE_DOWN, rate) } else { ferros_sys::MEASURE_NONE });
            ferros_sys::status(2, if st == 2 && lp <= 3 { ferros_sys::measure_word(ferros_sys::measure::LINK_POWER, lp as i64) } else { ferros_sys::MEASURE_NONE });
        }
        // The host gone, as the kernel's loop had it: configured, but the link suspended for 16 s. A pulled cable looks just like that (no disconnect event; the link goes to sleep and the driver stays configured), so without this the enumeration watchdog below never fired and a replugged cable found the device silent (2026-11-01, twice: the check went out of the kernel with its USB and not into this driver). The link brought up again, as a fresh start: four tries, then a fresh driver.
        let now = ferros_sys::now();
        if usb.link_state() != 3 {
            t_awake = now;
        }
        let host_gone = usb.is_configured() && now.wrapping_sub(t_awake) > 16 * freq;
        if host_gone {
            say!(grants, "usb: link suspended 16 s while configured: the host has gone, reconnecting\n");
            t_awake = now;
            attempts = 0;
        }
        // Enumeration watchdog, as the kernel's: not configured 4 s after an init, the link is brought up again.
        if host_gone || !usb.is_configured() && now.wrapping_sub(t_init) > 4 * freq {
            attempts += 1;
            // Never configured after four tries: this driver gives up, and init's keeper starts a fresh one (the link readied again from the start).
            if attempts > 4 {
                give_up(grants, power, "not configured after four tries");
            }
            ferros_sys::power(power, true);
            if let Some(u) = Dwc3Dev::init() {
                usb = u;
            }
            inbound = None;
            out = None;
            armed_run = 0;
            t_init = ferros_sys::now();
        }

        // No event this pass and no reply awaited: sleep until the controller interrupts (a packet, a finished run, a reply sent, the host) or a quarter second passes for the checks above. The events just drained have lowered the line (the HAL writes their count back), so it is unmasked first; one that came since raises it again at once.
        if idle && !waiting {
            ferros_sys::irq_ack(irq);
            if ferros_sys::irq_wait(irq, ferros_sys::now() + freq / 4) == 0 {
                irq_wakes += 1;
            }
        }
    }
}

/// Give up: the reason on the console and this process gone, one shot (owner, 2026-11-08): the kernel switches the controller off as this process ends (it switched it on), and init's keeper starts the driver again (payloads/init), its `power usb on` readying the link afresh; the kernel has no USB of its own to fall back to since 2026-11-01.
fn give_up(grants: u64, _power: u64, why: &str) -> ! {
    say!(grants, "usb: {why}; this driver ends, init's keeper starts another\n");
    ferros_sys::exit(2)
}

/// One packet's bytes, aligned for 16-byte loads and stores.
#[repr(C, align(16))]
struct Packet([u8; PACKET]);

/// One 512-byte packet between the run buffer (non-cacheable) and cached memory, 64 bytes a load and a store (four vector registers per instruction): from non-cacheable memory every load is a round trip to DRAM, so its width is the throughput (16-byte loads left the driver 78 ms of CPU per 2 MiB received). Both pointers are 16-byte aligned (`Packet`, and the run buffer's 512-byte slots).
fn copy_packet(dst: *mut u8, src: *const u8) {
    unsafe {
        core::arch::asm!(
            "mov {n}, #8",
            "2:",
            "ld1 {{v0.16b, v1.16b, v2.16b, v3.16b}}, [{s}], #64",
            "st1 {{v0.16b, v1.16b, v2.16b, v3.16b}}, [{d}], #64",
            "subs {n}, {n}, #1",
            "b.ne 2b",
            s = inout(reg) src => _,
            d = inout(reg) dst => _,
            n = out(reg) _,
            out("v0") _, out("v1") _, out("v2") _, out("v3") _,
            options(nostack),
        )
    };
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss).
ferros_sys::entry!(payload_main);
