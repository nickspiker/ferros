//! Post-handoff USB service: ferros keeps the UFS link Linux brought up AND serves Photon Transport over the USB controller Linux was running, so the bridge's DIAG / RELOAD / RUN / REBOOT work with the disk live.
//!
//! Linux's reboot notifier (husky-kernel ferros-handoff.patch) keeps the USB glue, DWC3 core and PHY runtime-resumed before the jump, so every block here is clocked and powered.
//! What Linux leaves that ferros must undo: the DWC3 DMA goes through the USB SYSMMU (dwc3 has `iommus` in the DT) with Linux's page tables, so every VM context is disabled to put the master on physical addresses; the HSI0 S2MPU is bypassed like the cold path does.
//! Everything else is the cold-boot recipe: eUSB2 PHY init, DWC3 init with soft reset, the enumeration watchdog, and the same PT dispatch.
//!
//! Exits always land back in Android with a ramoops report (exit reason in the crumb byte): REBOOT from the host, 64 s without enumerating, 512 s without a PT command, or both volume buttons. A fault or a hang is caught by the fault sink or the watchdog. So the phone never needs hands.
//!
//! RELOAD: the image is linked at 0x92400000 with absolute addresses in its data, so it must run there. The new image is staged at STAGE, a tiny copy stub is placed at STUB, and the stub copies the image over the link base and jumps to it; x0 carries the generation (bit 0 = reloaded).

use core::ptr::{read_volatile, write_volatile};

use ferros_hal::usb::UsbEvent;
use ferros_pt::packet;
use ferros_pt::transfer::{InboundTransfer, OutboundTransfer};

use crate::shim::{self, VaultProbe};

const DWC3: usize = 0x1121_0000;
const USBCON: usize = 0x1110_0000;
/// HSI0 S2MPU in front of the USB DMA master.
const S2MPU_HSI0: usize = 0x1107_0000;
/// USB SYSMMU (samsung sysmmu v9). Per-VM control at 0x8000 + vm * 0x1000, bit 0 = translation enable; VM count in NUM_CONTEXT (0x100) bits 4:0.
const USB_SYSMMU: usize = 0x1104_0000;
const SYSMMU_CTRL_VM: usize = 0x8000;
const SYSMMU_VM_STRIDE: usize = 0x1000;
const SYSMMU_NUM_CONTEXT: usize = 0x100;

/// Link base of the shim image (aarch64-shim.ld) — every generation runs here.
const LINK_BASE: usize = 0x9240_0000;
/// Staging slot for RELOAD and RUN, inside the carveout past this generation's image + stack + 4 MiB heap.
const STAGE: usize = 0x92A0_0000;
const STAGE_MAX: usize = 4 << 20;
/// Where the reload copy stub runs from (past the staging slot, below the 0x92F00000 result scratch).
const STUB: usize = 0x92E8_0000;

/// Exit reasons (crumb byte in the ramoops report, kernel prints it as the low byte of `mbr_sig`).
const EXIT_REBOOT: u8 = 0xB0;
const EXIT_NO_ENUM: u8 = 0xB1;
const EXIT_IDLE: u8 = 0xB2;
const EXIT_FASTBOOT: u8 = 0xB3;
const EXIT_NO_USB: u8 = 0xB4;
const EXIT_BUTTONS: u8 = 0xB5;
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

/// Volume buttons (pinctrl@G#154D0000), active low. Both held = leave ferros.
const GPA4_DAT: usize = 0x154D_0084;
const GPA6_DAT: usize = 0x154D_00A4;
const VOL_DOWN_BIT: u32 = 1 << 1;
const VOL_UP_BIT: u32 = 1 << 2;

// Reload copy stub. Position-independent; runs from STUB after being copied there, so it can overwrite the image that called it.
// x0 = source (STAGE), x1 = destination (LINK_BASE), x2 = byte count (multiple of 8, nonzero), x3 = x0 for the new image.
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

fn cntpct() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) v) };
    v
}

fn cntfrq() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) v) };
    // Unprogrammed CNTFRQ would make every timeout zero; fall back to the Tensor G3 arch timer rate.
    if v == 0 { 24_576_000 } else { v }
}

/// Append "LABEL=<8 hex digits>\n".
fn append_hex(log: &mut alloc::vec::Vec<u8>, label: &[u8], val: u32) {
    log.extend_from_slice(label);
    let hex = b"0123456789ABCDEF";
    for i in (0..8).rev() {
        log.push(hex[((val >> (i * 4)) & 0xF) as usize]);
    }
    log.push(b'\n');
}

