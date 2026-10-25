//! The Edge TPU ("rio", google,edgetpu-zuma) at G#1A00_0000, started as the AoC is: GSA's mailbox loads and authenticates the signed image (command 50), GSA's Trusty service starts the Cortex-R52 (trusty::tpu_cmd), then the kernel control interface (KCI) mailbox carries the first exchange (TPU.md; google-modules/edgetpu/rio).
//!
//! The image's config puts everything in the TZ-mapped carve-out G#9300_0000 (16 MiB), which the TPU sees at device G#1000_0000; shared data starts at G#9312_0000. The KCI queues and the info buffer sit in shared data where Linux's pool puts them, so the TPU reaches them through its secure mapping and ferros programs no SYSMMU.

use core::ptr::{read_volatile, write_volatile};

const TPU: usize = 0x1A00_0000;
/// The KCI mailbox (mailbox 0): context registers, then the command queue's at +G#1000 and the response queue's at +G#1800 (rio/config-mailbox.h, edgetpu-mailbox.h).
const KCI: usize = TPU + 0x3_0000;
const KCI_CMD: usize = KCI + 0x1000;
const KCI_RESP: usize = KCI + 0x1800;
/// The carve-out and how the TPU sees it.
const CARVE: usize = 0x9300_0000;
const DEV: u32 = 0x1000_0000;
/// The KCI queues and the info buffer in shared data (Linux: pool offset G#A_0000): 1023 command elements of 32 bytes, 1023 responses of 16.
const CMDQ: usize = 0x931C_0000;
const RESPQ: usize = 0x931C_8000;
const INFO: usize = 0x931D_0000;
const QUEUE_SIZE: u32 = 1023;
/// The stream security units in front of the TPU: every VID zeroed before start (mobile-soc-gsx01.c `gsx01_setup_ssmt`, 64 streams, read VIDs +G#1000, write +G#1200).
const SSMT: [usize; 2] = [0x1A3E_0000, 0x1A3F_0000];
const GSA_LOAD_TPU: u32 = 50;
const FW_HEADER: usize = 4096;

fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}
fn dev(phys: usize) -> u32 {
    (phys - CARVE) as u32 + DEV
}
fn line(out: &mut alloc::vec::Vec<u8>, s: alloc::string::String) {
    out.extend_from_slice(s.as_bytes());
}

/// The firmware's 4 KiB signed header, where GSA can read it: ferros's own non-cacheable DMA memory.
#[repr(C, align(4096))]
struct Hdr([u8; FW_HEADER]);
#[unsafe(link_section = ".bss.dma")]
static mut HDR: Hdr = Hdr([0; FW_HEADER]);

/// `diag tpu load`: the image from the vault (`tpu.fw`, tools/husky-tpu-fw.sh): its body into the carve-out, its header for GSA, GSA's load (it authenticates the image), the manager's state after (1 = loaded).
pub fn load(get: &mut dyn FnMut(&str) -> Option<alloc::vec::Vec<u8>>, out: &mut alloc::vec::Vec<u8>) -> bool {
    match crate::trusty::tpu_cmd(0, out) {
        Ok(0) => {}
        Ok(s) => {
            line(out, alloc::format!("TPU state A#{s}, not inactive: not loading\n"));
            return false;
        }
        Err(e) => {
            line(out, alloc::format!("TPU state: {e}\n"));
            return false;
        }
    }
    let Some(fw) = get("tpu.fw") else {
        line(out, "no tpu.fw in the vault (tools/husky-tpu-fw.sh)\n".into());
        return false;
    };
    if fw.len() <= FW_HEADER || &fw[0x400..0x404] != b"TPUF" {
        line(out, "tpu.fw is not a TPU image (no TPUF magic)\n".into());
        return false;
    }
    let body = &fw[FW_HEADER..];
    for (i, w) in body.chunks(4).enumerate() {
        let mut b = [0u8; 4];
        b[..w.len()].copy_from_slice(w);
        wr(CARVE + 4 * i, u32::from_le_bytes(b));
    }
    let hdr = &raw mut HDR;
    unsafe { (*hdr).0.copy_from_slice(&fw[..FW_HEADER]) };
    unsafe { core::arch::asm!("dsb sy") };
    let h = hdr as usize;
    let r = crate::gsa::call(GSA_LOAD_TPU, &[h as u32, (h >> 32) as u32, CARVE as u32, 0], 8192);
    line(out, alloc::format!("body G#{:X} bytes at G#{CARVE:X}; GSA load: {r:?}\n", body.len()));
    let st = crate::trusty::tpu_cmd(0, out);
    line(out, alloc::format!("TPU state after load: {st:?} (1 = loaded)\n"));
    matches!(st, Ok(1))
}

