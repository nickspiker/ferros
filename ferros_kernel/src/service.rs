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
    /// The last command parsed: cap bytes 0..3 (LE) | op << 56 — so a stall exit says what the host was doing (DIAG PREV_PT_CMD).
    last_cmd: u64,
    /// The transfer open at a stall exit: stream id, chunks received, expected bytes, and how long since its last packet (DIAG PREV_PT_*).
    open_sid: u32,
    open_recv: u32,
    open_total: u32,
    pt_age_ms: u32,
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
    w(24, st.last_cmd);
    w(25, st.open_sid as u64 | (st.open_recv as u64) << 32);
    w(26, st.open_total as u64 | (st.pt_age_ms as u64) << 32);
    w(27, crate::process::alive() as u64);
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
            let len = params.len() - 2 - klen;
            crate::flashlog::line(&alloc::format!("store set {key:?} {len} bytes"));
            match store.set(&key, params[2 + klen..].to_vec()) {
                Ok(h) => {
                    // A preference set from the bridge applies at once, as one set from the device does (SYS_SET_PREF): every live process sees it on its next frame.
                    if key == "device.base" && len == 1 && tatau::Base::from_code(params[2 + klen]).is_some() {
                        crate::process::set_pref(crate::process::PREF_BASE, params[2 + klen]);
                    }
                    if key == "device.labels" && len == 1 && params[2 + klen] <= 1 {
                        crate::process::set_pref(crate::process::PREF_LABELS, params[2 + klen]);
                    }
                    crate::flashlog::line(&alloc::format!("store set {key:?} ok: {}", store.describe()));
                    let mut r = alloc::vec![0u8];
                    r.extend_from_slice(&h);
                    r
                }
                Err(e) => {
                    crate::flashlog::line(&alloc::format!("store set {key:?} FAILED: {e:?}"));
                    // Status 2 followed by the vault's own error text, so the bridge can show it.
                    let mut r = alloc::vec![2u8];
                    r.extend_from_slice(alloc::format!("{e:?}").as_bytes());
                    r
                }
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
                Err(e) => {
                    crate::flashlog::line(&alloc::format!("store get {key:?} FAILED: {e:?}"));
                    let mut r = alloc::vec![2u8];
                    r.extend_from_slice(alloc::format!("{e:?}").as_bytes());
                    r
                }
            }
        }
        ferros_pt::Op::Exec => match key_of(params) {
            Some(k) if !k.is_empty() => {
                let r = store.remove_key(&k);
                crate::flashlog::line(&alloc::format!("store remove {k:?}: {r:?}"));
                match r {
                    Ok(true) => alloc::vec![0],
                    Ok(false) => alloc::vec![1],
                    Err(_) => alloc::vec![2],
                }
            }
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
/// The service loop's vault, lent to `SYS_SPAWN` (process.rs `CAPSULES`) for reading capsules by key: `run` never returns, so the pointer stays good for the rest of the boot.
static mut SPAWN_STORE: *mut Option<ShimStore> = core::ptr::null_mut();

/// `SYS_SET_PREF`'s persistence: a preference into the device vault under its key (SETTINGS.md).
fn store_pref(index: usize, value: u8) -> bool {
    let key = match index {
        crate::process::PREF_BASE => "device.base",
        crate::process::PREF_LABELS => "device.labels",
        _ => return false,
    };
    unsafe { (*(&raw mut SPAWN_STORE)).as_mut().and_then(|s| s.as_mut()).map(|s| s.set(key, alloc::vec![value]).is_ok()).unwrap_or(false) }
}

/// `SYS_STORE`'s vault: the kernel has already checked the capability and prefixed the key with its scope.
fn store_scoped(key: &str, value: Option<&[u8]>) -> Result<Option<alloc::vec::Vec<u8>>, ()> {
    let st = unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()).ok_or(())?;
    match value {
        Some(v) => {
            crate::flashlog::line(&alloc::format!("store set {key:?} {} bytes (process)", v.len()));
            st.set(key, v.to_vec()).map(|_| None).map_err(|_| ())
        }
        None => st.get_by_key(key).map_err(|_| ()),
    }
}

fn read_capsule(key: &[u8]) -> Option<alloc::vec::Vec<u8>> {
    let k = core::str::from_utf8(key).ok()?;
    unsafe { (*(&raw mut SPAWN_STORE)).as_mut()?.as_mut()?.get_by_key(k).ok().flatten() }
}

/// What init holds: everything the shell's three processes need, in the slot order payloads/init expects (its header). The kernel grants it once; init hands each process its share.
fn home_grants() -> alloc::vec::Vec<crate::process::Grant<'static>> {
    use crate::process::Grant::*;
    alloc::vec![
        Serve(b"display"),
        Hardware(b"decon0-main"), Hardware(b"decon0-win"), Hardware(b"decon0-wincon"), Hardware(b"dpu-dma0"), Hardware(b"dpp0"), Hardware(b"gpio-gpp0"),
        Dma(b"display-fb"), Dma(b"display-heap"),
        Hardware(b"dsim0"), Hardware(b"dphy0"), Hardware(b"sramc0"), Hardware(b"hdrcomm0"),
        Serve(b"touch"), Hardware(b"usi2-spi"), Hardware(b"gpio-gpn0"),
        Use(b"display"), Use(b"touch"),
        Settings,
        Store(b"map"),
        Dma(b"display-phases"),
    ]
}

/// The ring has completed: bring the touch controller up (the kernel's part, as `diag touch up` does) and start the vault's `init`, which starts the shell. Its console goes to `diag console`.
/// Returns false when the shell is off this boot (`home.off`).
fn start_home(store: &mut Option<ShimStore>) -> bool {
    crate::checklist::HOME.store(cntpct(), core::sync::atomic::Ordering::Relaxed);
    // `home.off` in the vault (any value): no shell this boot, so the panel is the bridge's alone — a measurement session (tools/display-profile.sh) must not have the map redraw because an instrument rests on the glass. Removed with `store rm home.off`.
    if store.as_mut().and_then(|s| s.get_by_key("home.off").ok().flatten()).is_some() {
        crate::process::push_console(b"home: off (vault home.off): no shell this boot\n");
        return false;
    }
    let Some(capsule) = store.as_mut().and_then(|s| s.get_by_key("init").ok().flatten()) else {
        crate::process::push_console(b"home: no init capsule in the vault\n");
        return true;
    };
    let sealed = store.as_mut().and_then(|s| s.get_by_key("sealed").ok().flatten()).unwrap_or_default();
    let mut rep = alloc::vec::Vec::new();
    // The shell draws in 10 bits (DISPLAY-CALIB.md, 2026-10-22): the 10 bpc blend with the DQE dither onto the 8 bpc link, the mode measured to carry a 10-bit layer's every code. ABL leaves the 8 bpc path; the full modeset is the only proven way across, so it runs once here, between the boot ring and the shell's first frame.
    crate::dpu::modeset_with(false, 0, 2, 0, true, cntfrq(), &mut rep);
    crate::checklist::at("home: 10 bpc blend");
    // The DQE's dither in shift mode (EN | MODE = plain truncation of the two low bits): the display server dithers to 8-bit codes itself, from its 14-bit table, with the low two bits of each 10-bit code zero, so the panel gets exactly that pattern (12 bits effective; owner, 2026-10-22). A second ordered dither on top of ours, the DQE's own, partly cancelled it into visible steps. Applied with the DQE's shadow at the next present.
    unsafe {
        core::ptr::write_volatile((0x194D_0000usize + 0x2800) as *mut u32, 3);
        let shd = 0x1947_0050usize;
        core::ptr::write_volatile(shd as *mut u32, core::ptr::read_volatile(shd as *const u32) | 1 << 24);
    }
    if !rep.is_empty() {
        crate::process::push_console(b"home: modeset: ");
        crate::process::push_console(&rep);
        rep.clear();
    }
    crate::touch::up(&mut rep);
    crate::checklist::at("home: touch up");
    match crate::process::spawn_capsule(b"init", &capsule, b"", &home_grants(), &sealed) {
        Ok(o) => crate::process::push_console(&o.console),
        Err(e) => {
            crate::process::push_console(b"home: init refused: ");
            crate::process::push_console(e.as_bytes());
            crate::process::push_console(b"\n");
        }
    }
    crate::checklist::at("home: init ran");
    true
}

/// The USB link as of the service loop's last pass, for the facts.
static USB_LINK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The last battery sample and when it was taken: the gauge is read at most every two seconds, however often a dashboard asks.
static mut GAUGE: ([u64; 4], u64) = ([crate::process::FACT_UNKNOWN; 4], 0);

/// SYS_FACTS (process.rs): 0 press to home µs; 1 battery %; 2 battery temperature, hundredths of a degree C (i64); 3 cell voltage µV; 4 battery current µA, positive = charging (i64); 5 USB link up (0/1); 6 processes alive; 7 keys in the vault; the rest unknown.
fn facts() -> [u64; crate::process::FACT_COUNT] {
    use core::sync::atomic::Ordering::Relaxed;
    let mut v = [crate::process::FACT_UNKNOWN; crate::process::FACT_COUNT];
    let f = cntfrq();
    let (press, home) = (crate::checklist::PRESS.load(Relaxed), crate::checklist::HOME.load(Relaxed));
    if press != 0 && home > press {
        v[0] = (home - press) * 1_000_000 / f;
    }
    let now = cntpct();
    unsafe {
        let g = &mut *(&raw mut GAUGE);
        if g.1 == 0 || now.wrapping_sub(g.1) > 2 * f {
            g.1 = now;
            g.0 = match crate::hsi2c::gauge::sample() {
                Ok(s) => [s.soc as u64, s.temp_cc as i64 as u64, s.vcell_uv as u64, s.current_ua as i64 as u64],
                Err(_) => [crate::process::FACT_UNKNOWN; 4],
            };
        }
        v[1..5].copy_from_slice(&g.0);
    }
    v[5] = USB_LINK.load(Relaxed) as u64;
    v[6] = crate::process::alive() as u64;
    if let Some(keys) = unsafe { (*(&raw mut SPAWN_STORE)).as_mut() }.and_then(|s| s.as_mut()).and_then(|s| s.keys().ok()) {
        v[7] = keys.len() as u64;
    }
    v
}