/// Frame a response as a PT outbound transfer: SPEC + DATA packets each padded to exactly 512 so the idle pump's 512-byte chunks align with packet boundaries (blast mode, no FIN — see the cold-boot loop).
fn queue_pt_response(out: &mut alloc::vec::Vec<u8>, payload: &[u8]) {
    out.clear();
    let mut ob_bitmap = alloc::vec![0u64; ferros_pt::transfer::outbound_bitmap_words(payload.len())];
    let mut spec_buf = [0u8; 512];
    if let Some((mut ob, spec_len)) = OutboundTransfer::start(ferros_pt::StreamId::FIRST, payload, &mut ob_bitmap, &mut spec_buf) {
        out.extend_from_slice(&spec_buf[..spec_len]);
        out.resize(512, 0);
        let mut pkt = [0u8; 512];
        while !ob.all_sent() {
            let plen = ob.next_data_packet(payload, &mut pkt);
            if plen == 0 {
                break;
            }
            let start = out.len();
            out.extend_from_slice(&pkt[..plen]);
            out.resize(start + 512, 0);
        }
    }
}

/// SYSMMU + S2MPU state as Linux left it, and what the disable read back. DIAG reports it.
struct DmaPath {
    s2mpu_ctrl: u32,
    num_vm: usize,
    vm_ctrl_before: [u32; 8],
    vm_ctrl_after: [u32; 8],
}

/// Put the DWC3 DMA master on physical addresses: bypass the HSI0 S2MPU, disable translation in every SYSMMU VM context.
fn open_usb_dma_path() -> DmaPath {
    let rd = |a: usize| unsafe { read_volatile(a as *const u32) };
    let wr = |a: usize, v: u32| unsafe { write_volatile(a as *mut u32, v) };
    let s2mpu_ctrl = rd(S2MPU_HSI0);
    wr(S2MPU_HSI0 + 0x54, 0xFF);
    wr(S2MPU_HSI0, 0);
    let num_vm = ((rd(USB_SYSMMU + SYSMMU_NUM_CONTEXT) & 0x1F) as usize).clamp(1, 8);
    let mut d = DmaPath { s2mpu_ctrl, num_vm, vm_ctrl_before: [0; 8], vm_ctrl_after: [0; 8] };
    for vm in 0..num_vm {
        let reg = USB_SYSMMU + SYSMMU_CTRL_VM + vm * SYSMMU_VM_STRIDE;
        d.vm_ctrl_before[vm] = rd(reg);
        wr(reg, d.vm_ctrl_before[vm] & !1);
        d.vm_ctrl_after[vm] = rd(reg);
    }
    unsafe { core::arch::asm!("dsb sy") };
    d
}

/// Report the vault + the USB session and warm-reset back to Android. The kernel dumps the whole 256-byte block as r[00..31].
/// USB-service layout: r[4] (0x20) bytes = init attempts, ever-configured, DSTS[7:0], DSTS[15:8], DSTS[23:16]; r[6] = bulk OUT completes | bulk IN completes << 32; r[7] = XferNotReady (HAL) | DEPCMD status failures << 32; r[8] = EP events seen | SETUPs << 32; r[9] = GEVNTCOUNT | GEVNTSIZ << 32; r[10] = last failed DEPCMD | its ep << 32 | its type << 40; r[11] = bulk_out_armed | bulk_out_ready << 8 | bulk_in_idle << 16 | configured << 24 | ep2 TRB ctrl snapshot << 32; r[12..17] = last 12 raw EP events (oldest-first order is evt_ring_n mod 12); r[18] = PT SPECs | DATA packets << 32; r[19] = DATA decoded | DATA accepted << 32; r[22] = commands | loop XferNotReady << 32.
fn leave(p: &VaultProbe, exit: u8, attempts: u32, configured_ever: bool, usb: Option<&ferros_hal::usb::Dwc3Dev>, st: &PtStats) -> ! {
    shim::report_vault(p, exit);
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
    unsafe {
        core::ptr::copy_nonoverlapping(usb_b.as_ptr(), (0xFD60_0000usize + 0x20) as *mut u8, 5);
        ferros_hal::mmio::cache_clean(0xFD60_0000, 0x100);
        // Graceful detach so the host sees a disconnect rather than a dead device.
        let dctl = read_volatile((DWC3 + 0xC704) as *const u32);
        write_volatile((DWC3 + 0xC704) as *mut u32, dctl & !(1 << 31));
    }
    shim::psci_reset()
}

