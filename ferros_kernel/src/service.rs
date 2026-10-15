//! The ferros USB service: Photon Transport on the phone's DWC3, shared by both ways ferros boots on husky.
//!
//! - **Handoff** (slot a, chainloaded from the patched Linux): UFS is live and the ira-rooted vault is open, so STORE works; every exit warm-resets back into Android.
//! - **Cold** (slot b, straight from ABL): no Linux; ferros powers the flash back on and brings UFS up itself (cold_ufs.rs), so the vault opens here too. Exits other than a plain host REBOOT go to fastboot, since a plain reset would just boot ferros again (slot marking via devinfo is not implemented yet).
//!
//! Both paths: the USB DMA master is put on physical addresses (HSI0 S2MPU bypassed, every USB SYSMMU VM context disabled), the eUSB2 PHY + DWC3 are (re)initialised with the enumeration watchdog, and PT serves DIAG / RUN / RELOAD / MEM / DISPLAY / STORE / REBOOT. Exits leave a ramoops report (exit reason in the crumb byte): REBOOT from the host, 64 s without enumerating, 512 s without a PT command, a 16 s mid-transfer stall, or both volume buttons; faults and hangs are caught by the fault sink and the watchdog.
//!
//! RELOAD: the image is statically linked with absolute addresses in its data, so every generation must run at its link base. The new image is staged at `Boot::stage`, a tiny copy stub is placed at `Boot::stub`, and the stub copies the image over the link base and jumps to it; x0 carries the generation (bit 0 = reloaded).

use core::ptr::{read_volatile, write_volatile};

use ferros_hal::usb::UsbEvent;
use ferros_pt::packet;
use ferros_pt::transfer::InboundTransfer;

use crate::platform::{self, append_hex, cntfrq, cntpct, open_usb_dma_path, queue_pt_response};
use crate::vault_boot::{report_vault, ShimStore, VaultProbe};

const DWC3: usize = 0x1121_0000;
const USBCON: usize = 0x1110_0000;

/// Which way ferros got here; decides the DIAG banner and where exits go.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BootKind {
    /// Chainloaded from the patched Linux on slot a (runtime trigger): exits warm-reset back into Android.
    #[cfg_attr(not(shim_handoff), allow(dead_code))]
    Handoff,
    /// Booted straight from ABL on slot b: exits other than a plain host REBOOT go to fastboot.
    Cold,
}