pub fn run(mut b: Boot) -> ! {
    // Report now too: if the USB bring-up wedges, the watchdog's reset still shows the vault result (handoff) and the crumb shows how far USB got.
    report(&b, 0xA0);
    let generation = if b.x0 & 1 == 1 { b.x0 >> 1 } else { 0 };
    let mut store = b.store.take();
    unsafe {
        SPAWN_STORE = &raw mut store;
        crate::process::CAPSULES = Some(read_capsule);
        crate::process::PREF_STORE = Some(store_pref);
        crate::process::FACTS = Some(facts);
        crate::process::STORE = Some(store_scoped);
    }
    let mut home_started = !matches!(b.kind, BootKind::Cold);
    let t_boot = cntpct();
    let exc_start = crate::exception_count();

    // Boot screen: after a --live handoff DECON is still running, so show where this generation stands before USB even comes up (show() touches nothing when the DPU is powered down).
    if !crate::checklist::active() {
        let _ = crate::display::show(&status_text(b.kind, generation, b.probe.as_ref()), cntfrq());
    }

    let dma = open_usb_dma_path();
    platform::crumb(0xA1);
    crate::checklist::at("usb: dma path open");
    crate::pixel8_eusb_phy_init();
    platform::crumb(0xA2);
    crate::checklist::at("usb: phy up");
    ferros_hal::usb::set_dwc3_base(DWC3);
    let mut usb = match ferros_hal::usb::Dwc3Dev::init() {
        Some(u) => u,
        None => leave(&b, EXIT_NO_USB, 1, false, None, &PtStats::default()),
    };
    platform::crumb(0xA3);
    crate::checklist::at("usb: controller up");

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
    // A finished transfer is closed at the top of the next pass, not at the end of the arm that finished it: the DIAG handlers leave their arm with `continue`, which skipped the close and left every finished `diag` transfer "open" — then a 16 s pause tripped the stall exit (every PT-stall exit of 2026-10-16 followed a `diag` with a pause; the record named it).
    let mut pt_done = false;
    let mut pt_out_data: alloc::vec::Vec<u8> = alloc::vec::Vec::new();

    let cap_diag = ferros_pt::command::dev_cap(ferros_pt::command::caps::DIAG);
    let cap_reload = ferros_pt::command::dev_cap(ferros_pt::command::caps::RELOAD);
    let cap_reboot = ferros_pt::command::dev_cap(ferros_pt::command::caps::REBOOT);
    let cap_run = ferros_pt::command::dev_cap(ferros_pt::command::caps::RUN);
    let cap_spawn = ferros_pt::command::dev_cap(ferros_pt::command::caps::SPAWN);
    let cap_seal = ferros_pt::command::dev_cap(ferros_pt::command::caps::SEAL);
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

    // The generic timer's event stream: an event on every 0 -> 1 transition of counter bit 12, every 8192 ticks — 333 us at 24.576 MHz. A core in `wfe` wakes on it without taking an interrupt, so the idle loop below can sleep instead of spinning and still answer USB well inside anything the host would notice.
    // The period sets the split. Each wake runs one full loop pass, about 13 us of work, so bit 9 (41.7 us) left the core awake a third of the time — 68% asleep, measured — while bit 12 leaves it awake about 4%. Only the first packet of a new command can wait up to one period; a transfer in progress never sleeps.
    // Without it the loop pinned one core at full speed for as long as ferros was up, and with nothing else running that core was most of the phone's heat (owner, 2026-10-21: "why's it so hot? 1 core, always on?").
    unsafe {
        let mut v: u64;
        core::arch::asm!("mrs {}, cnthctl_el2", out(reg) v);
        v = (v & !(0xF << 4 | 1 << 3)) | 1 << 2 | 12 << 4;
        core::arch::asm!("msr cnthctl_el2, {}", "isb", in(reg) v);
    }
    // Ticks spent asleep in the idle loop, and since when, for DIAG's IDLE_PCT.
    let mut asleep: u64 = 0;
    let t_awake_from = cntpct();
    // The gate's own press is spent; from here a press is the screen's (below).
    unsafe { core::ptr::write_volatile(&raw mut crate::__power_pressed, 0) };

    loop {
        platform::pet_watchdogs();
        // The killswitch needs IRQs unmasked whenever the kernel isn't deliberately masking them (only the RELOAD handover does): re-assert it every pass, so no path back from a process or a fault can leave it disarmed.
        unsafe { core::arch::asm!("msr daifclr, #2") };
        // The power key turns the screen off and on (owner, 2026-10-22): a preference every process sees at once (process.rs PREF_SCREEN_OFF); the display server darkens the panel. The wake policy (proximity) comes later.
        if unsafe { core::ptr::read_volatile(&raw const crate::__power_pressed) } != 0 {
            unsafe { core::ptr::write_volatile(&raw mut crate::__power_pressed, 0) };
            let off = crate::process::pref(crate::process::PREF_SCREEN_OFF) ^ 1;
            crate::process::set_pref(crate::process::PREF_SCREEN_OFF, off);
        }
        USB_LINK.store(usb.is_configured(), core::sync::atomic::Ordering::Relaxed);
        if pt_done {
            pt_done = false;
            pt_inbound = None;
            pt_seq_width = 0;
        }
        let now = cntpct();
        // Home: once the ring has completed — the host has the USB link, or a second has passed since the press with no cable — start init and with it the shell.
        if !home_started {
            let press = crate::checklist::PRESS.load(core::sync::atomic::Ordering::Relaxed);
            if configured_ever || (press != 0 && now.wrapping_sub(press) > cntfrq()) {
                home_started = true;
                // A session with the shell off (`home.off`) is someone measuring: no idle exit while they position an instrument.
                hold |= !start_home(&mut store);
            }
        }
        // No host after 64 s is a dead USB attempt only while nothing else is running: with the shell up, a phone without a cable is the normal case.
        if !configured_ever && crate::process::alive() == 0 && now.wrapping_sub(t_boot) > enum_giveup {
            leave(&b, EXIT_NO_ENUM, attempts, configured_ever, Some(&usb), &st);
        }
        // The idle exit is a dev safety net for a kernel nobody is talking to; a live process (the display server, a driver) is the session itself, so it holds the kernel up like `display --hold` does. The killswitch and `bridge reboot` still end it.
        if configured_ever && !hold && crate::process::alive() == 0 && now.wrapping_sub(t_command) > idle_giveup {
            leave(&b, EXIT_IDLE, attempts, configured_ever, Some(&usb), &st);
        }
        if pt_inbound.is_some() && now.wrapping_sub(t_pt) > stall_giveup {
            if let Some(x) = &pt_inbound {
                st.open_sid = x.sid.0 as u32;
                st.open_recv = x.chunks_received as u32;
                st.open_total = x.expected_total as u32;
                st.pt_age_ms = (now.wrapping_sub(t_pt) * 1000 / freq) as u32;
            }
            leave(&b, EXIT_PT_STALL, attempts, configured_ever, Some(&usb), &st);
        }

        match usb.poll_event() {
            UsbEvent::None => {
                // Idle: give one runnable process a time slice (it comes back at the latest when the slice timer fires) — but not while a PT transfer is open or a response is still going out: a 10 ms slice per idle pass crawls a transfer to the point where the host gives up, and the half-open transfer then trips the 16 s stall exit (measured 2026-10-16: every PT-stall exit of the day had a busy process running).
                if pt_inbound.is_none() && pt_out_data.is_empty() {
                    crate::process::run_slice();
                }
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
                // Truly idle — enumerated, no transfer half-received, nothing waiting to go out, no process to run — sleep until the next timer event. Anything else keeps the old short spin, so transfers and a live display server behave exactly as before.
                if usb.is_configured() && pt_inbound.is_none() && pt_out_data.is_empty() && crate::process::alive() == 0 {
                    let t = cntpct();
                    unsafe { core::arch::asm!("wfe") };
                    asleep += cntpct().wrapping_sub(t);
                } else {
                    for _ in 0..64u32 {
                        core::hint::spin_loop();
                    }
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
                    crate::checklist::at("usb: configured by host");
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
                                pt_done = true;
                                if let Some(cmd) = ferros_pt::command::parse(xfer.payload()) {
                                    st.last_cmd = u32::from_le_bytes([cmd.cap[0], cmd.cap[1], cmd.cap[2], cmd.cap[3]]) as u64 | (cmd.op as u64) << 56;
                                    if cmd.cap == cap_diag && cmd.op == ferros_pt::Op::Read {
                                        let mut resp: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
                                        if cmd.params.starts_with(b"lun ") {
                                            // Hex dump of UFS blocks: `diag lun <lun> <first lba> <count>`, non-zero 16-byte rows only (LU3: devinfo LBA 6, pinfo 8, blenv 10; LU0 partitions at the LBAs `diag ufsdesc` prints) — for finding where the bootloader keeps the active slot besides devinfo (2026-10-17). devinfo carries the device's identity (serials, IMEIs, MACs): don't paste it anywhere.
                                            let mut it = cmd.params[4..].split(|&c| c == b' ').filter_map(|t| core::str::from_utf8(t).ok()?.parse::<u32>().ok());
                                            let (lun, first, count) = (it.next().unwrap_or(3).min(7) as u8, it.next().unwrap_or(6), it.next().unwrap_or(1).min(8));
                                            let ufs = ferros_hal::ufs::UfsController::new(0x1320_0000);
                                            let hex = b"0123456789ABCDEF";
                                            for lba in first..first + count {
                                                let mut ok = false;
                                                for _ in 0..4 {
                                                    if ufs.read_block_lun(lun, lba) == 0 && ufs.last_response_status() == 0 {
                                                        ok = true;
                                                        break;
                                                    }
                                                }
                                                resp.extend_from_slice(alloc::format!("LBA {lba}{}\n", if ok { "" } else { " unreadable" }).as_bytes());
                                                if !ok {
                                                    continue;
                                                }
                                                let b = ufs.data_buffer();
                                                for (i, row) in b.chunks(16).enumerate() {
                                                    if row.iter().any(|&x| x != 0) {
                                                        resp.extend_from_slice(alloc::format!("  {:04X}:", i * 16).as_bytes());
                                                        for &x in row {
                                                            resp.push(b' ');
                                                            resp.push(hex[(x >> 4) as usize]);
                                                            resp.push(hex[(x & 15) as usize]);
                                                        }
                                                        resp.push(b'\n');
                                                    }
                                                }
                                            }
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params.starts_with(b"bootlun ") {
                                            // `diag bootlun <1|2>`: write the UFS device's bBootLunEn attribute (1 = Boot LU A, 2 = Boot LU B) and read it back. The ROM boots the bootloader chain from that LU, and the bootloader's "boot slot" follows the LU's letter before devinfo is read (its log: "ufshc: bBootLunEn 0x2" then "boot slot: b"; devinfo's active flag gets re-synced to it). Only 1 or 2 are accepted: 0 disables boot.
                                            let ufs = ferros_hal::ufs::UfsController::new(0x1320_0000);
                                            let before = ufs.read_attribute(0x00, 0);
                                            let want = cmd.params[8..].first().map(|&c| c.wrapping_sub(b'0') as u32);
                                            let line = match want {
                                                Some(v @ 1..=2) => {
                                                    let ok = ufs.write_attribute(0x00, 0, v);
                                                    alloc::format!("bBootLunEn before {before:?}, write {v}: {}, after {:?}\n", if ok { "accepted" } else { "REFUSED" }, ufs.read_attribute(0x00, 0))
                                                }
                                                _ => alloc::format!("bBootLunEn {before:?}; only 1 or 2 may be written\n"),
                                            };
                                            resp.extend_from_slice(line.as_bytes());
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        // The handoff build has no `slot`: marking an A/B slot is a cold-boot job, and a ferros chainloaded out of a running Android must not rewrite devinfo under it.
                                        #[cfg(not(shim_handoff))]
                                        if cmd.params == b"android" {
                                            // The gate's Android path from the host, for bisecting it: slot a active in devinfo, then the plain PSCI reset. The bootloader boots the active slot.
                                            let r = crate::slot::set_active(0);
                                            // Tell slot a's boot gate (ferros_hv/src/gate.rs) to answer "Android" this once instead of auto-choosing ferros; cleaned to DRAM, since the gate reads it with the caches off.
                                            unsafe {
                                                let p = 0xFD60_0FF8usize as *mut u64;
                                                core::ptr::write_volatile(p, 0x414E_4452_4F49_4431);
                                                core::arch::asm!("dc civac, {a}", "dsb sy", a = in(reg) p as u64);
                                            }
                                            resp.extend_from_slice(alloc::format!("set_active(a) result {r}; {}\n", if r == crate::slot::MARKED { "resetting" } else { "not resetting" }).as_bytes());
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            if r == crate::slot::MARKED {
                                                let t0 = cntpct();
                                                while cntpct().wrapping_sub(t0) < freq / 2 {
                                                    if !pt_out_data.is_empty() && usb.bulk_in_is_idle() {
                                                        let len = pt_out_data.len().min(512);
                                                        let chunk: alloc::vec::Vec<u8> = pt_out_data.drain(..len).collect();
                                                        usb.bulk_in_send(&chunk);
                                                    }
                                                    let _ = usb.poll_event();
                                                }
                                                platform::crumb(0xC7);
                                                platform::reboot();
                                            }
                                            continue;
                                        }
                                        if cmd.params == b"devinfo" {
                                            // The bootloader's slot record as it stands (slot.rs): both words, retries byte and flags byte (b0 unbootable, b1 successful, b2 active, b3 fastboot-ok).
                                            let ufs = ferros_hal::ufs::UfsController::new(0x1320_0000);
                                            let mut ok = false;
                                            for _ in 0..4 {
                                                if ufs.read_block_lun(3, 6) == 0 && ufs.last_response_status() == 0 {
                                                    ok = true;
                                                    break;
                                                }
                                            }
                                            if ok {
                                                let b = ufs.data_buffer();
                                                let w = |s: usize| u32::from_le_bytes([b[0x30 + 4 * s], b[0x31 + 4 * s], b[0x32 + 4 * s], b[0x33 + 4 * s]]);
                                                resp.extend_from_slice(alloc::format!("DEVINFO magic {}{}{}{} major {}: slot a word G#{:08X} (retries {}, flags G#{:02X}), slot b word G#{:08X} (retries {}, flags G#{:02X})\n", b[0] as char, b[1] as char, b[2] as char, b[3] as char, u16::from_le_bytes([b[4], b[5]]), w(0), w(0) & 0xFF, (w(0) >> 8) & 0xFF, w(1), w(1) & 0xFF, (w(1) >> 8) & 0xFF).as_bytes());
                                            } else {
                                                resp.extend_from_slice(b"DEVINFO unreadable\n");
                                            }
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"ufsdesc" {
                                            // The UFS device's provisioning, read from the device itself: the Device descriptor (LU count, feature bits), every Unit descriptor (enabled, boot LUN id, WRITE PROTECT TYPE: 0 none / 1 power-on / 2 permanent, block size, block count), and each enabled LU's GPT partition names — so the boot-chain write-protect question (BOOT.md, 2026-10-17) is answered by the flash, not by guessing.
                                            let ufs = ferros_hal::ufs::UfsController::new(0x1320_0000);
                                            let (d, n, ocs) = ufs.read_descriptor(0x00, 0);
                                            resp.extend_from_slice(alloc::format!("DEVICE_DESC len {n} ocs {ocs}: numLU {} bootEnable {} descrAccessEn {} features G#{:02X}{:02X}\n", d.get(0x08).copied().unwrap_or(0), d.get(0x05).copied().unwrap_or(0), d.get(0x06).copied().unwrap_or(0), d.get(0x1F).copied().unwrap_or(0), d.get(0x1E).copied().unwrap_or(0)).as_bytes());
                                            // The provisioning lock and the write-protect flags (JEDEC UFS: attribute bConfigDescrLock G#1B is one-time — once 1 the LU layout is permanent; flags fPermanentWPEn G#02 and fPowerOnWPEn G#03; attribute bBootLunEn G#00).
                                            let fmt = |v: Option<u32>| match v { Some(x) => alloc::format!("{x}"), None => alloc::string::String::from("?") };
                                            let fmtb = |v: Option<bool>| match v { Some(x) => alloc::format!("{}", x as u8), None => alloc::string::String::from("?") };
                                            resp.extend_from_slice(alloc::format!("bConfigDescrLock {} bBootLunEn {} bCurrentPowerMode {} fPermanentWPEn {} fPowerOnWPEn {} fDeviceInit {}\n", fmt(ufs.read_attribute(0x1B, 0)), fmt(ufs.read_attribute(0x00, 0)), fmt(ufs.read_attribute(0x02, 0)), fmtb(ufs.read_flag(0x02)), fmtb(ufs.read_flag(0x03)), fmtb(ufs.read_flag(0x01))).as_bytes());
                                            let (c, cn, cocs) = ufs.read_descriptor(0x01, 0);
                                            resp.extend_from_slice(alloc::format!("CONFIG_DESC len {cn} ocs {cocs}: bootEnable {} descrAccessEn {} initPowerMode {} highPriorityLUN {}\n", c.get(0x03).copied().unwrap_or(0), c.get(0x04).copied().unwrap_or(0), c.get(0x05).copied().unwrap_or(0), c.get(0x06).copied().unwrap_or(0)).as_bytes());
                                            for lu in 0..8u8 {
                                                let (u, n, ocs) = ufs.read_descriptor(0x02, lu);
                                                if n < 0x13 || ocs != 0 {
                                                    resp.extend_from_slice(alloc::format!("LU{lu}: no descriptor (len {n}, ocs {ocs})\n").as_bytes());
                                                    continue;
                                                }
                                                let blocks = u64::from_be_bytes([u[0x0B], u[0x0C], u[0x0D], u[0x0E], u[0x0F], u[0x10], u[0x11], u[0x12]]);
                                                let bs = 1u64 << u[0x0A];
                                                let wp = match u[0x05] { 0 => "none", 1 => "POWER-ON", 2 => "PERMANENT", _ => "?" };
                                                resp.extend_from_slice(alloc::format!("LU{lu}: enable {} bootLunId {} writeProtect {} memType {} blockSize {bs} blocks {blocks} = {} MiB\n", u[0x03], u[0x04], wp, u[0x08], blocks * bs >> 20).as_bytes());
                                                if u[0x03] == 0 {
                                                    continue;
                                                }
                                                // GPT: header at LBA 1, the entry array where it says (usually LBA 2), 128-byte entries, UTF-16LE names at +56.
                                                if ufs.read_block_lun(lu, 1) != 0 || ufs.last_response_status() != 0 {
                                                    resp.extend_from_slice(b"    (LBA 1 unreadable)\n");
                                                    continue;
                                                }
                                                let h = *ufs.data_buffer();
                                                if &h[0..8] != b"EFI PART" {
                                                    resp.extend_from_slice(b"    (no GPT)\n");
                                                    continue;
                                                }
                                                let entry_lba = u64::from_le_bytes(h[0x48..0x50].try_into().unwrap());
                                                let count = u32::from_le_bytes(h[0x50..0x54].try_into().unwrap()).min(256) as usize;
                                                let esize = u32::from_le_bytes(h[0x54..0x58].try_into().unwrap()) as usize;
                                                let per_block = if esize > 0 { 4096 / esize } else { 0 };
                                                let mut names = alloc::string::String::new();
                                                let mut i = 0usize;
                                                while i < count && per_block > 0 {
                                                    if ufs.read_block_lun(lu, (entry_lba as usize + i / per_block) as u32) != 0 {
                                                        break;
                                                    }
                                                    let blk = *ufs.data_buffer();
                                                    for e in 0..per_block {
                                                        if i >= count {
                                                            break;
                                                        }
                                                        let ent = &blk[e * esize..(e + 1) * esize];
                                                        if ent[..16].iter().any(|&b| b != 0) {
                                                            let name: alloc::string::String = ent[56..128].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).filter_map(|c| char::from_u32(c as u32)).collect();
                                                            let first = u64::from_le_bytes(ent[32..40].try_into().unwrap());
                                                            let last = u64::from_le_bytes(ent[40..48].try_into().unwrap());
                                                            let attr = u64::from_le_bytes(ent[48..56].try_into().unwrap());
                                                            names.push_str(&alloc::format!(" {name}@{first}({}K)", (last + 1 - first) * bs >> 10));
                                                            if attr != 0 {
                                                                // GPT attribute bits (Android's A/B HALs keep slot state in bits 48+ on some SoCs); printed only when set, to see whether husky's bootloader uses them.
                                                                names.push_str(&alloc::format!("[attr G#{attr:X}]"));
                                                            }
                                                        }
                                                        i += 1;
                                                    }
                                                }
                                                resp.extend_from_slice(alloc::format!("    GPT:{names}\n").as_bytes());
                                            }
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
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
                                        #[cfg(not(shim_handoff))]
                                        if cmd.params == b"ready" && b.kind == BootKind::Cold {
                                            // Redraw the "Ready?" screen (dev check; the checklist comes back on the next boot) and count what landed.
                                            let (text, tri) = crate::ready::show();
                                            append_hex(&mut resp, b"READY_TEXT_PX=", text);
                                            append_hex(&mut resp, b"READY_TRIANGLE_PX=", tri);
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
                                        if cmd.params == b"cpu" {
                                            // The CPU's own feature registers (this core): what the instruction set offers, read rather than assumed — the 1:1 target for codegen, the checker and toka. 64-bit values as HI/LO pairs.
                                            macro_rules! id {
                                                ($name:literal, $reg:literal) => {{
                                                    let v: u64;
                                                    unsafe { core::arch::asm!(concat!("mrs {}, ", $reg), out(reg) v) };
                                                    append_hex(&mut resp, concat!($name, "_HI=").as_bytes(), (v >> 32) as u32);
                                                    append_hex(&mut resp, concat!($name, "_LO=").as_bytes(), v as u32);
                                                }};
                                            }
                                            id!("MIDR", "midr_el1");
                                            id!("MPIDR", "mpidr_el1");
                                            id!("ISAR0", "id_aa64isar0_el1");
                                            id!("ISAR1", "id_aa64isar1_el1");
                                            id!("ISAR2", "s3_0_c0_c6_2");
                                            id!("PFR0", "id_aa64pfr0_el1");
                                            id!("PFR1", "id_aa64pfr1_el1");
                                            id!("ZFR0", "s3_0_c0_c4_4");
                                            id!("SMFR0", "s3_0_c0_c4_5");
                                            id!("MMFR0", "id_aa64mmfr0_el1");
                                            id!("MMFR1", "id_aa64mmfr1_el1");
                                            id!("MMFR2", "id_aa64mmfr2_el1");
                                            id!("DFR0", "id_aa64dfr0_el1");
                                            id!("CTR", "ctr_el0");
                                            id!("DCZID", "dczid_el0");
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
                                        if cmd.params == b"boot" {
                                            // `diag boot`: reset-to-entry, then each boot step's time since ferros was entered (checklist.rs).
                                            resp.extend_from_slice(crate::checklist::boot_times().as_bytes());
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam uw scan" || cmd.params == b"cam uw scan init" {
                                            crate::camera::uw_scan(&mut resp, cmd.params == b"cam uw scan init");
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam uw up" {
                                            // `diag cam uw up`: power the ultrawide in its own sequence and read its identifier (camera.rs).
                                            crate::camera::uw_up(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params.starts_with(b"dvfs") {
                                            // `diag dvfs`: which core the kernel runs on, its clock measured (2^20 dependent adds), and every DVFS domain's rate as the ACPM firmware holds it. `diag dvfs set <domain index> <kHz>`: ask the firmware for a rate (acpm::dvfs_set), then measure again.
                                            let measure = || {
                                                let t0 = cntpct();
                                                let mut acc = 0u64;
                                                for _ in 0..(1u64 << 20) {
                                                    unsafe { core::arch::asm!("add {0}, {0}, #1", inout(reg) acc) };
                                                }
                                                let us = ((cntpct() - t0) * 1_000_000 / cntfrq()).max(1);
                                                (1u64 << 20) / us + (acc & 0)
                                            };
                                            let words: alloc::vec::Vec<&[u8]> = cmd.params.split(|&b| b == b' ').filter(|w| !w.is_empty()).collect();
                                            if words.len() == 4 && words[1] == b"set" {
                                                let num = |w: &[u8]| core::str::from_utf8(w).ok().and_then(|t| t.parse::<u32>().ok());
                                                match (num(words[2]), num(words[3])) {
                                                    (Some(d), Some(khz)) if (d as usize) < crate::acpm::DVFS_DOMAINS.len() => {
                                                        let r = crate::acpm::dvfs_set(d, khz);
                                                        // The firmware takes a moment to move the PLL; measure after it has.
                                                        let t = cntpct() + cntfrq() / 100;
                                                        while cntpct() < t {}
                                                        resp.extend_from_slice(alloc::format!("set {} to A#{khz} kHz: {r:?}\n", crate::acpm::DVFS_DOMAINS[d as usize]).as_bytes());
                                                    }
                                                    _ => resp.extend_from_slice(b"cpu set <domain index> <kHz>\n"),
                                                }
                                            }
                                            let mpidr: u64;
                                            unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr) };
                                            resp.extend_from_slice(alloc::format!("kernel core MPIDR G#{mpidr:X}, clock ~A#{} MHz\n", measure()).as_bytes());
                                            for (i, name) in crate::acpm::DVFS_DOMAINS.iter().enumerate() {
                                                match crate::acpm::dvfs_get(i as u32) {
                                                    Ok(k) => resp.extend_from_slice(alloc::format!("{i:>2} {name:<7} A#{k} kHz\n").as_bytes()),
                                                    Err(e) => resp.extend_from_slice(alloc::format!("{i:>2} {name:<7} error G#{e:X}\n").as_bytes()),
                                                }
                                            }
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"pmic rails" {
                                            // `diag pmic rails`: the two rails the camera's clocks run on, read through ACPM: S5M_VDD_INT (main PMIC BUCK5M, control G#1F, outputs G#20/G#C7) and S1S_VDD_CAM (sub PMIC BUCK1S, control G#0E, outputs G#0F/G#CA). Volts = 200 mV + 6.25 mV x code (s2mpg14/15-register.h). Under Linux, streaming: INT 568.75 mV, CAM 600 mV (idle 537.5 mV).
                                            let mv = |r: Result<u8, u32>| match r { Ok(c) => alloc::format!("G#{c:02X} ({} uV)", 200_000 + c as u32 * 6250), Err(e) => alloc::format!("error G#{e:X}") };
                                            let hx = |r: Result<u8, u32>| match r { Ok(c) => alloc::format!("G#{c:02X}"), Err(e) => alloc::format!("error G#{e:X}") };
                                            let (m, sb, t) = (crate::acpm::MAIN, crate::acpm::SUB, crate::acpm::T_PMIC);
                                            resp.extend_from_slice(alloc::format!("VDD_INT (BUCK5M): ctrl {} out1 {} out2 {}\nVDD_CAM (BUCK1S): ctrl {} out1 {} out2 {}\n",
                                                hx(crate::acpm::read(m, t, 0x1F)), mv(crate::acpm::read(m, t, 0x20)), mv(crate::acpm::read(m, t, 0xC7)),
                                                hx(crate::acpm::read(sb, t, 0x0E)), mv(crate::acpm::read(sb, t, 0x0F)), mv(crate::acpm::read(sb, t, 0xCA))).as_bytes());
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam bus" {
                                            // `diag cam bus`: mux and bring up both camera HS-I2C controllers (no device addressed).
                                            crate::camera::buses_up(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params.starts_with(b"cam pdma") {
                                            // `diag cam pdma`: hand the front end's register program to the PDMA engine (camera.rs), the only path to the region the CPU cannot write.
                                            let mut b2 = store.as_mut().and_then(|s| s.get_by_key("cam.frame").ok().flatten());
                                            if b2.is_none() && cfg!(cam_frame_blob) {
                                                b2 = Some(include_bytes!(concat!(env!("OUT_DIR"), "/cam_frame.bin")).to_vec());
                                            }
                                            let init = match b2.as_deref() {
                                                Some(b) if b.len() > 8 && &b[..4] == b"CFRM" => {
                                                    let n = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
                                                    if 8 + n <= b.len() { Some(b[8..8 + n].to_vec()) } else { None }
                                                }
                                                _ => store.as_mut().and_then(|s| s.get_by_key("cam.boitata.init").ok().flatten()),
                                            };
                                            let prog = match b2.as_deref() {
                                                Some(b) if b.len() > 8 && &b[..4] == b"CFRM" => {
                                                    let n = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
                                                    if 8 + n <= b.len() { Some(b[8 + n..].to_vec()) } else { None }
                                                }
                                                _ => store.as_mut().and_then(|s| s.get_by_key("cam.ispfe.prog").ok().flatten()),
                                            };
                                            let words: alloc::vec::Vec<&[u8]> = cmd.params.split(|&c| c == b' ').collect();
                                            // `cam pdma uw ...`: the ultrawide's own init and CPU setup (FERROS_CAM_UW) instead of the main camera's bundle.
                                            let (init, prog) = if words.contains(&&b"uw"[..]) { crate::camera::Cam::uw_bundle() } else { (init, prog) };
                                            crate::camera::pdma_probe(&mut resp, init, prog, if words.contains(&&b"slit"[..]) { 64 } else { 0 }, words.contains(&&b"fast"[..]), crate::camera::Shot::parse(&cmd.params));
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam frame" {
                                            // `diag cam frame`: `cam stream` plus the front end's DMA program from the vault (cam.ispfe.prog, tools/camera-prog.py); reports which output buffers were written.
                                            // Both tables can come as one value, `cam.frame` = 'CFRM' + init length (u32 LE) + init + program, because only one vault write per boot succeeds at the moment (2026-10-20).
                                            let mut bundle = store.as_mut().and_then(|s| s.get_by_key("cam.frame").ok().flatten());
                                            if bundle.is_none() && cfg!(cam_frame_blob) {
                                                bundle = Some(include_bytes!(concat!(env!("OUT_DIR"), "/cam_frame.bin")).to_vec());
                                            }
                                            let (init, prog) = match bundle.as_deref() {
                                                Some(b) if b.len() > 8 && &b[..4] == b"CFRM" => {
                                                    let n = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
                                                    if 8 + n <= b.len() { (Some(b[8..8 + n].to_vec()), Some(b[8 + n..].to_vec())) } else { (None, None) }
                                                }
                                                _ => (store.as_mut().and_then(|s| s.get_by_key("cam.boitata.init").ok().flatten()), store.as_mut().and_then(|s| s.get_by_key("cam.ispfe.prog").ok().flatten())),
                                            };
                                            crate::camera::frame_probe(&mut resp, init, prog);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam stream" || cmd.params == b"cam raw" || cmd.params == b"cam lochs" {
                                            // `diag cam stream`: replay the sensor init, configure CSI link 1, stream 300 ms, report what the link saw and dump its words.
                                            // The init comes from the same places `cam frame` takes it from: the vault's own key, the bundled `cam.frame` value, or the image's built-in bundle. The vault lost these keys in the move to manifestus, and the dump is the whole point of the command, so it should not depend on which of the three is present.
                                            let mut init = store.as_mut().and_then(|s| s.get_by_key("cam.boitata.init").ok().flatten());
                                            if init.is_none() {
                                                let mut b = store.as_mut().and_then(|s| s.get_by_key("cam.frame").ok().flatten());
                                                if b.is_none() && cfg!(cam_frame_blob) {
                                                    b = Some(include_bytes!(concat!(env!("OUT_DIR"), "/cam_frame.bin")).to_vec());
                                                }
                                                if let Some(b) = b.as_deref() {
                                                    if b.len() > 8 && &b[..4] == b"CFRM" {
                                                        let n = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
                                                        if 8 + n <= b.len() {
                                                            init = Some(b[8..8 + n].to_vec());
                                                        }
                                                    }
                                                }
                                            }
                                            if cmd.params == b"cam raw" { crate::camera::raw_probe(&mut resp, init) } else if cmd.params == b"cam lochs" { crate::camera::lochs_probe(&mut resp, init) } else { crate::camera::stream_probe(&mut resp, init) }
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam slit" || cmd.params == b"cam slitsweep" {
                                            // `diag cam slit`: set our own window and frame rate through the sensor's CCS registers and measure the rate change (camera.rs).
                                            let mut t = store.as_mut().and_then(|s| s.get_by_key("cam.boitata.init").ok().flatten());
                                            if t.is_none() {
                                                let mut b = store.as_mut().and_then(|s| s.get_by_key("cam.frame").ok().flatten());
                                                if b.is_none() && cfg!(cam_frame_blob) {
                                                    b = Some(include_bytes!(concat!(env!("OUT_DIR"), "/cam_frame.bin")).to_vec());
                                                }
                                                if let Some(b) = b.as_deref() {
                                                    if b.len() > 8 && &b[..4] == b"CFRM" {
                                                        let n = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
                                                        if 8 + n <= b.len() {
                                                            t = Some(b[8..8 + n].to_vec());
                                                        }
                                                    }
                                                }
                                            }
                                            if cmd.params == b"cam slitsweep" { crate::camera::slit_sweep(&mut resp, t) } else { crate::camera::slit(&mut resp, t, 64) }
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam preexist" {
                                            // `diag cam preexist`: front-end state Linux had before the capture window, which nothing in the replay sets up (camera.rs).
                                            crate::camera::preexisting(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam probe" {
                                            // `diag cam probe`: whether an impossible capability read is the sensor's answer or this driver's (camera.rs).
                                            crate::camera::read_probe(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam preview" {
                                            // `diag cam preview`: the camera app's 1344 x 1008 NV12 preview, its luma painted 1:1 (camera.rs).
                                            crate::camera::preview(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam iova" {
                                            // `diag cam iova`: every buffer the camera IOMMU maps, by walking its page table in DRAM (camera.rs).
                                            crate::camera::iova_scan(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam inherit" {
                                            // `diag cam inherit`: after a live handoff with the camera running, read the frame Linux's pipeline wrote and paint it (camera.rs).
                                            crate::camera::inherit_probe(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam scan all" {
                                            // `diag cam scan all`: power each camera module in turn and read its model ID (camera.rs).
                                            crate::camera::scan_all(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam buses" {
                                            // `diag cam buses`: all four camera I2C controllers up, nothing powered and no device addressed (camera.rs).
                                            crate::camera::buses_all(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam clamp" {
                                            // `diag cam clamp`: prove the camera-PMIC overvoltage guard refuses what it must, with no I2C at all (camera.rs).
                                            crate::camera::clamp_test(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam caps" {
                                            // `diag cam caps`: the sensor's MIPI CCS capability block — what rates, windows and modes the silicon says it allows (camera.rs).
                                            crate::camera::caps_dump(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam events" {
                                            // `diag cam events`: every ISP interrupt source that is reporting, with its bits named (camera.rs, camera_events.rs).
                                            crate::camera::events_dump(&mut resp, false);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam link" {
                                            // `diag cam link`: every CSI link 1 word the camera stack touches, for diffing a cold ferros against a streaming Android through the handoff (camera.rs).
                                            crate::camera::link_dump(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam fe" {
                                            // `diag cam fe`: power the ISP front end, restore its clocks, read CSI link 1's version.
                                            crate::camera::fe_up(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam up" {
                                            // `diag cam up`: power the main camera module (rails programmed and verified first), then read the sensor's model ID.
                                            crate::camera::power_up(&mut resp);
                                            crate::camera::sensor_id(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam off" {
                                            // `diag cam off`: power the main camera module down.
                                            crate::camera::power_down(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam scan" {
                                            // `diag cam scan`: which addresses acknowledge on the camera-PMIC bus.
                                            crate::camera::scan(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params.starts_with(b"acpm ") {
                                            // `diag acpm r <speedy> <type> <reg>` / `diag acpm u <speedy> <type> <reg> <val> <mask>` (hex): one PMIC transaction over the ACPM channel, for experiments without a rebuild.
                                            let words: alloc::vec::Vec<&[u8]> = cmd.params[5..].split(|&c| c == b' ').filter(|w| !w.is_empty()).collect();
                                            let hx = |w: &[u8]| -> Option<u32> { let w = w.strip_prefix(b"G#").or_else(|| w.strip_prefix(b"0x")).unwrap_or(w); u32::from_str_radix(core::str::from_utf8(w).ok()?, 16).ok() };
                                            let nums: alloc::vec::Vec<u32> = words.iter().skip(1).filter_map(|w| hx(w)).collect();
                                            let out = match (words.first().copied(), nums.as_slice()) {
                                                (Some(b"r"), [sp, ty, rg]) => alloc::format!("read {sp:X}/{ty:X}/{rg:X}: {:?}\n", crate::acpm::read(*sp, *ty, *rg).map(|v| alloc::format!("G#{v:02X}")).map_err(|e| alloc::format!("G#{e:X}"))),
                                                (Some(b"u"), [sp, ty, rg, v, m]) => {
                                                    let r = crate::acpm::update(*sp, *ty, *rg, *v as u8, *m as u8);
                                                    alloc::format!("update {sp:X}/{ty:X}/{rg:X} val G#{v:X} mask G#{m:X}: {:?}; now {:?}\n", r.map_err(|e| alloc::format!("G#{e:X}")), crate::acpm::read(*sp, *ty, *rg).map(|v| alloc::format!("G#{v:02X}")).map_err(|e| alloc::format!("G#{e:X}")))
                                                }
                                                _ => alloc::string::String::from("usage: acpm r <speedy> <type> <reg> | acpm u <speedy> <type> <reg> <val> <mask> (hex)\n"),
                                            };
                                            resp.extend_from_slice(out.as_bytes());
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam pmicon" {
                                            // `diag cam pmicon`: raise the camera PMIC's supply enables (sub-PMIC GPIO1, GPIO3) and read its ID.
                                            crate::camera::pmic_power_on(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam pmic" {
                                            // `diag cam pmic`: the camera PMIC's ID and the main camera's regulators, read only.
                                            crate::camera::pmic_dump(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params.len() == 8 && cmd.params.starts_with(b"modem s") && cmd.params[7].is_ascii_digit() {
                                            // `diag modem sN`: the staged cold modem bring-up, stages 0..=N (modem.rs): 0 survey, 1 power-on, 2 PHY, 3 link, 4 endpoint, 5 ROM boot.
                                            crate::modem::stages(&mut resp, cmd.params[7] - b'0');
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"kv selftest" {
                                            // `diag kv selftest`: one large put through the manifestus vault, with its block I/O counted and every appended block re-read (kvault.rs).
                                            match store.as_mut() {
                                                Some(s) => resp.extend_from_slice(s.selftest().as_bytes()),
                                                None => resp.extend_from_slice(b"no vault open\n"),
                                            }
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"log" {
                                            // `diag log`: the last 16 KiB of the flash text log (flashlog.rs).
                                            resp.extend_from_slice(&crate::flashlog::tail(16 * 1024));
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"modem files" {
                                            // `diag modem files`: the modem's images on the flash (modem.bin's TOC, NV file sizes), read only (modem.rs).
                                            crate::modem::files(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"modem readmain" {
                                            // `diag modem readmain`: read MAIN off the flash, CRC it against the TOC, time it.
                                            crate::modem::read_main(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"amp state" {
                                            // `diag amp state`: the speaker amplifiers' SPI clock path, pins and supply, read only (amp.rs).
                                            crate::amp::state(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"amp up" {
                                            // `diag amp up`: SPI up, supply, each amplifier's reset, boot-done and chip IDs.
                                            crate::amp::up(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"touch raw" {
                                            // `diag touch raw`: the chip's rawest live layer (mutual raw, no baseline removed), auto-ranged on the panel for five minutes (touch.rs).
                                            crate::touch::raw(&mut resp, 300_000);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"touch gamma" {
                                            // `diag touch gamma`: the pressure value alone in greyscale — a white disc of that radius and a full-screen flood at that level, to see how brightness maps to pressure (touch.rs).
                                            crate::touch::gamma(&mut resp, 30_000);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"touch decay" {
                                            // `diag touch decay`: a 1:1 10-bit decay screen stamped with a circle per touch reading: the contact's major axis in red, its minor in blue, its pressure in green (touch.rs).
                                            crate::touch::decay(&mut resp, 30_000);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"touch map" {
                                            // `diag touch map`: the touch chip's capacitance strength frame, live on the panel and as an ASCII map, for 12 s (touch.rs).
                                            crate::touch::map(&mut resp, 12_000);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"touch info" {
                                            // `diag touch info`: the touch chip's sensor grid and the addresses of its capacitance frames (touch.rs).
                                            crate::touch::info(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"touch state" {
                                            // `diag touch state`: the touch controller's clocks, pins and rails, read only (touch.rs).
                                            crate::touch::state(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"touch up" {
                                            // `diag touch up`: SPI controller up, rails on, reset released, controller ready, scan mode active.
                                            crate::touch::up(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"touch watch" {
                                            // `diag touch watch`: report touches for 8 s.
                                            crate::touch::watch(&mut resp, 8192);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"touch paint" {
                                            // `diag touch paint`: draw finger strokes on the panel for 16 s, the by-eye test of touch.
                                            crate::touch::paint(&mut resp, 16384);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"touch off" {
                                            crate::touch::off(&mut resp);
                                            queue_pt_response(&mut pt_out_data, &resp);
                                            continue;
                                        }
                                        if cmd.params == b"cam state" {
                                            // `diag cam state`: the main camera's power-up dependencies, read only (camera.rs), so a cold boot is measured before anything is written.
                                            crate::camera::state(&mut resp);
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
                                        // Percent of time since the service loop started that the core spent asleep in `wfe`, rather than spinning.
                                        append_hex(&mut resp, b"IDLE_PCT=", (asleep * 100 / cntpct().wrapping_sub(t_awake_from).max(1)) as u32);
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
                                            #[cfg(all(feature = "pixel8", not(shim_handoff)))]
                                            crate::s2mpu::report(&mut resp);
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
                                            // The PT record (leave(): words 24..27): the last command (cap bytes, op), and at a stall exit the open transfer and its age.
                                            append_hex(&mut resp, b"PREV_PT_CMD_CAP=", prev[24] as u32);
                                            append_hex(&mut resp, b"PREV_PT_CMD_OP=", (prev[24] >> 56) as u32);
                                            append_hex(&mut resp, b"PREV_PT_OPEN_SID=", prev[25] as u32);
                                            append_hex(&mut resp, b"PREV_PT_OPEN_RECV=", (prev[25] >> 32) as u32);
                                            append_hex(&mut resp, b"PREV_PT_OPEN_TOTAL=", prev[26] as u32);
                                            append_hex(&mut resp, b"PREV_PT_AGE_MS=", (prev[26] >> 32) as u32);
                                            append_hex(&mut resp, b"PREV_ALIVE=", prev[27] as u32);
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
                                        // A leading G#01 byte = hold (bridge `display --hold`); G#02 = a DSI packet script for the panel (dsim::sequence, bridge `display dcs`); G#03 = dump the DSIM link registers (bridge `display dsim`). Anything else is text for the painter.
                                        match cmd.params.split_first() {
                                            Some((2, script)) => {
                                                let mut rep = alloc::vec::Vec::new();
                                                if !crate::display::dpu_on() {
                                                    rep.extend_from_slice(b"DPU powered down\n");
                                                } else if !crate::dsim::alive() {
                                                    rep.extend_from_slice(b"DSIM0 not running\n");
                                                } else {
                                                    let (sent, fail) = crate::dsim::sequence(script, freq);
                                                    rep.extend_from_slice(b"DSI packets sent: ");
                                                    rep.extend_from_slice(alloc::format!("{sent}").as_bytes());
                                                    match fail {
                                                        Some(f) => rep.extend_from_slice(alloc::format!(", then FIFO stuck (FIFOCTRL G#{f:08x})\n").as_bytes()),
                                                        None => rep.extend_from_slice(b", all went\n"),
                                                    }
                                                }
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            Some((3, _)) => {
                                                let mut rep = alloc::vec::Vec::new();
                                                if !crate::display::dpu_on() {
                                                    rep.extend_from_slice(b"DPU powered down\n");
                                                } else {
                                                    crate::dsim::dump(&mut rep);
                                                    crate::dsim::dump_phy(&mut rep);
                                                }
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            // G#06 [hz] = switch the panel's refresh rate with its own command set, set the DSIM's command-TE timers, and measure the result (bridge `display mode 60|120`).
                                            Some((6, args)) if !args.is_empty() => {
                                                let mut rep = alloc::vec::Vec::new();
                                                let hz = args[0] as u32;
                                                let script = match hz {
                                                    60 => Some(crate::hk3::REFRESH_60),
                                                    120 => Some(crate::hk3::REFRESH_120),
                                                    _ => None,
                                                };
                                                if !crate::display::dpu_on() || !crate::dsim::alive() {
                                                    rep.extend_from_slice(b"display not running\n");
                                                } else if let Some(script) = script {
                                                    let (sent, fail) = crate::dsim::sequence(script, freq);
                                                    if fail.is_none() {
                                                        crate::dsim::set_cmd_te(hz);
                                                        // The DSIM's registers are shadowed: a frame applies the timers.
                                                        let _ = crate::display::show(if hz == 60 { b"ferros 60 Hz" } else { b"ferros 120 Hz" }, freq);
                                                    }
                                                    let (pin_hz, _) = crate::display::te_rate(freq);
                                                    rep.extend_from_slice(alloc::format!("mode {hz} Hz: {sent} packets{}; TE now {pin_hz} Hz\n", if fail.is_some() { " (FIFO stuck)" } else { "" }).as_bytes());
                                                } else {
                                                    rep.extend_from_slice(b"mode: 60 or 120\n");
                                                }
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            // G#0B [samples] [interval, 1/16 s units] [1 = battery only] = battery gauge samples over hsi2c_11 (hsi2c::gauge; bridge `display power [n] [sixteenths] [battery]`): voltage, current, the average of the samples. Battery only: the charger's buck is switched off for the run (the system then draws from the battery and the current is the system's), half a second for the gauge to see it, and the charger put back whatever happens.
                                            Some((11, args)) => {
                                                let mut rep = alloc::vec::Vec::new();
                                                let n = args.first().copied().unwrap_or(1).max(1) as u32;
                                                let gap = args.get(1).copied().unwrap_or(8) as u64 * freq / 16;
                                                let battery = args.get(2).copied().unwrap_or(0) >= 1;
                                                // 2 = also open the CHGIN input (CHG_CNFG_12 CHGINSEL) for the window, the stronger isolation.
                                                let open_input = args.get(2).copied().unwrap_or(0) == 2;
                                                let wait = |ticks: u64| {
                                                    let t0: u64;
                                                    unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) t0) };
                                                    loop {
                                                        let t: u64;
                                                        unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) t) };
                                                        if t.wrapping_sub(t0) >= ticks {
                                                            break;
                                                        }
                                                    }
                                                };
                                                let mut input_prev: Option<u8> = None;
                                                let charger = if battery {
                                                    match crate::hsi2c::charger::set_mode(0) {
                                                        Ok(prev) => {
                                                            rep.extend_from_slice(alloc::format!("charger mode G#{:X} -> buck off\n", prev & 0xF).as_bytes());
                                                            if open_input {
                                                                match crate::hsi2c::charger::input_select(false) {
                                                                    Ok(p) => {
                                                                        input_prev = Some(p);
                                                                        rep.extend_from_slice(alloc::format!("charger input CHG_CNFG_12 G#{p:02X} -> CHGIN deselected\n").as_bytes());
                                                                    }
                                                                    Err(e) => rep.extend_from_slice(alloc::format!("charger input write failed: G#{e:X}\n").as_bytes()),
                                                                }
                                                            }
                                                            wait(freq / 2);
                                                            Some(prev)
                                                        }
                                                        Err(e) => {
                                                            rep.extend_from_slice(alloc::format!("charger mode write failed: G#{e:X}; sampling as is\n").as_bytes());
                                                            None
                                                        }
                                                    }
                                                } else {
                                                    None
                                                };
                                                let mut sum_ua: i64 = 0;
                                                let mut sum_uv: u64 = 0;
                                                let mut got = 0u32;
                                                for i in 0..n {
                                                    if i > 0 {
                                                        wait(gap);
                                                    }
                                                    match crate::hsi2c::gauge::sample() {
                                                        Ok(g) => {
                                                            sum_ua += g.current_ua as i64;
                                                            sum_uv += g.vcell_uv as u64;
                                                            got += 1;
                                                            rep.extend_from_slice(alloc::format!("soc {}% temp {}.{:02} C vcell {} mV current {} mA (avg {} mA, avg vcell {} mV)\n", g.soc, g.temp_cc / 100, (g.temp_cc % 100).unsigned_abs(), g.vcell_uv / 1000, g.current_ua / 1000, g.avg_current_ua / 1000, g.avg_vcell_uv / 1000).as_bytes());
                                                        }
                                                        Err(e) => rep.extend_from_slice(alloc::format!("gauge read failed: G#{e:X} (1 controller not up, 2 busy, 4 no device, 8 bus error, 10 timeout, 20 empty)\n").as_bytes()),
                                                    }
                                                }
                                                if let Some(p) = input_prev {
                                                    match crate::hsi2c::charger::restore_input(p) {
                                                        Ok(()) => rep.extend_from_slice(alloc::format!("charger input G#{p:02X} restored\n").as_bytes()),
                                                        Err(e) => rep.extend_from_slice(alloc::format!("CHARGER INPUT RESTORE FAILED: G#{e:X} — write G#69[G#C5] = G#{p:02X} by hand\n").as_bytes()),
                                                    }
                                                }
                                                if let Some(prev) = charger {
                                                    match crate::hsi2c::charger::restore(prev) {
                                                        Ok(()) => rep.extend_from_slice(alloc::format!("charger mode G#{:X} restored\n", prev & 0xF).as_bytes()),
                                                        Err(e) => rep.extend_from_slice(alloc::format!("CHARGER RESTORE FAILED: G#{e:X} — write G#69[G#B9] = G#{prev:02X} by hand\n").as_bytes()),
                                                    }
                                                }
                                                if got > 0 {
                                                    let ma = sum_ua / got as i64 / 1000;
                                                    let mv = sum_uv / got as u64 / 1000;
                                                    rep.extend_from_slice(alloc::format!("MEAN over {got}: {ma} mA at {mv} mV = {} mW (negative = discharging)\n", ma * mv as i64 / 1000).as_bytes());
                                                }
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            // G#0D [target] [reg] = an 8-bit register read on hsi2c_11; G#0E [target] [reg] [val] = an 8-bit write, read back after (bridge `display i2c <target> <reg> [val]`). The PMIC-side chips: gauge G#36, charger G#69, PMIC G#66.
                                            Some((13, args)) if args.len() >= 2 => {
                                                let mut rep = alloc::vec::Vec::new();
                                                match crate::hsi2c::read_byte(crate::hsi2c::HSI2C_11, args[0], args[1]) {
                                                    Ok(v) => rep.extend_from_slice(alloc::format!("G#{:02X}[G#{:02X}] = G#{v:02X}\n", args[0], args[1]).as_bytes()),
                                                    Err(e) => rep.extend_from_slice(alloc::format!("i2c read failed: G#{e:X}\n").as_bytes()),
                                                }
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            Some((14, args)) if args.len() >= 3 => {
                                                let mut rep = alloc::vec::Vec::new();
                                                match crate::hsi2c::write_byte(crate::hsi2c::HSI2C_11, args[0], args[1], args[2]).and_then(|_| crate::hsi2c::read_byte(crate::hsi2c::HSI2C_11, args[0], args[1])) {
                                                    Ok(v) => rep.extend_from_slice(alloc::format!("G#{:02X}[G#{:02X}] <- G#{:02X}, reads back G#{v:02X}\n", args[0], args[1], args[2]).as_bytes()),
                                                    Err(e) => rep.extend_from_slice(alloc::format!("i2c write failed: G#{e:X}\n").as_bytes()),
                                                }
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            // G#0F [1 = ferros's own framebuffer (cachemap DISPLAY_FB), 0 = ABL's surface] = where the painter draws and the layer reads (bridge `display fb own|abl`), then a frame to prove it.
                                            Some((15, args)) if !args.is_empty() => {
                                                let mut rep = alloc::vec::Vec::new();
                                                if !crate::display::use_own_fb(args[0] == 1) {
                                                    rep.extend_from_slice(b"no framebuffer region on this boot path\n");
                                                } else {
                                                    let r = crate::display::show(if args[0] == 1 { b"ferros: own framebuffer" } else { b"ferros: ABL's surface" }, freq);
                                                    rep.extend_from_slice(&r);
                                                }
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            // G#10 [dither word] = the DQE's display dither register (DQE_DISP_DITHER: EN b0, MODE b1 = shift, FRAME_CON b2 = temporal, FRAME_OFFSET [4:3], TABLE_SEL R/G/B b5..b7) with the DQE shadow update requested; the next present applies it (bridge `display dither <hex>`). For finding the setting that doesn't speckle black at 10 bpc.
                                            Some((16, args)) if !args.is_empty() => {
                                                let mut rep = alloc::vec::Vec::new();
                                                let dqe = 0x194D_0000usize;
                                                unsafe {
                                                    write_volatile((dqe + 0x2800) as *mut u32, args[0] as u32);
                                                    let shd = 0x1947_0050usize;
                                                    write_volatile(shd as *mut u32, read_volatile(shd as *const u32) | 1 << 24);
                                                }
                                                crate::display::append_hex(&mut rep, b"DQE_DISP_DITHER=", unsafe { read_volatile((dqe + 0x2800) as *const u32) });
                                                crate::display::append_hex(&mut rep, b"DQE_CGC_DITHER=", unsafe { read_volatile((dqe + 0x2400) as *const u32) });
                                                crate::display::append_hex(&mut rep, b"DECON_GLOBAL_CON=", unsafe { read_volatile(0x1947_0020usize as *const u32) });
                                                crate::display::append_hex(&mut rep, b"DATA_PATH_CON_0=", unsafe { read_volatile(0x1947_0200usize as *const u32) });
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            // G#11 [diameter hi] [diameter lo] [r] [g] [b] [surround r] [g] [b] = a measurement disc centred on the panel over a surround; diameter 0 = full field (bridge `display patch <diameter px> <rrggbb> [surround rrggbb]`, tools/display-profile.sh).
                                            Some((17, args)) if args.len() >= 8 => {
                                                let diam = (args[0] as usize) << 8 | args[1] as usize;
                                                let c = |i: usize| (args[i] as u32) << 16 | (args[i + 1] as u32) << 8 | args[i + 2] as u32;
                                                let rep = crate::display::patch(diam, c(2), c(5), freq);
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            // G#12 [diameter hi lo] [r g b, 10-bit, hi lo each] [surround r g b, the same] = the measurement disc in 10 bits a channel, the layer in 2:10:10:10 (bridge `display patch10`).
                                            Some((18, args)) if args.len() >= 14 => {
                                                let diam = (args[0] as usize) << 8 | args[1] as usize;
                                                let v = |i: usize| (args[i] as u32) << 8 | args[i + 1] as u32;
                                                let rep = crate::display::patch10(diam, [v(2), v(4), v(6)], [v(8), v(10), v(12)], freq);
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            // G#13 [r g b, 10-bit, hi lo each] = DECON's background colour with the base window off; G#13 alone = the window back on (bridge `display bg <r> <g> <b>` / `display bg off`).
                                            Some((19, args)) => {
                                                let v = |i: usize| (args[i] as u32) << 8 | args[i + 1] as u32;
                                                let c = if args.len() >= 6 { Some([v(0), v(2), v(4)]) } else { None };
                                                let rep = crate::display::bg10(c, freq);
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            // G#14 [diameter hi lo] [word, 4 bytes BE] [surround word, 4 bytes BE] = the 10-bit layer's disc with raw words (bridge `display patchraw`).
                                            Some((20, args)) if args.len() >= 10 => {
                                                let diam = (args[0] as usize) << 8 | args[1] as usize;
                                                let w = |i: usize| u32::from_be_bytes([args[i], args[i + 1], args[i + 2], args[i + 3]]);
                                                let rep = crate::display::patch_raw(diam, w(2), w(6), freq);
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            // G#0C [r] [g] [b] = a solid colour over the whole panel (bridge `display fill rrggbb`).
                                            Some((12, args)) if args.len() >= 3 => {
                                                let rep = crate::display::fill((args[0] as u32) << 16 | (args[1] as u32) << 8 | args[2] as u32, freq);
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            // G#07 = set DECON0 to Android's state (10 bpc, dither, SRAM, QoS, interrupts), then paint a frame to apply it (bridge `display tune`).
                                            Some((7, _)) => {
                                                let mut rep = alloc::vec::Vec::new();
                                                if !crate::display::dpu_on() {
                                                    rep.extend_from_slice(b"DPU powered down\n");
                                                } else {
                                                    crate::display::configure_like_android(&mut rep);
                                                    let r = crate::display::show(b"ferros: DECON as Android", freq);
                                                    rep.extend_from_slice(&r);
                                                }
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            // G#08 [1 = FHD, 0 = WQHD] = switch the whole pipe's raster (bridge `display res fhd|wqhd`).
                                            Some((8, args)) if !args.is_empty() => {
                                                let mut rep = alloc::vec::Vec::new();
                                                if !crate::display::dpu_on() || !crate::dsim::alive() {
                                                    rep.extend_from_slice(b"display not running\n");
                                                } else {
                                                    crate::display::resolution(args[0] == 1, args.get(1).copied().unwrap_or(7), freq, &mut rep);
                                                }
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            // G#09 [floor: 30, 10, 1, or 0 = exit to manual 120] = the panel's auto-rate idle floor, then 1.5 s of stillness and the TE pin (bridge `display idle N`).
                                            Some((9, args)) if !args.is_empty() => {
                                                let mut rep = alloc::vec::Vec::new();
                                                let script = match args[0] { 30 => Some(crate::hk3::IDLE_30), 10 => Some(crate::hk3::IDLE_10), 1 => Some(crate::hk3::IDLE_1), 0 => Some(crate::hk3::IDLE_EXIT), _ => None };
                                                if !crate::display::dpu_on() || !crate::dsim::alive() {
                                                    rep.extend_from_slice(b"display not running\n");
                                                } else if let Some(script) = script {
                                                    let (sent, fail) = crate::dsim::sequence(script, freq);
                                                    // A second byte of 1 asks for changeable TE during the measurement, so the pin shows the panel's real rate; fixed TE is restored after.
                                                    let changeable = args.get(1) == Some(&1);
                                                    if changeable {
                                                        let _ = crate::dsim::sequence(crate::hk3::TE_CHANGEABLE, freq);
                                                    }
                                                    let t0 = cntpct();
                                                    while cntpct().wrapping_sub(t0) < freq * 3 / 2 {}
                                                    let ((pin_hz, _), width_us) = crate::display::te_profile(freq);
                                                    if changeable {
                                                        let _ = crate::dsim::sequence(crate::hk3::TE_FIXED, freq);
                                                    }
                                                    rep.extend_from_slice(alloc::format!("idle floor {}: {sent} packets{}; after 1.5 s still, TE {pin_hz} Hz ({width_us} us high){}\n", args[0], if fail.is_some() { " (FIFO stuck)" } else { "" }, if changeable { " [changeable TE]" } else { "" }).as_bytes());
                                                } else {
                                                    rep.extend_from_slice(b"idle: 30, 10, 1 or 0\n");
                                                }
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            // G#0A [1 = FHD, 0 = WQHD] [link rate: 0 keep, 1 = 1368 Mbps, 2 = 1346 Mbps] [blend depth: 0 keep, 1 = 8 bpc, 2 = 10 bpc] = the full modeset (dpu::modeset: DECON and DSIM stopped, re-initialised, started; bridge `display modeset fhd|wqhd [1368|1346] [8|10]`).
                                            Some((10, args)) if !args.is_empty() => {
                                                let mut rep = alloc::vec::Vec::new();
                                                crate::dpu::modeset(args[0] == 1, args.get(1).copied().unwrap_or(0), args.get(2).copied().unwrap_or(0), args.get(3).copied().unwrap_or(0), freq, &mut rep);
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            // G#05 = the panel's TE rate as DECON counts it (bridge `display terate`).
                                            Some((5, _)) => {
                                                let mut rep = alloc::vec::Vec::new();
                                                if !crate::display::dpu_on() {
                                                    rep.extend_from_slice(b"DPU powered down\n");
                                                } else {
                                                    let ((pin_hz, irq_hz), width_us) = crate::display::te_profile(freq);
                                                    rep.extend_from_slice(alloc::format!("TE: {pin_hz} Hz on the pin ({width_us} us high), {irq_hz} Hz by DECON interrupt\n").as_bytes());
                                                }
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            // G#04 [cmd][len] = a DCS read from the panel (bridge `display read`).
                                            Some((4, args)) if args.len() >= 2 => {
                                                let mut rep = alloc::vec::Vec::new();
                                                if !crate::display::dpu_on() {
                                                    rep.extend_from_slice(b"DPU powered down\n");
                                                } else {
                                                    match crate::dsim::read(args[0], args[1] as usize, freq) {
                                                        Ok(bytes) => {
                                                            rep.extend_from_slice(alloc::format!("read G#{:02x}:", args[0]).as_bytes());
                                                            for b in bytes {
                                                                rep.extend_from_slice(alloc::format!(" {b:02x}").as_bytes());
                                                            }
                                                            rep.push(b'\n');
                                                        }
                                                        Err(e) => rep.extend_from_slice(alloc::format!("read G#{:02x} failed: G#{e:08x}\n", args[0]).as_bytes()),
                                                    }
                                                }
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                            _ => {
                                                let (text, h) = match cmd.params.split_first() {
                                                    Some((1, rest)) => (rest, true),
                                                    _ => (cmd.params, false),
                                                };
                                                hold |= h;
                                                let rep = crate::display::show(text, freq);
                                                queue_pt_response(&mut pt_out_data, &rep);
                                            }
                                        }
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
                                        // Run the staged payload as a process (EL0, own arc, a console capability plus the requested endpoint grants); reply [exit][console]. Params: [name len][name][grant count][(kind 1 serve / 2 use / 3 hardware / 4 entropy / 5 dma, len, name)*][input].
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
                                                grants.push(match kind {
                                                    1 => crate::process::Grant::Serve(g),
                                                    3 => crate::process::Grant::Hardware(g),
                                                    4 => crate::process::Grant::Entropy,
                                                    5 => crate::process::Grant::Dma(g),
                                                    _ => crate::process::Grant::Use(g),
                                                });
                                            }
                                            let input = &pr[at.min(pr.len())..];
                                            // Signed capsules only: the gate (oracle.rs) must pass, or spawn never completes.
                                            let sealed = store.as_mut().and_then(|s| s.get_by_key("sealed").ok().flatten()).unwrap_or_default();
                                            match crate::process::spawn_capsule(if name.is_empty() { b"proc" } else { name }, code, input, &grants, &sealed) {
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
                                    } else if cmd.cap == cap_seal && cmd.op == ferros_pt::Op::Exec {
                                        // Install the staged masked capsule: gate + checker once, then its seal joins the vault's `sealed` list (idempotent). Reply [0][seal] or ERR.
                                        if run_ok && run_size > 0 {
                                            let capsule = unsafe { core::slice::from_raw_parts(b.stage as *const u8, run_size) };
                                            let resp = match (crate::process::install_capsule(capsule), store.as_mut()) {
                                                (Ok(seal), Some(store)) => {
                                                    let mut list = store.get_by_key("sealed").ok().flatten().unwrap_or_default();
                                                    if !list.chunks_exact(32).any(|e| e == seal) {
                                                        list.extend_from_slice(&seal);
                                                    }
                                                    match store.set("sealed", list) {
                                                        Ok(_) => {
                                                            let mut r = alloc::vec![0u8];
                                                            r.extend_from_slice(&seal);
                                                            r
                                                        }
                                                        Err(_) => alloc::vec::Vec::from(&b"ERR:install: vault write failed"[..]),
                                                    }
                                                }
                                                (Ok(_), None) => alloc::vec::Vec::from(&b"ERR:install: no vault"[..]),
                                                (Err(e), _) => {
                                                    let mut r = alloc::vec::Vec::from(&b"ERR:"[..]);
                                                    r.extend_from_slice(e.as_bytes());
                                                    r
                                                }
                                            };
                                            queue_pt_response(&mut pt_out_data, &resp);
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