/// `ferros.dev.store` on the ira-keyed vault (every set/remove is sealed by commit_root, so it survives a reset). Responses start with a status byte: 0 ok, 1 key not found, 2 vault error, 3 bad request, 4 no vault open.
/// Write [keylen:2 BE][key][value] -> [0][object hash:32]. Read [key] -> [0][value]; an empty key lists every key, '\n'-separated. Exec [key] -> remove -> [0] or [1].
fn store_command(store: Option<&mut shim::ShimStore>, op: ferros_pt::Op, params: &[u8]) -> alloc::vec::Vec<u8> {
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

/// The boot screen: generation, vault state + root, and how to get out.
fn status_text(generation: u64, p: &VaultProbe) -> alloc::vec::Vec<u8> {
    let mut t = alloc::vec::Vec::new();
    let hex = b"0123456789ABCDEF";
    t.extend_from_slice(b"ferros\n\nchainloaded from Linux, generation G#");
    for i in (0..4).rev() {
        t.push(hex[((generation >> (i * 4)) & 0xF) as usize]);
    }
    t.extend_from_slice(b"\n\nufs: ");
    t.extend_from_slice(if p.link_up { b"adopted, link up" } else { b"LINK DOWN" });
    t.extend_from_slice(b"\nvault: ");
    t.extend_from_slice(if p.verified == 1 { b"opened + verified".as_slice() } else if p.sealed == 1 { b"genesis sealed" } else if p.opened == 1 { b"opened, NOT verified" } else { b"NOT OPENED" });
    t.extend_from_slice(b"\nroot: ");
    for &b in &p.root16[..8] {
        t.push(hex[(b >> 4) as usize]);
        t.push(hex[(b & 0xF) as usize]);
    }
    t.extend_from_slice(b"\nwairua: ");
    t.extend_from_slice(if p.wairua_ok == 1 { b"live" } else { b"NO TRNG" });
    t.extend_from_slice(b"\n\nusb: serving Photon Transport\n     1209:4665\n\nhold both volume keys\nto return to Android\n");
    t
}

/// Shim entry with the USB service: adopt UFS, arm the safety nets, probe the vault, then serve PT over USB until told to leave.
pub fn entry_usb(x0: u64) -> ! {
    let link_up = shim::adopt_ufs();
    shim::arm_safety();
    let (probe, mut store) = shim::vault_probe(link_up);
    // Report now too: if the USB bring-up wedges, the watchdog's reset still shows the vault result and the crumb shows how far USB got.
    shim::report_vault(&probe, 0xA0);
    let generation = if x0 & 1 == 1 { x0 >> 1 } else { 0 };
    let t_boot = cntpct();
    let exc_start = crate::exception_count();

    // Boot screen: after a --live handoff DECON is still running, so show where this generation stands before USB even comes up (show() touches nothing when the DPU is powered down).
    let _ = crate::shim_display::show(&status_text(generation, &probe), cntfrq());

    let dma = open_usb_dma_path();
    shim::crumb(0xA1);
    crate::pixel8_eusb_phy_init();
    shim::crumb(0xA2);
    ferros_hal::usb::set_dwc3_base(DWC3);
    let mut usb = match ferros_hal::usb::Dwc3Dev::init() {
        Some(u) => u,
        None => leave(&probe, EXIT_NO_USB, 1, false, None, &PtStats::default()),
    };
    shim::crumb(0xA3);

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
    let mut init_history: alloc::vec::Vec<(u32, u32, u32)> = alloc::vec::Vec::new();
    let mut st = PtStats::default();
    // A PT transfer that stops moving for 16 s is a wedged endpoint: report it and leave rather than idling for 512 s.
    let stall_giveup = freq * 16;
    let mut t_pt = t_init;
    // `display --hold` keeps a message on the panel: the idle exit is off until both volume keys (or a host REBOOT) end the session.
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
        shim::pet_watchdogs();
        let gpa4 = unsafe { read_volatile(GPA4_DAT as *const u32) };
        let gpa6 = unsafe { read_volatile(GPA6_DAT as *const u32) };
        if (gpa4 & VOL_DOWN_BIT) == 0 && (gpa6 & VOL_UP_BIT) == 0 {
            leave(&probe, EXIT_BUTTONS, attempts, configured_ever, Some(&usb), &st);
        }
        let now = cntpct();
        if !configured_ever && now.wrapping_sub(t_boot) > enum_giveup {
            leave(&probe, EXIT_NO_ENUM, attempts, configured_ever, Some(&usb), &st);
        }
        if configured_ever && !hold && now.wrapping_sub(t_command) > idle_giveup {
            leave(&probe, EXIT_IDLE, attempts, configured_ever, Some(&usb), &st);
        }
        if pt_inbound.is_some() && now.wrapping_sub(t_pt) > stall_giveup {
            leave(&probe, EXIT_PT_STALL, attempts, configured_ever, Some(&usb), &st);
        }

        match usb.poll_event() {
            UsbEvent::None => {
                if !usb.is_configured() && (now.wrapping_sub(t_progress) > enum_timeout || now.wrapping_sub(t_init) > wedge_timeout) {
                    if init_history.len() < 32 {
                        unsafe {
                            init_history.push((
                                read_volatile((USBCON + 0x80) as *const u32),  // LTSTATE_HIS
                                read_volatile((USBCON + 0x84) as *const u32),  // LINK_DEBUG_L
                                read_volatile((DWC3 + 0xC70C) as *const u32), // DSTS
                            ));
                        }
                    }
                    crate::pixel8_eusb_phy_init();
                    if let Some(nu) = ferros_hal::usb::Dwc3Dev::init() {
                        usb = nu;
                    }
                    attempts += 1;
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
                t_progress = cntpct();
                usb.handle_reset();
                pt_inbound = None;
                pt_seq_width = 0;
                pt_out_data.clear();
            }
            UsbEvent::ConnectDone { .. } => {
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
                t_progress = cntpct();
                if !usb.handle_setup(&request) {
                    usb.ep0_stall();
                }
                if usb.is_configured() && !configured_ever {
                    configured_ever = true;
                    t_command = cntpct();
                    shim::crumb(0xA4);
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
                                        resp.extend_from_slice(b"ferros shim on Pixel 8 Pro (Tensor G3), chainloaded from Linux\nUSB: inherited DWC3, enumerated\n");
                                        let r = |a: usize| unsafe { read_volatile(a as *const u32) };
                                        append_hex(&mut resp, b"GEN=", generation as u32);
                                        append_hex(&mut resp, b"UPTIME_MS=", (cntpct().wrapping_sub(t_boot) / (freq / 1000)) as u32);
                                        append_hex(&mut resp, b"EXC=", crate::exception_count().wrapping_sub(exc_start) as u32);
                                        append_hex(&mut resp, b"SNPSID=", r(DWC3 + 0xC120));
                                        append_hex(&mut resp, b"GSTS=", r(DWC3 + 0xC118));
                                        append_hex(&mut resp, b"DSTS=", r(DWC3 + 0xC70C));
                                        append_hex(&mut resp, b"USB_ATTEMPTS=", attempts);
                                        append_hex(&mut resp, b"BASE=", LINK_BASE as u32);
                                        append_hex(&mut resp, b"STAGE=", STAGE as u32);
                                        append_hex(&mut resp, b"HEAP_END=", heap_end as u32);
                                        append_hex(&mut resp, b"UFS_LINK=", probe.link_up as u32);
                                        append_hex(&mut resp, b"VAULT_OPENED=", probe.opened as u32);
                                        append_hex(&mut resp, b"VAULT_VERIFIED=", probe.verified as u32);
                                        append_hex(&mut resp, b"VAULT_SEALED=", probe.sealed as u32);
                                        append_hex(&mut resp, b"VAULT_STAGE=", probe.stage as u32);
                                        for (i, w) in probe.root16.chunks(4).enumerate() {
                                            let label: &[u8] = [b"VAULT_ROOT0=".as_slice(), b"VAULT_ROOT1=", b"VAULT_ROOT2=", b"VAULT_ROOT3="][i];
                                            append_hex(&mut resp, label, u32::from_be_bytes([w[0], w[1], w[2], w[3]]));
                                        }
                                        append_hex(&mut resp, b"WAIRUA_OK=", probe.wairua_ok as u32);
                                        // Reboot mode as the bootloader will see it, read through the EL3 monitor (proves the fastboot path without rebooting; 0 = normal).
                                        append_hex(&mut resp, b"PMU_REBOOT_CMD=", shim::priv_reg(shim::PMU_REBOOT_CMD, false, 0) as u32);
                                        append_hex(&mut resp, b"DSS_REASON=", r(shim::DSS_HEADER + shim::DSS_OFFSET_EMERGENCY_REASON));
                                        append_hex(&mut resp, b"DSS_SCRATCH=", r(shim::DSS_HEADER + shim::DSS_OFFSET_SCRATCH));
                                        append_hex(&mut resp, b"S2MPU_HSI0_CTRL_LINUX=", dma.s2mpu_ctrl);
                                        append_hex(&mut resp, b"SYSMMU_NUM_VM=", dma.num_vm as u32);
                                        for vm in 0..dma.num_vm {
                                            append_hex(&mut resp, b"SYSMMU_VM_CTRL_LINUX=", dma.vm_ctrl_before[vm]);
                                            append_hex(&mut resp, b"SYSMMU_VM_CTRL_NOW=", dma.vm_ctrl_after[vm]);
                                        }
                                        for (i, &(lt, dbg, dsts)) in init_history.iter().enumerate() {
                                            append_hex(&mut resp, b"FAIL_ATT=", i as u32);
                                            append_hex(&mut resp, b"  LTSTATE=", lt);
                                            append_hex(&mut resp, b"  LINKDBG=", dbg);
                                            append_hex(&mut resp, b"  DSTS=", dsts);
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
                                        let rep = crate::shim_display::show(text, freq);
                                        queue_pt_response(&mut pt_out_data, &rep);
                                    } else if cmd.cap == cap_mem && cmd.op == ferros_pt::Op::Write && cmd.params.len() >= 12 {
                                        // 32-bit poke: params [addr:8 BE][val:4 BE]. Address bit 63 set = through the EL3 monitor (SMC_CMD_PRIV_REG) for registers the S2MPU blocks from EL2. Response: [smc_ret:8 LE][readback:4 LE] (smc_ret 0 for a direct write).
                                        let raw = u64::from_be_bytes(cmd.params[0..8].try_into().unwrap());
                                        let val = u32::from_be_bytes(cmd.params[8..12].try_into().unwrap());
                                        let addr = raw & !(1 << 63);
                                        let (ret, back) = if raw >> 63 == 1 {
                                            let ret = shim::priv_reg(addr, true, val);
                                            (ret, shim::priv_reg(addr, false, 0) as u32)
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
                                                let v = shim::priv_reg((addr + i * 4) as u64, false, 0) as u32;
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
                                        let fits = !params.is_empty() && params.len() <= STAGE_MAX && heap_end <= STAGE;
                                        let ok_shape = if cmd.cap == cap_reload { is_kernel && fits } else { fits };
                                        let mut ok = false;
                                        if ok_shape {
                                            let src_hash = blake3::hash(params);
                                            unsafe { core::ptr::copy_nonoverlapping(params.as_ptr(), STAGE as *mut u8, params.len()) };
                                            let staged = unsafe { core::slice::from_raw_parts(STAGE as *const u8, params.len()) };
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
                                            shim::crumb(0xA6);
                                            unsafe {
                                                let dctl = read_volatile((DWC3 + 0xC704) as *const u32);
                                                write_volatile((DWC3 + 0xC704) as *mut u32, dctl & !(1 << 31));
                                                let stub = &raw const ferros_reload_stub as usize;
                                                let stub_len = &raw const ferros_reload_stub_end as usize - stub;
                                                core::ptr::copy_nonoverlapping(stub as *const u8, STUB as *mut u8, stub_len);
                                                let len = (reload_size + 7) & !7;
                                                let next_x0 = ((generation + 1) << 1) | 1;
                                                core::arch::asm!(
                                                    "dsb sy",
                                                    "ic iallu",
                                                    "dsb sy",
                                                    "isb",
                                                    "br {stub}",
                                                    stub = in(reg) STUB,
                                                    in("x0") STAGE,
                                                    in("x1") LINK_BASE,
                                                    in("x2") len,
                                                    in("x3") next_x0,
                                                    options(noreturn)
                                                );
                                            }
                                        }
                                    } else if cmd.cap == cap_run && cmd.op == ferros_pt::Op::Exec {
                                        // CALL the staged payload. ABI: extern "C" fn(in_ptr, in_len, out_ptr, out_cap) -> u64, entry at offset 0, PC-relative code only. A payload fault lands in the fault sink (report + reset to Android); a payload that never returns stops the watchdog pets.
                                        if run_ok && run_size > 0 {
                                            unsafe { core::arch::asm!("dsb sy", "ic iallu", "dsb sy", "isb") };
                                            let mut out = alloc::vec![0u8; 4096];
                                            let entry: extern "C" fn(*const u8, usize, *mut u8, usize) -> u64 = unsafe { core::mem::transmute(STAGE) };
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
                                            shim::report_vault(&probe, EXIT_FASTBOOT);
                                            unsafe {
                                                let dctl = read_volatile((DWC3 + 0xC704) as *const u32);
                                                write_volatile((DWC3 + 0xC704) as *mut u32, dctl & !(1 << 31));
                                            }
                                            let _ = shim::reboot_to_bootloader();
                                        }
                                        leave(&probe, EXIT_REBOOT, attempts, configured_ever, Some(&usb), &st);
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