/// The KCI mailbox as `edgetpu_kci_reinit` + `edgetpu_mailbox_reset` + `init_doorbells` leave it: disabled, both queues placed and emptied, stale doorbells cleared, doorbell interrupts on, enabled.
fn kci_setup() {
    wr(KCI, 0);
    for a in (CMDQ..CMDQ + 0x8000).step_by(4).chain((RESPQ..RESPQ + 0x4000).step_by(4)) {
        wr(a, 0);
    }
    wr(KCI + 0x14, dev(CMDQ));
    wr(KCI + 0x18, 0);
    wr(KCI + 0x1C, QUEUE_SIZE);
    wr(KCI_CMD + 0xC, 0);
    wr(KCI_CMD + 0x8, 0);
    wr(KCI + 0x28, dev(RESPQ));
    wr(KCI + 0x2C, 0);
    wr(KCI + 0x30, QUEUE_SIZE);
    wr(KCI_RESP + 0xC, 0);
    wr(KCI_RESP + 0x10, 0);
    wr(KCI_RESP + 0x4, 1);
    wr(KCI + 0x10, 1);
    wr(KCI + 0x8, 1);
    wr(KCI + 0x20, 1);
    wr(KCI, 1);
}

/// The TPU's local power manager up, as `edgetpu_soc_pm_lpm_up` does (mobile-soc-gsx01.c, CONFIG_RIO): its microcode patched (`rio_patch_lpm`), the core asked to power state 4 and both clusters to 1, the four power state machines started where they are not in a valid state and waited for, then clock gating allowed. ABL leaves pd_tpu on but the LPM idle: the core and cluster requests read zero and the R52 never ran (2026-10-25: GSA said running, the KCI queue untouched).
fn lpm_up(out: &mut alloc::vec::Vec<u8>) -> bool {
    let imem = |n: usize, v: u32| wr(TPU + 0x1C_0800 + 4 * n, v);
    for (n, v) in [(190, 0x1106_1104u32), (3, 0x1005_1101), (13, 0x1105_1001), (22, 0x2200_1005), (28, 0x2126_1004), (32, 0x100B_1104), (33, 0x100F_2126), (41, 0x1009_0011), (135, 0x110E_100B), (138, 0x2125_1110), (164, 0x100B_1109), (174, 0x22B3_2125), (197, 0x0024_1002), (198, 0x0BC7_5301), (199, 0x0024_1102), (200, 0x0BC9_5001), (201, 0x1417_1018), (202, 0x0200_1118)] {
        imem(n, v);
    }
    wr(TPU + 0x1C_2020, 0);
    wr(TPU + 0x1C_2024, 0xC5);
    wr(TPU + 0x1C_2030, 5);
    wr(TPU + 0x1C_2034, 1);
    wr(TPU + 0x1C_012C, 3);
    imem(3, 0x2126_1101);
    imem(4, 0x1111_1005);
    let core = TPU + 0x1D_0028;
    wr(core, rd(core) | 4 << 3 | 1 << 6);
    wr(TPU + 0x1D_0030, 1 << 1);
    wr(TPU + 0x1D_0038, 1 << 1);
    let ctl = TPU + 0x1D_0020;
    if rd(ctl) & 1 << 8 != 0 {
        wr(ctl, rd(ctl) & !(1 << 5));
        line(out, "LPM: off-state flag set; tpuPowerOff cleared\n".into());
        return true;
    }
    for i in 0..4 {
        let st = TPU + 0x1C_1708 + i * 0x1000;
        if rd(st) & 0x10 == 0 {
            wr(TPU + 0x1C_1704 + i * 0x1000, 1);
        }
    }
    let f = crate::platform::cntfrq();
    for i in 0..4 {
        let st = TPU + 0x1C_1708 + i * 0x1000;
        let t0 = crate::platform::cntpct();
        while rd(st) & 0x80 == 0 {
            if crate::platform::cntpct().wrapping_sub(t0) > f / 32 {
                line(out, alloc::format!("LPM: PSM{i} did not settle (G#{:X})\n", rd(st)));
                return false;
            }
        }
    }
    for i in 0..4 {
        wr(TPU + 0x1C_1700 + i * 0x1000, 0);
    }
    wr(ctl, 1 << 3);
    wr(TPU + 0x1D_0068, 3);
    line(out, alloc::format!("LPM up: PSM status {:X?}, core G#{:X}, control G#{:X}\n", (0..4).map(|i| rd(TPU + 0x1C_1708 + i * 0x1000)).collect::<alloc::vec::Vec<_>>(), rd(core), rd(ctl)));
    true
}