/// Everything the service needs from the boot path that brought ferros up.
pub struct Boot {
    pub kind: BootKind,
    /// Address the image is linked at (absolute pointers in its data assume it); RELOAD's copy stub lands every generation here.
    pub link_base: usize,
    /// RELOAD/RUN staging slot, clear of this image, its stack and its heap.
    pub stage: usize,
    /// Where the RELOAD copy stub runs from (clear of both the image and the staging slot).
    pub stub: usize,
    /// x0 at entry: 0 or a DTB pointer from the loader, or ((generation << 1) | 1) from a RELOAD.
    pub x0: u64,
    pub probe: Option<VaultProbe>,
    pub store: Option<ShimStore>,
    /// The previous session's report block, read before `arm_safety` cleared it (survives warm resets).
    pub prev: Option<[u64; 32]>,
    /// Boot-path specific DIAG lines.
    pub extras: alloc::vec::Vec<(&'static [u8], u32)>,
    /// `bridge diag ufs`: the cold path's UFS link-startup probe.
    pub ufs_probe: Option<fn() -> alloc::vec::Vec<u8>>,
}

/// What `reboot_to_bootloader` returned when a fastboot request fell through (0 = never tried); reported in r[23].
static FASTBOOT_FAIL: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Largest RELOAD/RUN image the staging slot takes.
const STAGE_MAX: usize = 4 << 20;

/// Exit reasons (crumb byte in the ramoops report, kernel prints it as the low byte of `mbr_sig`).
const EXIT_REBOOT: u8 = 0xB0;
const EXIT_NO_ENUM: u8 = 0xB1;
const EXIT_IDLE: u8 = 0xB2;
const EXIT_FASTBOOT: u8 = 0xB3;
const EXIT_NO_USB: u8 = 0xB4;
const EXIT_PT_STALL: u8 = 0xB6;

/// PT-level counters for the exit report.
#[derive(Default)]
struct PtStats {
    specs: u32,
    data_pkts: u32,
    data_decoded: u32,
    data_accepted: u32,
    commands: u32,
    not_ready: u32,
}


// Reload copy stub. Position-independent; runs from `Boot::stub` after being copied there, so it can overwrite the image that called it.
// x0 = source (`Boot::stage`), x1 = destination (`Boot::link_base`), x2 = byte count (multiple of 8, nonzero), x3 = x0 for the new image.
// MMU is off at EL2 after the handoff, so data stores go straight to DRAM; only the I-side needs invalidating.
core::arch::global_asm!(
    ".section .text.reload_stub, \"ax\"",
    ".global ferros_reload_stub",
    ".global ferros_reload_stub_end",
    ".balign 8",
    "ferros_reload_stub:",
    "    mov x5, x1",
    "1:  ldr x4, [x0], #8",
    "    str x4, [x1], #8",
    "    subs x2, x2, #8",
    "    b.hi 1b",
    "    dsb sy",
    "    ic iallu",
    "    dsb sy",
    "    isb",
    "    mov x0, x3",
    "    br x5",
    "ferros_reload_stub_end:",
);

unsafe extern "C" {
    static ferros_reload_stub: u8;
    static ferros_reload_stub_end: u8;
}

/// Report the vault + the USB session and warm-reset back to Android. The kernel dumps the whole 256-byte block as r[00..31].
/// USB-service layout: r[4] (0x20) bytes = init attempts, ever-configured, DSTS[7:0], DSTS[15:8], DSTS[23:16]; r[6] = bulk OUT completes | bulk IN completes << 32; r[7] = XferNotReady (HAL) | DEPCMD status failures << 32; r[8] = EP events seen | SETUPs << 32; r[9] = GEVNTCOUNT | GEVNTSIZ << 32; r[10] = last failed DEPCMD | its ep << 32 | its type << 40; r[11] = bulk_out_armed | bulk_out_ready << 8 | bulk_in_idle << 16 | configured << 24 | ep2 TRB ctrl snapshot << 32; r[12..17] = last 12 raw EP events (oldest-first order is evt_ring_n mod 12); r[18] = PT SPECs | DATA packets << 32; r[19] = DATA decoded | DATA accepted << 32; r[22] = commands | loop XferNotReady << 32.
fn leave(b: &Boot, exit: u8, attempts: u32, configured_ever: bool, usb: Option<&ferros_hal::usb::Dwc3Dev>, st: &PtStats) -> ! {
    report(b, exit);
    let rd = |a: usize| unsafe { read_volatile(a as *const u32) };
    let dsts = rd(DWC3 + 0xC70C);
    let usb_b = [attempts.min(255) as u8, configured_ever as u8, dsts as u8, (dsts >> 8) as u8, (dsts >> 16) as u8];
    let w = |idx: usize, v: u64| unsafe { write_volatile((0xFD60_0000usize + idx * 8) as *mut u64, v) };
    if let Some(u) = usb {
        w(6, u.bulk_out_xfer_complete as u64 | (u.bulk_in_xfer_complete as u64) << 32);
        w(7, u.ep0_xfer_notready as u64 | (u.cmd_status_fail as u64) << 32);
        w(8, u.evt_ring_n as u64 | (u.setup_count as u64) << 32);
        w(10, u.last_cmd_status as u64 | (u.last_cmd_ep as u64) << 32 | (u.last_cmd_type as u64 & 0xFF_FFFF) << 40);
        w(11, u.bulk_out_armed as u64 | (u.bulk_out_ready as u64) << 8 | (u.bulk_in_idle as u64) << 16 | (u.is_configured() as u64) << 24 | (u.ep2_trb_ctrl_snap as u64) << 32);
        for i in 0..6 {
            w(12 + i, u.evt_ring[2 * i] as u64 | (u.evt_ring[2 * i + 1] as u64) << 32);
        }
    }
    w(9, rd(DWC3 + 0xC40C) as u64 | (rd(DWC3 + 0xC408) as u64) << 32); // GEVNTCOUNT(0) | GEVNTSIZ(0)
    w(18, st.specs as u64 | (st.data_pkts as u64) << 32);
    w(19, st.data_decoded as u64 | (st.data_accepted as u64) << 32);
    w(22, st.commands as u64 | (st.not_ready as u64) << 32);
    w(23, FASTBOOT_FAIL.load(core::sync::atomic::Ordering::Relaxed));
    unsafe {
        core::ptr::copy_nonoverlapping(usb_b.as_ptr(), (0xFD60_0000usize + 0x20) as *mut u8, 5);
        ferros_hal::mmio::cache_clean(0xFD60_0000, 0x100);
        // Graceful detach so the host sees a disconnect rather than a dead device.
        let dctl = read_volatile((DWC3 + 0xC704) as *const u32);
        write_volatile((DWC3 + 0xC704) as *mut u32, dctl & !(1 << 31));
    }
    // Cold boot: a plain reset would just boot ferros again, so anything but an explicit host REBOOT goes to fastboot for the host to decide (falls back to a plain reset if the EL3 monitor refuses the reboot-mode write).
    if b.kind == BootKind::Cold && exit != EXIT_REBOOT {
        let _ = platform::reboot_to_bootloader();
    }
    platform::psci_reset()
}

/// Write this session's report: the full vault report when a vault was probed, the minimal one otherwise.
fn report(b: &Boot, exit: u8) {
    match &b.probe {
        Some(p) => report_vault(p, exit),
        None => platform::report_exit(exit),
    }
}

/// `ferros.dev.store` on the ira-keyed vault (every set/remove is sealed by commit_root, so it survives a reset). Responses start with a status byte: 0 ok, 1 key not found, 2 vault error, 3 bad request, 4 no vault open.
/// Write [keylen:2 BE][key][value] -> [0][object hash:32]. Read [key] -> [0][value]; an empty key lists every key, '\n'-separated. Exec [key] -> remove -> [0] or [1].
fn store_command(store: Option<&mut ShimStore>, op: ferros_pt::Op, params: &[u8]) -> alloc::vec::Vec<u8> {
    let store = match store {
        Some(s) => s,
        None => return alloc::vec![4],
    };
    let key_of = |b: &[u8]| core::str::from_utf8(b).ok().map(alloc::string::String::from);
    match op {
        ferros_pt::Op::Write => {
            if params.len() < 2 {
                return alloc::vec![3];
            }
            let klen = u16::from_be_bytes([params[0], params[1]]) as usize;
            if params.len() < 2 + klen {
                return alloc::vec![3];
            }
            let key = match key_of(&params[2..2 + klen]) {
                Some(k) if !k.is_empty() => k,
                _ => return alloc::vec![3],
            };
            match store.set(&key, params[2 + klen..].to_vec()) {
                Ok(h) => {
                    let mut r = alloc::vec![0u8];
                    r.extend_from_slice(&h.0);
                    r
                }
                Err(_) => alloc::vec![2],
            }
        }
        ferros_pt::Op::Read => {
            let key = match key_of(params) {
                Some(k) => k,
                None => return alloc::vec![3],
            };
            if key.is_empty() {
                return match store.keys() {
                    Ok(keys) => {
                        let mut r = alloc::vec![0u8];
                        for k in keys {
                            r.extend_from_slice(k.as_bytes());
                            r.push(b'\n');
                        }
                        r
                    }
                    Err(_) => alloc::vec![2],
                };
            }
            match store.get_by_key(&key) {
                Ok(Some(v)) => {
                    let mut r = alloc::vec![0u8];
                    r.extend_from_slice(&v);
                    r
                }
                Ok(None) => alloc::vec![1],
                Err(_) => alloc::vec![2],
            }
        }
        ferros_pt::Op::Exec => match key_of(params) {
            Some(k) if !k.is_empty() => match store.remove_key(&k) {
                Ok(true) => alloc::vec![0],
                Ok(false) => alloc::vec![1],
                Err(_) => alloc::vec![2],
            },
            _ => alloc::vec![3],
        },
    }
}

/// The boot screen: boot path, generation, vault state + root, and how to get out.
fn status_text(kind: BootKind, generation: u64, probe: Option<&VaultProbe>) -> alloc::vec::Vec<u8> {
    let mut t = alloc::vec::Vec::new();
    let hex = b"0123456789ABCDEF";
    t.extend_from_slice(match kind {
        BootKind::Handoff => b"ferros\n\nchainloaded from Linux, generation G#".as_slice(),
        BootKind::Cold => b"ferros\n\nbooted from ABL, generation G#",
    });
    for i in (0..4).rev() {
        t.push(hex[((generation >> (i * 4)) & 0xF) as usize]);
    }
    let p = match probe {
        Some(p) => p,
        None => {
            t.extend_from_slice(b"\n\nufs: no vault probe\n\nusb: serving Photon Transport\n     1209:4665\n\nboth volume keys\n= power off\n");
            return t;
        }
    };
    t.extend_from_slice(b"\n\nufs: ");
    t.extend_from_slice(if p.link_up { b"link up" } else { b"LINK DOWN" });
    t.extend_from_slice(b"\nvault: ");
    t.extend_from_slice(if p.verified == 1 { b"opened + verified".as_slice() } else if p.sealed == 1 { b"genesis sealed" } else if p.opened == 1 { b"opened, NOT verified" } else { b"NOT OPENED" });
    t.extend_from_slice(b"\nroot: ");
    for &b in &p.root16[..8] {
        t.push(hex[(b >> 4) as usize]);
        t.push(hex[(b & 0xF) as usize]);
    }
    t.extend_from_slice(b"\nwairua: ");
    t.extend_from_slice(match crate::wairua::peek() { None => b"not drawn yet".as_slice(), Some((Some(_), _)) => b"live", Some((None, _)) => b"KOHI FAILED" });
    t.extend_from_slice(b"\n\nusb: serving Photon Transport\n     1209:4665\n\nboth volume keys\n= power off\n");
    t
}

/// Shim entry with the USB service: adopt UFS, arm the safety nets, probe the vault, then serve PT over USB until told to leave.
pub fn run(mut b: Boot) -> ! {
    // Report now too: if the USB bring-up wedges, the watchdog's reset still shows the vault result (handoff) and the crumb shows how far USB got.
    report(&b, 0xA0);
    let generation = if b.x0 & 1 == 1 { b.x0 >> 1 } else { 0 };
    let mut store = b.store.take();
    let t_boot = cntpct();
    let exc_start = crate::exception_count();

    // Boot screen: after a --live handoff DECON is still running, so show where this generation stands before USB even comes up (show() touches nothing when the DPU is powered down).
    if !crate::checklist::active() {
        let _ = crate::display::show(&status_text(b.kind, generation, b.probe.as_ref()), cntfrq());
    }

    let dma = open_usb_dma_path();
    platform::crumb(0xA1);
    crate::pixel8_eusb_phy_init();
    platform::crumb(0xA2);
    ferros_hal::usb::set_dwc3_base(DWC3);
    let mut usb = match ferros_hal::usb::Dwc3Dev::init() {
        Some(u) => u,
        None => leave(&b, EXIT_NO_USB, 1, false, None, &PtStats::default()),
    };
    platform::crumb(0xA3);

    let freq = cntfrq();
    // Enumeration watchdog (cold-boot values): 2 s of total silence = dead attempt; 4 s since init without SET_CONFIGURATION = wedged EP0. Either re-inits PHY + DWC3, which the host sees as a fresh attach.
    let enum_timeout = freq * 2;
    let wedge_timeout = freq * 4;
    // Give up on enumeration after 64 s (Android comes back with the report); give up on an idle session after 512 s without a PT command.
    let enum_giveup = freq * 64;
    let idle_giveup = freq * 512;
    let mut t_init = cntpct();
    let mut t_progress = t_init;
    let mut t_command = t_init;
    let mut attempts: u32 = 1;
    let mut configured_ever = false;
    let mut init_history: alloc::vec::Vec<[u32; 24]> = alloc::vec::Vec::new();
    // Per-attempt bus events seen by the loop: USB resets, ConnectDones, SETUPs (reset with each re-init; recorded into init_history when an attempt is abandoned).
    let mut att_ev = [0u32; 3];
    let mut st = PtStats::default();
    // A PT transfer that stops moving for 16 s is a wedged endpoint: report it and leave rather than idling for 512 s.
    let stall_giveup = freq * 16;
    let mut t_pt = t_init;
    // `display --hold` keeps a message on the panel: the idle exit is off until a host REBOOT ends the session (both volume keys still kill it: killswitch.rs).
    let mut hold = false;

    // Backing storage for the open inbound transfer (it holds raw slices into these); assigned on every SPEC.
    let mut pt_data_buf: alloc::vec::Vec<u8>;
    let mut pt_bitmap_buf: alloc::vec::Vec<u64>;
    let mut pt_inbound: Option<InboundTransfer<'_>> = None;
    let mut pt_seq_width: usize = 0;
    let mut pt_out_data: alloc::vec::Vec<u8> = alloc::vec::Vec::new();

    let cap_diag = ferros_pt::command::dev_cap(ferros_pt::command::caps::DIAG);
    let cap_reload = ferros_pt::command::dev_cap(ferros_pt::command::caps::RELOAD);
    let cap_reboot = ferros_pt::command::dev_cap(ferros_pt::command::caps::REBOOT);
    let cap_run = ferros_pt::command::dev_cap(ferros_pt::command::caps::RUN);
    let cap_spawn = ferros_pt::command::dev_cap(ferros_pt::command::caps::SPAWN);
    let cap_mem = ferros_pt::command::dev_cap(ferros_pt::command::caps::MEM);
    let cap_display = ferros_pt::command::dev_cap(ferros_pt::command::caps::DISPLAY);
    let cap_store = ferros_pt::command::dev_cap(ferros_pt::command::caps::STORE);
    // RELOAD and RUN share STAGE; at most one of these is set, so a payload can never be jumped to as a kernel or vice versa.
    let mut reload_ok = false;
    let mut reload_size = 0usize;
    let mut run_ok = false;
    let mut run_size = 0usize;

    unsafe extern "C" {
        static __stack_top: u8;
    }
    let heap_end = ((&raw const __stack_top as usize + 0xFFF) & !0xFFF) + crate::HEAP_SIZE;

    loop {
        platform::pet_watchdogs();
        let now = cntpct();
        if !configured_ever && now.wrapping_sub(t_boot) > enum_giveup {
            leave(&b, EXIT_NO_ENUM, attempts, configured_ever, Some(&usb), &st);
        }
        if configured_ever && !hold && now.wrapping_sub(t_command) > idle_giveup {
            leave(&b, EXIT_IDLE, attempts, configured_ever, Some(&usb), &st);
        }
        if pt_inbound.is_some() && now.wrapping_sub(t_pt) > stall_giveup {
            leave(&b, EXIT_PT_STALL, attempts, configured_ever, Some(&usb), &st);
        }

        match usb.poll_event() {
            UsbEvent::None => {
                // Idle: give one runnable process a time slice (it comes back at the latest when the slice timer fires).
                crate::process::run_slice();
                if !usb.is_configured() && (now.wrapping_sub(t_progress) > enum_timeout || now.wrapping_sub(t_init) > wedge_timeout) {
                    if init_history.len() < 32 {
                        unsafe {
                            let n = usb.evt_ring_n as usize;
                            let mut h = [0u32; 24];
                            h[..12].copy_from_slice(&[
                                read_volatile((USBCON + 0x80) as *const u32),  // LTSTATE_HIS
                                read_volatile((USBCON + 0x84) as *const u32),  // LINK_DEBUG_L
                                read_volatile((DWC3 + 0xC70C) as *const u32), // DSTS
                                att_ev[0],
                                att_ev[1],
                                att_ev[2],
                                usb.ep0_setup_arm_ok | usb.ep0_setup_arm_fail << 16,
                                usb.ep0_xfer_notready,
                                usb.cmd_status_fail,
                                usb.evt_ring_n,
                                usb.last_setup_brequest as u32 | (usb.last_setup_wvalue as u32) << 16,
                                usb.status_in_count | usb.status_out_count << 16,
                            ]);
                            // The event ring, oldest first.
                            for k in 0..12 {
                                if n >= 12 - k {
                                    h[12 + k] = usb.evt_ring[(n + k) % 12];
                                }
                            }
                            init_history.push(h);
                        }
                    }
                    crate::pixel8_eusb_phy_init();
                    if let Some(nu) = ferros_hal::usb::Dwc3Dev::init() {
                        usb = nu;
                    }
                    attempts += 1;
                    att_ev = [0; 3];
                    pt_inbound = None;
                    pt_seq_width = 0;
                    pt_out_data.clear();
                    t_init = cntpct();
                    t_progress = t_init;
                }
                if !pt_out_data.is_empty() && usb.bulk_in_is_idle() {
                    let len = pt_out_data.len().min(512);
                    let chunk: alloc::vec::Vec<u8> = pt_out_data.drain(..len).collect();
                    usb.bulk_in_send(&chunk);
                }
                for _ in 0..64u32 {
                    core::hint::spin_loop();
                }
            }
            UsbEvent::Reset => {
                att_ev[0] += 1;
                t_progress = cntpct();
                usb.handle_reset();
                pt_inbound = None;
                pt_seq_width = 0;
                pt_out_data.clear();
            }
            UsbEvent::ConnectDone { .. } => {
                att_ev[1] += 1;
                t_progress = cntpct();
                usb.handle_connect_done();
                usb.ep0_start_setup();
                usb.bulk_out_arm();
            }
            UsbEvent::Disconnect => {
                usb.handle_disconnect();
                pt_inbound = None;
                pt_seq_width = 0;
                pt_out_data.clear();
            }
            UsbEvent::Ep0Setup { request } => {
                att_ev[2] += 1;
                t_progress = cntpct();
                if !usb.handle_setup(&request) {
                    usb.ep0_stall();
                }
                if usb.is_configured() && !configured_ever {
                    configured_ever = true;
                    crate::checklist::mark(crate::checklist::Item::Usb);
                    t_command = cntpct();
                    platform::crumb(0xA4);
                }
            }
            UsbEvent::TransferComplete { ep } => {
                t_progress = cntpct();
                if ep != 2 {
                    continue;
                }
                let mut tmp = [0u8; 512];
                let mut n = 0usize;
                if let Some(data) = usb.bulk_out_read() {
                    n = data.len().min(512);
                    tmp[..n].copy_from_slice(&data[..n]);
                }
                if n > 0 && ferros_pt::is_data_packet(tmp[0]) {
                    st.data_pkts += 1;
                    t_pt = cntpct();
                    let mut done = false;
                    if let Some(ref mut xfer) = pt_inbound {
                        if let Some((_sid, seq, hash, payload)) = packet::decode_data(&tmp[..n], pt_seq_width) {
                            st.data_decoded += 1;
                            if xfer.handle_data(seq, &hash, payload) {
                                st.data_accepted += 1;
                            }
                            if xfer.all_received() {
                                st.commands += 1;
                                let mut complete_buf = [0u8; 512];
                                let clen = xfer.finish(&mut complete_buf);
                                if clen > 0 {
                                    usb.bulk_in_send(&complete_buf[..clen]);
                                }
                                t_command = cntpct();
                                done = true;
                                if let Some(cmd) = ferros_pt::command::parse(xfer.payload()) {
                                    if cmd.cap == cap_diag && cmd.op == ferros_pt::Op::Read {
                                        let mut resp: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
                                        if cmd.params == b"rep" {
                                            // The eUSB2 repeater's whole 8-bit register map over I2C (HSI2C11, address from the DT), one "REP_xx=" line per register; G#8000_xxxx = the read failed (TRANS_STATUS low bits). For diffing cold vs handoff state.
                                            let i2c = ferros_hal::hsi2c::Hsi2c::new(ferros_hal::hsi2c::PIXEL8_HSI2C11_BASE);
                                            i2c.init();
                                            let hex = b"0123456789ABCDEF";
                                            for reg in 0..=255u8 {
                                                let v = match i2c.read_reg(ferros_hal::hsi2c::PIXEL8_EUSB_REPEATER_ADDR, reg) {
                                                    Ok(v) => v as u32,
                                                    Err(t) => 0x8000_0000 | (t & 0xFFFF),
                                                };
                                                let label = [b'R', b'E', b'P', b'_', hex[(reg >> 4) as usize], hex[(reg & 0xF) as usize], b'='];
                                                append_hex(&mut resp, &label, v);
                                            }
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"console" {
                                            // What processes have sent their consoles since the last read (background slices included), and how many are alive.
                                            append_hex(&mut resp, b"ALIVE=", crate::process::alive() as u32);
                                            resp.extend_from_slice(&crate::process::take_console());
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"bench" {
                                            // Memory speed, for the cache map's before/after: 8 copies of 512 KiB (4 MiB moved) and BLAKE3 over 512 KiB, timed in microseconds, plus SCTLR_EL2 (M b0, C b2, I b12).
                                            let n = 512 * 1024;
                                            let src = alloc::vec![0x5Au8; n];
                                            let mut dst = alloc::vec![0u8; n];
                                            let us = |t: u64| (t * 1_000_000 / freq) as u32;
                                            let t0 = cntpct();
                                            for _ in 0..8 {
                                                dst.copy_from_slice(&src);
                                                core::hint::black_box(&mut dst);
                                            }
                                            let t1 = cntpct();
                                            let h = blake3::hash(core::hint::black_box(&src));
                                            let t2 = cntpct();
                                            let sctlr: u64;
                                            unsafe { core::arch::asm!("mrs {}, sctlr_el2", out(reg) sctlr) };
                                            append_hex(&mut resp, b"BENCH_COPY_4MIB_US=", us(t1 - t0));
                                            append_hex(&mut resp, b"BENCH_BLAKE3_512KIB_US=", us(t2 - t1));
                                            append_hex(&mut resp, b"BENCH_HASH0=", h.as_bytes()[0] as u32);
                                            append_hex(&mut resp, b"SCTLR_EL2=", sctlr as u32);
                                            resp.extend_from_slice(b"END\n");
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"devinfo" {
                                            // The bootloader's A/B slot record: devinfo is UFS LUN 3 (sdd1), starting at 512-byte sector 48 = 4 KiB LBA 6. Only the 128-byte header goes out ("DEVINFO_xx=" one 32-bit word per line): the rest of the partition is factory identity data (serials, MACs, IMEIs) that has no business on the wire. Needs the disk up (any boot that opened the vault).
                                            if store.is_none() {
                                                resp.extend_from_slice(b"no disk on this boot\n");
                                            } else {
                                                let ufs = ferros_hal::ufs::UfsController::new(0x1320_0000);
                                                // A LUN's first command after power-on can report UNIT ATTENTION; it succeeds on retry.
                                                let mut ocs = 0xFF;
                                                for _ in 0..4 {
                                                    ocs = ufs.read_block_lun(3, 6);
                                                    if ocs == 0 && ufs.last_response_status() == 0 {
                                                        break;
                                                    }
                                                }
                                                append_hex(&mut resp, b"DEVINFO_OCS=", ocs as u32 | (ufs.last_response_status() as u32) << 8);
                                                let d = ufs.data_buffer();
                                                let hx = b"0123456789ABCDEF";
                                                for i in 0..32 {
                                                    let w = u32::from_le_bytes([d[i * 4], d[i * 4 + 1], d[i * 4 + 2], d[i * 4 + 3]]);
                                                    let key = [b'D', b'E', b'V', b'I', b'N', b'F', b'O', b'_', hx[i * 4 >> 4], hx[i * 4 & 15], b'='];
                                                    append_hex(&mut resp, &key, w);
                                                }
                                                // Fingerprint of everything past the header (rest of LBA 6 + all of LBA 7): tells whether a slot change touched anything else (a checksum, a mirror) without sending the identity data itself.
                                                let mut h = blake3::Hasher::new();
                                                h.update(&d[0x80..]);
                                                let mut ocs7 = ufs.read_block_lun(3, 7);
                                                if ocs7 != 0 || ufs.last_response_status() != 0 {
                                                    ocs7 = ufs.read_block_lun(3, 7);
                                                }
                                                h.update(ufs.data_buffer());
                                                let f = h.finalize();
                                                let fb = f.as_bytes();
                                                append_hex(&mut resp, b"DEVINFO_REST_OCS=", ocs7 as u32);
                                                append_hex(&mut resp, b"DEVINFO_REST_B3=", u32::from_be_bytes([fb[0], fb[1], fb[2], fb[3]]));
                                            }
                                            resp.extend_from_slice(b"END\n");
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"ufs" {
                                            match b.ufs_probe {
                                                Some(f) => resp = f(),
                                                None => resp.extend_from_slice(b"no UFS probe on this boot path (the handoff adopts Linux's live link)\n"),
                                            }
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        resp.extend_from_slice(match b.kind {
                                            BootKind::Handoff => b"ferros on Pixel 8 Pro (Tensor G3), chainloaded from Linux\n".as_slice(),
                                            BootKind::Cold => b"ferros on Pixel 8 Pro (Tensor G3), cold boot from ABL\n",
                                        });
                                        let r = |a: usize| unsafe { read_volatile(a as *const u32) };
                                        append_hex(&mut resp, b"GEN=", generation as u32);
                                        append_hex(&mut resp, b"UPTIME_MS=", (cntpct().wrapping_sub(t_boot) / (freq / 1000)) as u32);
                                        append_hex(&mut resp, b"EXC=", crate::exception_count().wrapping_sub(exc_start) as u32);
                                        append_hex(&mut resp, b"SNPSID=", r(DWC3 + 0xC120));
                                        append_hex(&mut resp, b"GSTS=", r(DWC3 + 0xC118));
                                        append_hex(&mut resp, b"DSTS=", r(DWC3 + 0xC70C));
                                        append_hex(&mut resp, b"USB_ATTEMPTS=", attempts);
                                        {
                                            use core::sync::atomic::Ordering::Relaxed;
                                            let k = &crate::killswitch::REPORT;
                                            append_hex(&mut resp, b"KILL_GICR_FRAME=", k[0].load(Relaxed));
                                            append_hex(&mut resp, b"KILL_GICD_CTLR=", k[1].load(Relaxed));
                                            append_hex(&mut resp, b"KILL_HCR_BEFORE=", k[2].load(Relaxed));
                                            append_hex(&mut resp, b"KILL_KEYS_IDLE=", k[3].load(Relaxed));
                                            let kohi = crate::wairua::peek();
                                            append_hex(&mut resp, b"KOHI_DONE=", kohi.is_some() as u32);
                                            for (name, h) in crate::kohi::SOURCES.iter().zip(kohi.map(|k| k.1).unwrap_or_default().iter()) {
                                                // present b0, rct_fail b1, apt_fail b2, level_fail b8..15, level-0 ones/2 b16..31
                                                let mut key = alloc::vec::Vec::from(&b"KOHI_"[..]);
                                                key.extend_from_slice(name.to_ascii_uppercase().as_bytes());
                                                key.push(b'=');
                                                append_hex(&mut resp, &key, h.packed());
                                                key.pop();
                                                key.extend_from_slice(b"_US=");
                                                append_hex(&mut resp, &key, h.us);
                                            }
                                            append_hex(&mut resp, b"KOHI_OK=", kohi.map(|k| k.0.is_some()).unwrap_or(false) as u32);
                                            append_hex(&mut resp, b"KOHI_MS=", crate::wairua::kohi_ms());
                                            append_hex(&mut resp, b"VHE=", crate::cachemap::vhe() as u32);
                                            append_hex(&mut resp, b"KILL_IRQS=", unsafe { read_volatile(&raw const crate::__irq_count) } as u32);
                                            let daif: u64;
                                            unsafe { core::arch::asm!("mrs {}, daif", out(reg) daif) };
                                            append_hex(&mut resp, b"DAIF=", daif as u32);
                                        }
                                        unsafe extern "C" {
                                            static _start: u8;
                                        }
                                        append_hex(&mut resp, b"BASE=", b.link_base as u32);
                                        append_hex(&mut resp, b"RUNNING_AT=", &raw const _start as usize as u32);
                                        append_hex(&mut resp, b"STAGE=", b.stage as u32);
                                        append_hex(&mut resp, b"BOOT_EL=", crate::boot_el() as u32);
                                        append_hex(&mut resp, b"HEAP_END=", heap_end as u32);
                                        if let Some(probe) = &b.probe {
                                            append_hex(&mut resp, b"UFS_LINK=", probe.link_up as u32);
                                            append_hex(&mut resp, b"VAULT_OPENED=", probe.opened as u32);
                                            append_hex(&mut resp, b"VAULT_VERIFIED=", probe.verified as u32);
                                            append_hex(&mut resp, b"VAULT_SEALED=", probe.sealed as u32);
                                            append_hex(&mut resp, b"VAULT_STAGE=", probe.stage as u32);
                                            for (i, w) in probe.root16.chunks(4).enumerate() {
                                                let label: &[u8] = [b"VAULT_ROOT0=".as_slice(), b"VAULT_ROOT1=", b"VAULT_ROOT2=", b"VAULT_ROOT3="][i];
                                                append_hex(&mut resp, label, u32::from_be_bytes([w[0], w[1], w[2], w[3]]));
                                            }
                                            append_hex(&mut resp, b"WAIRUA_OK=", crate::wairua::peek().map(|k| k.0.is_some()).unwrap_or(false) as u32);
                                            append_hex(&mut resp, b"IRA_FAIL=", probe.ira_fail);
                                        }
                                        if let Some(prev) = &b.prev {
                                            // Why the previous session ended (crumb byte G#Bx = exit reason, G#EExx = panic; a nonzero fault ELR = the fault sink fired).
                                            append_hex(&mut resp, b"PREV_EXIT=", prev[3] as u32);
                                            append_hex(&mut resp, b"PREV_EXC=", prev[5] as u32);
                                            append_hex(&mut resp, b"PREV_FAULT_ELR=", prev[20] as u32);
                                            append_hex(&mut resp, b"PREV_FAULT_FAR=", prev[21] as u32);
                                        }
                                        for (label, v) in &b.extras {
                                            append_hex(&mut resp, label, *v);
                                        }
                                        // Reboot mode as the bootloader will see it, read through the EL3 monitor (proves the fastboot path without rebooting; 0 = normal).
                                        append_hex(&mut resp, b"PMU_REBOOT_CMD=", platform::priv_reg(platform::PMU_REBOOT_CMD, false, 0) as u32);
                                        append_hex(&mut resp, b"DSS_REASON=", r(platform::DSS_HEADER + platform::DSS_OFFSET_EMERGENCY_REASON));
                                        append_hex(&mut resp, b"DSS_SCRATCH=", r(platform::DSS_HEADER + platform::DSS_OFFSET_SCRATCH));
                                        append_hex(&mut resp, b"S2MPU_HSI0_CTRL_LINUX=", dma.s2mpu_ctrl);
                                        append_hex(&mut resp, b"SYSMMU_NUM_VM=", dma.num_vm as u32);
                                        for vm in 0..dma.num_vm {
                                            append_hex(&mut resp, b"SYSMMU_VM_CTRL_LINUX=", dma.vm_ctrl_before[vm]);
                                            append_hex(&mut resp, b"SYSMMU_VM_CTRL_NOW=", dma.vm_ctrl_after[vm]);
                                        }
                                        const ATT_KEYS: [&[u8]; 24] = [b"  LTSTATE=", b"  LINKDBG=", b"  DSTS=", b"  RESETS=", b"  CONNDONES=", b"  SETUPS=", b"  SETUP_ARM_OK_FAIL=", b"  XFERNOTREADY=", b"  CMD_FAIL=", b"  EVENTS=", b"  LAST_SETUP=", b"  STATUS_IN_OUT=", b"  E0=", b"  E1=", b"  E2=", b"  E3=", b"  E4=", b"  E5=", b"  E6=", b"  E7=", b"  E8=", b"  E9=", b"  EA=", b"  EB="];
                                        for (i, h) in init_history.iter().enumerate() {
                                            append_hex(&mut resp, b"FAIL_ATT=", i as u32);
                                            for (k, &v) in ATT_KEYS.iter().zip(h.iter()) {
                                                append_hex(&mut resp, k, v);
                                            }
                                        }
                                        resp.extend_from_slice(b"END\n");
                                        queue_pt_response(&mut pt_out_data, &resp);
                                    } else if cmd.cap == cap_store {
                                        let resp = store_command(store.as_mut(), cmd.op, cmd.params);
                                        queue_pt_response(&mut pt_out_data, &resp);
                                    } else if cmd.cap == cap_display && cmd.op == ferros_pt::Op::Write {
                                        // A leading G#01 byte = hold (bridge `display --hold`).
                                        let (text, h) = match cmd.params.split_first() {
                                            Some((1, rest)) => (rest, true),
                                            _ => (cmd.params, false),
                                        };
                                        hold |= h;
                                        let rep = crate::display::show(text, freq);
                                        queue_pt_response(&mut pt_out_data, &rep);
                                    } else if cmd.cap == cap_mem && cmd.op == ferros_pt::Op::Write && cmd.params.len() >= 12 {
                                        // 32-bit poke: params [addr:8 BE][val:4 BE]. Address bit 63 set = through the EL3 monitor (SMC_CMD_PRIV_REG) for registers the S2MPU blocks from EL2. Response: [smc_ret:8 LE][readback:4 LE] (smc_ret 0 for a direct write).
                                        let raw = u64::from_be_bytes(cmd.params[0..8].try_into().unwrap());
                                        let val = u32::from_be_bytes(cmd.params[8..12].try_into().unwrap());
                                        let addr = raw & !(1 << 63);
                                        let (ret, back) = if raw >> 63 == 1 {
                                            let ret = platform::priv_reg(addr, true, val);
                                            (ret, platform::priv_reg(addr, false, 0) as u32)
                                        } else {
                                            unsafe {
                                                write_volatile(addr as usize as *mut u32, val);
                                                core::arch::asm!("dsb sy");
                                                (0, read_volatile(addr as usize as *const u32))
                                            }
                                        };
                                        let mut resp = [0u8; 12];
                                        resp[..8].copy_from_slice(&ret.to_le_bytes());
                                        resp[8..].copy_from_slice(&back.to_le_bytes());
                                        queue_pt_response(&mut pt_out_data, &resp);
                                    } else if cmd.cap == cap_mem && cmd.op == ferros_pt::Op::Read && cmd.params.len() >= 12 {
                                        // Physical memory / MMIO read: params [addr:8 BE][len:4 BE], up to 64 KiB per call. Aligned requests use 32-bit loads so MMIO windows read correctly; anything else is read bytewise. Address bit 63 set = 32-bit reads through the EL3 monitor. MMU is off, so every address is physical (a bad address takes the fault sink: report + reset to Android).
                                        let raw = u64::from_be_bytes(cmd.params[0..8].try_into().unwrap());
                                        let addr = (raw & !(1 << 63)) as usize;
                                        let len = (u32::from_be_bytes(cmd.params[8..12].try_into().unwrap()) as usize).min(64 << 10);
                                        let mut out = alloc::vec![0u8; len];
                                        if raw >> 63 == 1 {
                                            for (i, c) in out.chunks_mut(4).enumerate() {
                                                let v = platform::priv_reg((addr + i * 4) as u64, false, 0) as u32;
                                                c.copy_from_slice(&v.to_le_bytes()[..c.len()]);
                                            }
                                        } else if addr % 4 == 0 && len % 4 == 0 {
                                            for (i, c) in out.chunks_mut(4).enumerate() {
                                                c.copy_from_slice(&unsafe { read_volatile((addr + i * 4) as *const u32) }.to_le_bytes());
                                            }
                                        } else {
                                            for (i, b) in out.iter_mut().enumerate() {
                                                *b = unsafe { read_volatile((addr + i) as *const u8) };
                                            }
                                        }
                                        queue_pt_response(&mut pt_out_data, &out);
                                    } else if (cmd.cap == cap_reload || cmd.cap == cap_run) && cmd.op == ferros_pt::Op::Write {
                                        // Stage into STAGE, write-verify against the source. RELOAD additionally requires the ARM64 MZ header and must fit below the heap of the image it will replace.
                                        let params = cmd.params;
                                        let is_kernel = params.len() > 0x1000 && params[..4] == [0x4D, 0x5A, 0x00, 0x91];
                                        let fits = !params.is_empty() && params.len() <= STAGE_MAX && heap_end <= b.stage;
                                        let ok_shape = if cmd.cap == cap_reload { is_kernel && fits } else { fits };
                                        let mut ok = false;
                                        if ok_shape {
                                            let src_hash = blake3::hash(params);
                                            unsafe { core::ptr::copy_nonoverlapping(params.as_ptr(), b.stage as *mut u8, params.len()) };
                                            let staged = unsafe { core::slice::from_raw_parts(b.stage as *const u8, params.len()) };
                                            ok = blake3::hash(staged) == src_hash;
                                        }
                                        let size = if ok { params.len() } else { 0 };
                                        if cmd.cap == cap_reload {
                                            reload_ok = ok;
                                            reload_size = size;
                                            run_ok = false;
                                        } else {
                                            run_ok = ok;
                                            run_size = size;
                                            reload_ok = false;
                                        }
                                        queue_pt_response(&mut pt_out_data, &(size as u32).to_le_bytes());
                                    } else if cmd.cap == cap_reload && cmd.op == ferros_pt::Op::Exec {
                                        if reload_ok && reload_size > 0 {
                                            // Flush this Exec's COMPLETE before tearing USB down, give the host a moment, detach, then hand off to the copy stub.
                                            let mut spins = 0u32;
                                            while !usb.bulk_in_is_idle() && spins < (1 << 20) {
                                                let _ = usb.poll_event();
                                                spins += 1;
                                            }
                                            for _ in 0..(1 << 24) {
                                                core::hint::spin_loop();
                                            }
                                            platform::crumb(0xA6);
                                            unsafe {
                                                // The copy overwrites the vector table: no interrupt may land mid-copy. The new image re-arms the killswitch first thing.
                                                core::arch::asm!("msr daifset, #2");
                                                // Copy the stub (cacheable) first, then hand over with every dirty line written back and the MMU off: the stub and the next image run MMU-off, as from ABL.
                                                let dctl = read_volatile((DWC3 + 0xC704) as *const u32);
                                                write_volatile((DWC3 + 0xC704) as *mut u32, dctl & !(1 << 31));
                                                let stub = &raw const ferros_reload_stub as usize;
                                                let stub_len = &raw const ferros_reload_stub_end as usize - stub;
                                                core::ptr::copy_nonoverlapping(stub as *const u8, b.stub as *mut u8, stub_len);
                                                crate::cachemap::off_for_handover();
                                                let len = (reload_size + 7) & !7;
                                                let next_x0 = ((generation + 1) << 1) | 1;
                                                core::arch::asm!(
                                                    "dsb sy",
                                                    "ic iallu",
                                                    "dsb sy",
                                                    "isb",
                                                    "br {stub}",
                                                    stub = in(reg) b.stub,
                                                    in("x0") b.stage,
                                                    in("x1") b.link_base,
                                                    in("x2") len,
                                                    in("x3") next_x0,
                                                    options(noreturn)
                                                );
                                            }
                                        }
                                    } else if cmd.cap == cap_spawn && cmd.op == ferros_pt::Op::Exec {
                                        // Run the staged payload as a process (EL0, own arc, a console capability plus the requested endpoint grants); reply [exit][console]. Params: [name len][name][grant count][(kind 1 serve / 2 use, len, name)*][input].
                                        if run_ok && run_size > 0 {
                                            let code = unsafe { core::slice::from_raw_parts(b.stage as *const u8, run_size) };
                                            let pr = cmd.params;
                                            let mut at = 0usize;
                                            let mut take = |n: usize| { let s = &pr[at.min(pr.len())..(at + n).min(pr.len())]; at += n; s };
                                            let nl = take(1).first().copied().unwrap_or(0) as usize;
                                            let name = take(nl);
                                            let ng = take(1).first().copied().unwrap_or(0) as usize;
                                            let mut grants = alloc::vec::Vec::new();
                                            for _ in 0..ng {
                                                let kind = take(1).first().copied().unwrap_or(0);
                                                let l = take(1).first().copied().unwrap_or(0) as usize;
                                                let g = take(l);
                                                grants.push(if kind == 1 { crate::process::Grant::Serve(g) } else { crate::process::Grant::Use(g) });
                                            }
                                            let input = &pr[at.min(pr.len())..];
                                            // Signed capsules only: the gate (oracle.rs) must pass, or spawn never completes.
                                            match crate::process::spawn_capsule(if name.is_empty() { b"proc" } else { name }, code, input, &grants) {
                                                Ok(o) => {
                                                    use crate::checklist::{mark, Item};
                                                    mark(Item::Arcs);
                                                    mark(Item::Spawn);
                                                    if o.exit >> 56 != 0xFA {
                                                        mark(Item::El0);
                                                    }
                                                    if !o.console.is_empty() {
                                                        mark(Item::Syscall);
                                                        mark(Item::Caps);
                                                    }
                                                    if o.round_trips > 0 {
                                                        mark(Item::Ipc);
                                                    }
                                                    let mut resp: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(8 + o.console.len());
                                                    resp.extend_from_slice(&o.exit.to_le_bytes());
                                                    resp.extend_from_slice(&o.console);
                                                    queue_pt_response(&mut pt_out_data, &resp);
                                                }
                                                Err(e) => {
                                                    let mut resp = alloc::vec::Vec::from(&b"ERR:"[..]);
                                                    resp.extend_from_slice(e.as_bytes());
                                                    queue_pt_response(&mut pt_out_data, &resp);
                                                }
                                            }
                                        } else {
                                            queue_pt_response(&mut pt_out_data, b"ERR:NOPAYLOAD");
                                        }
                                    } else if cmd.cap == cap_run && cmd.op == ferros_pt::Op::Exec {
                                        // CALL the staged payload. ABI: extern "C" fn(in_ptr, in_len, out_ptr, out_cap) -> u64, entry at offset 0, PC-relative code only. A payload fault lands in the fault sink (report + reset to Android); a payload that never returns stops the watchdog pets.
                                        if run_ok && run_size > 0 {
                                            crate::cachemap::sync_code(b.stage, run_size);
                                            let mut out = alloc::vec![0u8; 4096];
                                            let entry: extern "C" fn(*const u8, usize, *mut u8, usize) -> u64 = unsafe { core::mem::transmute(b.stage) };
                                            let ret = entry(cmd.params.as_ptr(), cmd.params.len(), out.as_mut_ptr(), out.len());
                                            let n = (ret as usize).min(out.len());
                                            let mut resp: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(8 + n);
                                            resp.extend_from_slice(&ret.to_le_bytes());
                                            resp.extend_from_slice(&out[..n]);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                        } else {
                                            queue_pt_response(&mut pt_out_data, b"ERR:NOPAYLOAD");
                                        }
                                    } else if cmd.cap == cap_reboot && cmd.op == ferros_pt::Op::Exec {
                                        // Mode byte 1 = fastboot: stamp the PMU reboot mode through the EL3 monitor and reset straight into the bootloader. If the monitor refuses, fall back to a normal exit (report + Android).
                                        let fastboot = cmd.params.first().copied() == Some(1);
                                        let mut spins = 0u32;
                                        while !usb.bulk_in_is_idle() && spins < (1 << 20) {
                                            let _ = usb.poll_event();
                                            spins += 1;
                                        }
                                        if fastboot {
                                            report(&b, EXIT_FASTBOOT);
                                            unsafe {
                                                let dctl = read_volatile((DWC3 + 0xC704) as *const u32);
                                                write_volatile((DWC3 + 0xC704) as *mut u32, dctl & !(1 << 31));
                                            }
                                            // Only returns on failure: platform::WATCHDOG_DID_NOT_FIRE, or the EL3 monitor's refusal (nonzero, negative = error). Kept in report slot r[23] so the exit report says which.
                                            FASTBOOT_FAIL.store(platform::reboot_to_bootloader(), core::sync::atomic::Ordering::Relaxed);
                                        }
                                        leave(&b, EXIT_REBOOT, attempts, configured_ever, Some(&usb), &st);
                                    }
                                }
                            }
                        }
                    }
                    if done {
                        pt_inbound = None;
                        pt_seq_width = 0;
                    }
                } else if n > 0 && ferros_pt::is_control_packet(tmp[0]) {
                    // SPEC opens a transfer; reply SPEC ACK.
                    if let Some(spec) = packet::Spec::decode(&tmp[..n]) {
                        st.specs += 1;
                        t_pt = cntpct();
                        pt_seq_width = ferros_ledger::ewe::seq_width(spec.count);
                        pt_data_buf = alloc::vec![0u8; spec.total as usize];
                        pt_bitmap_buf = alloc::vec![0u64; ferros_pt::transfer::bitmap_words(spec.count)];
                        // SAFETY: pt_data_buf/pt_bitmap_buf outlive pt_inbound (both are replaced only when a new SPEC replaces the transfer).
                        let xfer = unsafe {
                            InboundTransfer::new(
                                &spec,
                                core::slice::from_raw_parts_mut(pt_data_buf.as_mut_ptr(), pt_data_buf.len()),
                                core::slice::from_raw_parts_mut(pt_bitmap_buf.as_mut_ptr(), pt_bitmap_buf.len()),
                            )
                        };
                        if let Some(xfer) = xfer {
                            let ack = packet::Ack { sid: spec.sid, seq: u64::MAX };
                            let mut ack_buf = [0u8; 64];
                            let ack_len = ack.encode(&mut ack_buf);
                            if ack_len > 0 {
                                usb.bulk_in_send(&ack_buf[..ack_len]);
                            }
                            pt_inbound = Some(xfer);
                        }
                    }
                }
                usb.bulk_out_arm();
            }
            UsbEvent::TransferNotReady { .. } => st.not_ready += 1,
        }
    }
}