/// `diag tpu start`: the KCI mailbox set up, the SSMT VIDs zeroed, GSA's START (state 2 = running), then EXCHANGE_INFO (KCI code 11) with a 64-byte info buffer: the reply and the buffer, which should carry the firmware's changelist (836291931 for this image).
pub fn start(out: &mut alloc::vec::Vec<u8>) {
    if !lpm_up(out) {
        return;
    }
    // Already started without the LPM (an earlier try this boot): hold the core in reset again first, as the driver's restart path does.
    if let Ok(2) = crate::trusty::tpu_cmd(0, out) {
        let r = crate::trusty::tpu_cmd(4, out);
        line(out, alloc::format!("SHUTDOWN first: {r:?}\n"));
    }
    kci_setup();
    for base in SSMT {
        for vid in 0..64 {
            wr(base + 0x1000 + 4 * vid, 0);
            wr(base + 0x1200 + 4 * vid, 0);
        }
    }
    unsafe { core::arch::asm!("dsb sy") };
    match crate::trusty::tpu_cmd(1, out) {
        Ok(s) => line(out, alloc::format!("START: state A#{s} (2 = running)\n")),
        Err(e) => return line(out, alloc::format!("START: {e}\n")),
    }
    for i in 0..16 {
        wr(INFO + 4 * i, 0);
    }
    // The command: {u64 seq, u16 code, u16 pad[3], {u64 address, u32 size, u32 flags}}.
    let c = CMDQ;
    wr(c, 1);
    wr(c + 4, 0);
    wr(c + 8, 11);
    wr(c + 12, 0);
    wr(c + 16, dev(INFO));
    wr(c + 20, 0);
    wr(c + 24, 64);
    wr(c + 28, 0);
    unsafe { core::arch::asm!("dsb sy") };
    wr(KCI_CMD + 0xC, 1);
    wr(KCI_CMD, 1);
    let f = crate::platform::cntfrq();
    let t0 = crate::platform::cntpct();
    while rd(KCI_RESP + 0x10) == 0 && crate::platform::cntpct().wrapping_sub(t0) < f {
        core::hint::spin_loop();
    }
    let ms = crate::platform::cntpct().wrapping_sub(t0) * 1000 / f;
    line(out, alloc::format!("KCI after A#{ms} ms: cmd head A#{} tail A#{}, resp head A#{} tail A#{}, resp status G#{:X}, context spare1 (boot stage) G#{:X}\n", rd(KCI_CMD + 8), rd(KCI_CMD + 0xC), rd(KCI_RESP + 0xC), rd(KCI_RESP + 0x10), rd(KCI_RESP + 8), rd(KCI + 0x38)));
    if rd(KCI_RESP + 0x10) != 0 {
        let r = RESPQ;
        line(out, alloc::format!("response: seq A#{} code A#{} status A#{} retval G#{:X}\n", rd(r) as u64 | (rd(r + 4) as u64) << 32, rd(r + 8) & 0xFFFF, rd(r + 8) >> 16, rd(r + 12)));
        let words: alloc::vec::Vec<u32> = (0..16).map(|i| rd(INFO + 4 * i)).collect();
        line(out, alloc::format!("info buffer: {words:08X?}\n"));
    }
}
