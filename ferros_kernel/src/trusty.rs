//! A minimal Trusty client: enough of Linux's Trusty transport (google-modules/trusty) to open one tipc channel and exchange one message — to start the AoC through GSA's hwmgr service without a TEE of ferros's own (SENSORS.md).
//!
//! The stack: FF-A (ffa.rs) carries everything; on top of it Trusty exposes a virtio device whose buffers are referred to by FF-A memory handle, not physical address (the one trick: a vring descriptor's `addr` is a handle from `ffa::mem_share`, trusty-virtio.c). On the tipc virtio device, a control plane opens a named-port channel, then data messages carry the GSA request and reply.
//!
//! Single-threaded and polling: no interrupts, no scheduler-share region, no teardown. Trusty is given CPU time by SMC_SC_NOP between steps (the one thing that cannot be skipped without interrupts).

use core::ptr::{read_volatile, write_volatile};
use ferros_hal::mmio::{cache_clean, cache_invalidate};

// ── SMC call numbers (smcall.h) ──
const SMC_SC_VIRTIO_GET_DESCR: u64 = 0x3200_0014;
const SMC_SC_VIRTIO_START: u64 = 0x3200_0015;
const SMC_SC_VDEV_KICK_VQ: u64 = 0x3200_0018;
const SMC_SC_NOP: u64 = 0x3C00_0003;
const SMC_SC_RESTART_LAST: u64 = 0x3C00_0000;
/// Fast call to select the Trusty API version (SMC_FASTCALL_NR(SECURE_MONITOR=60, 11)); Trusty stays at version 0 until the AP selects one, and the modern path (SMP, the NOP queue, memory objects, the virtio worker) depends on it. Current is 5.
const SMC_FC_API_VERSION: u64 = 0xBC00_000B;
const TRUSTY_API_VERSION_CURRENT: u64 = 5;

// ── SM error codes (sm_err.h) ──
const SM_ERR_INTERRUPTED: i64 = -3;
const SM_ERR_BUSY: i64 = -5;
const SM_ERR_CPU_IDLE: i64 = -13;
const SM_ERR_NOP_INTERRUPTED: i64 = -14;
const SM_ERR_NOP_DONE: i64 = -15;

const PAGE: usize = 4096;
const VIRTIO_ID_TRUSTY_IPC: u32 = 13;
const RSC_VDEV: u32 = 3;
const VRING_DESC_F_WRITE: u16 = 0x2;

// ── tipc (trusty-ipc.c) ──
const TIPC_CTRL_ADDR: u32 = 53;
const LOCAL_ADDR: u32 = 1024;
const CTRL_GO_ONLINE: u32 = 1;
const CTRL_CONN_REQ: u32 = 3;
const CTRL_CONN_RSP: u32 = 4;

/// One yielding (std) SMC with the interrupt/busy/idle resume loop every std call to Trusty needs (trusty.c `trusty_std_call_helper`): on interrupted/idle, re-enter with RESTART_LAST; on busy, retry the same args. Returns w0 as i64.
fn std_call(func: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    loop {
        let mut r = crate::ffa::smc8([func, a1, a2, a3, 0, 0, 0, 0])[0] as i64;
        while r == SM_ERR_INTERRUPTED || r == SM_ERR_CPU_IDLE {
            r = crate::ffa::smc8([SMC_SC_RESTART_LAST, 0, 0, 0, 0, 0, 0, 0])[0] as i64;
        }
        if r == SM_ERR_BUSY {
            continue;
        }
        return r;
    }
}

/// Give Trusty CPU time to make progress (deliver a ctrl message, consume a tx buffer, produce a reply). With no interrupts this is how work advances; repeat NOP while it reports more to do.
fn nop() {
    for _ in 0..256 {
        if std_call(SMC_SC_NOP, 0, 0, 0) != SM_ERR_NOP_INTERRUPTED {
            break;
        }
    }
}

// ── Shared buffers (cacheable .bss, identity-mapped = physical; shared with FF-A attr WB inner-shareable, cleaned/invalidated around every hand-off) ──
#[repr(C, align(4096))]
struct Buf([u8; PAGE]);
const RX_SLOTS: usize = 32;
static mut DESCR: Buf = Buf([0; PAGE]);
static mut VRING_RX: [Buf; 8] = [const { Buf([0; PAGE]) }; 8];
static mut VRING_TX: [Buf; 8] = [const { Buf([0; PAGE]) }; 8];
static mut RXBUF: [Buf; RX_SLOTS] = [const { Buf([0; PAGE]) }; RX_SLOTS];
static mut TXBUF: Buf = Buf([0; PAGE]);
static mut SCHED: Buf = Buf([0; PAGE]);

const SMC_SC_SCHED_SHARE_REGISTER: u64 = 0x3C00_0004;

/// Register the scheduler-share region (trusty-sched-share.c): a per-CPU priority table Trusty's shadow threads (which run the virtio worker) need to be scheduled. Header {hdr_size, cpu_count, percpu_data_size} then cpu_count × {cur_priority, ask_priority}; ask = NORMAL (2). The husky has 9 cores.
fn sched_share(out: &mut alloc::vec::Vec<u8>) {
    const CPUS: usize = 9;
    let m = (&raw mut SCHED) as *mut u8 as usize;
    unsafe {
        core::ptr::write_bytes(m as *mut u8, 0, PAGE);
        write_volatile(m as *mut u32, 12); // hdr_size
        write_volatile((m + 4) as *mut u32, CPUS as u32); // cpu_count
        write_volatile((m + 8) as *mut u32, 8); // percpu_data_size
        for c in 0..CPUS {
            write_volatile((m + 12 + c * 8 + 4) as *mut u32, 2); // ask_shadow_priority = NORMAL
        }
        cache_clean(m, PAGE);
    }
    match crate::ffa::mem_share(m, 1) {
        Ok(id) => {
            let r = std_call(SMC_SC_SCHED_SHARE_REGISTER, id & 0xFFFF_FFFF, id >> 32, PAGE as u64);
            out.extend_from_slice(alloc::format!("sched share register: A#{r}\n").as_bytes());
        }
        Err(e) => out.extend_from_slice(alloc::format!("sched share mem_share failed: A#{e}\n").as_bytes()),
    }
}

fn pa(p: *const u8) -> usize {
    p as usize
}

/// A legacy split virtqueue laid out in one shared region (virtio 1.0 legacy, trusty-virtio.c).
struct Vq {
    base: usize,
    bytes: usize,
    num: u16,
    align: usize,
    vdev_notify: u32,
    vring_notify: u32,
    handle: u64,
    last_used: u16,
    /// Each descriptor slot's shared buffer handle (what goes in desc.addr).
    buf_handle: [u64; RX_SLOTS],
    buf_pa: [usize; RX_SLOTS],
}

impl Vq {
    fn desc(&self, i: usize) -> usize {
        self.base + i * 16
    }
    fn avail(&self) -> usize {
        self.base + 16 * self.num as usize
    }
    fn used(&self) -> usize {
        // avail is 2 (flags) + 2 (idx) + 2*num + 2 (used_event), rounded up to align.
        let a = self.avail() + 4 + 2 * self.num as usize + 2;
        (a + self.align - 1) & !(self.align - 1)
    }
    fn set_desc(&self, i: usize, addr: u64, len: u32, flags: u16) {
        let d = self.desc(i);
        unsafe {
            write_volatile(d as *mut u64, addr);
            write_volatile((d + 8) as *mut u32, len);
            write_volatile((d + 12) as *mut u16, flags);
            write_volatile((d + 14) as *mut u16, 0);
        }
    }
    /// Put descriptor `i` on the available ring and publish it.
    fn offer(&self, i: u16) {
        let av = self.avail();
        unsafe {
            let idx = read_volatile((av + 2) as *const u16);
            write_volatile((av + 4 + 2 * (idx as usize % self.num as usize)) as *mut u16, i);
            core::arch::asm!("dsb sy");
            write_volatile((av + 2) as *mut u16, idx.wrapping_add(1));
        }
        unsafe { cache_clean(self.base, self.bytes) };
    }
    /// Poll the used ring for the next completion: (descriptor index, bytes). None if nothing new.
    fn reap(&mut self) -> Option<(u16, u32)> {
        unsafe { cache_invalidate(self.base, self.bytes) };
        let us = self.used();
        let idx = unsafe { read_volatile((us + 2) as *const u16) };
        if idx == self.last_used {
            return None;
        }
        let slot = self.last_used as usize % self.num as usize;
        let e = us + 4 + slot * 8;
        let id = unsafe { read_volatile(e as *const u32) } as u16;
        let len = unsafe { read_volatile((e + 4) as *const u32) };
        self.last_used = self.last_used.wrapping_add(1);
        Some((id, len))
    }
}

fn kick(vq: &Vq) -> i64 {
    std_call(SMC_SC_VDEV_KICK_VQ, vq.vdev_notify as u64, vq.vring_notify as u64, 0)
}

static mut RXVQ: Option<Vq> = None;
static mut TXVQ: Option<Vq> = None;
static mut REMOTE: u32 = 0;

/// vring_size(num, align): descriptor table + available ring rounded to align, then the used ring.
fn vring_size(num: usize, align: usize) -> usize {
    ((16 * num + 2 * (3 + num) + align - 1) & !(align - 1)) + 2 * 3 + 8 * num
}

/// Bring up the tipc virtio device: share the descriptor page, GET_DESCR, parse the resource table for the tipc vdev and its two vrings, lay out and share the vrings and message buffers, then VIRTIO_START. Returns the two queues, or a message.
fn virtio_bringup(out: &mut alloc::vec::Vec<u8>) -> Result<(), &'static str> {
    let descr = (&raw mut DESCR) as *mut u8 as usize;
    unsafe { core::ptr::write_bytes(descr as *mut u8, 0, PAGE) };
    unsafe { cache_clean(descr, PAGE) };
    let descr_id = match crate::ffa::mem_share(descr, 1) {
        Ok(h) => { out.extend_from_slice(alloc::format!("descr handle G#{h:X}\n").as_bytes()); h },
        Err(e) => {
            out.extend_from_slice(alloc::format!("share descr failed: A#{e}\n").as_bytes());
            return Err("share descr failed");
        }
    };
    let sz = std_call(SMC_SC_VIRTIO_GET_DESCR, descr_id & 0xFFFF_FFFF, descr_id >> 32, PAGE as u64);
    if sz <= 0 {
        return Err("GET_DESCR failed");
    }
    unsafe { cache_invalidate(descr, PAGE) };
    let rd32 = |o: usize| unsafe { read_volatile((descr + o) as *const u32) };
    if rd32(0) != 1 {
        return Err("resource table version != 1");
    }
    let n = rd32(4) as usize;
    // Every resource entry: type, and for a vdev its id, vring count and each vring's geometry. Trusty's START brings up ALL vdevs, so any we leave unconfigured would stall the whole device set.
    for i in 0..n {
        let off = rd32(16 + 4 * i) as usize;
        let ty = rd32(off);
        if ty == RSC_VDEV {
            let v = off + 4;
            let id = rd32(v);
            let nv = unsafe { read_volatile((descr + v + 21) as *const u8) };
            out.extend_from_slice(alloc::format!("entry {i}: RSC_VDEV id A#{id} notify A#{} vrings A#{nv} config_len A#{}:", rd32(v + 4), rd32(v + 16)).as_bytes());
            for k in 0..nv as usize {
                let vr = v + 24 + k * 20;
                out.extend_from_slice(alloc::format!(" [num A#{} align G#{:X} notify A#{}]", rd32(vr + 8), rd32(vr + 4), rd32(vr + 12)).as_bytes());
            }
            out.push(b'\n');
        } else {
            out.extend_from_slice(alloc::format!("entry {i}: type A#{ty}\n", ).as_bytes());
        }
    }
    // Find the RSC_VDEV entry whose virtio id is the tipc device.
    let mut vdev = 0usize;
    for i in 0..n {
        let off = rd32(16 + 4 * i) as usize;
        if rd32(off) == RSC_VDEV && rd32(off + 4) == VIRTIO_ID_TRUSTY_IPC {
            vdev = off + 4; // fw_rsc_vdev begins after the 4-byte type header
            break;
        }
    }
    if vdev == 0 {
        return Err("no tipc vdev in resource table");
    }
    let vdev_notify = rd32(vdev + 4);
    let num_vrings = unsafe { read_volatile((descr + vdev + 21) as *const u8) };
    if num_vrings != 2 {
        return Err("tipc vdev is not 2 vrings");
    }
    out.extend_from_slice(alloc::format!("tipc vdev at +G#{vdev:X}, notify A#{vdev_notify}, config_len A#{}\n", rd32(vdev + 16)).as_bytes());
    // vring[0] = rx (Trusty writes), vring[1] = tx (we write). Each fw_rsc_vdev_vring is 20 bytes at vdev+24.
    let mut make = |k: usize, region: usize| -> Result<Vq, &'static str> {
        let v = vdev + 24 + k * 20;
        let num = rd32(v + 8) as u16;
        let align = rd32(v + 4) as usize;
        let vring_notify = rd32(v + 12);
        if num == 0 || num as usize > RX_SLOTS || vring_size(num as usize, align) > PAGE * 8 {
            return Err("vring geometry out of range");
        }
        unsafe { core::ptr::write_bytes(region as *mut u8, 0, PAGE * 8) };
        unsafe { cache_clean(region, PAGE * 8) };
        let pages = (vring_size(num as usize, align) + PAGE - 1) / PAGE;
        let handle = crate::ffa::mem_share(region, pages as u32).map_err(|_| "share vring failed")?;
        out.extend_from_slice(alloc::format!("vring {k}: region G#{region:X} pages A#{pages} -> handle G#{handle:X}\n").as_bytes());
        // Write the vring's handle into the descriptor's da (low) / pa (high) fields.
        unsafe {
            write_volatile((descr + v) as *mut u32, handle as u32);
            write_volatile((descr + v + 16) as *mut u32, (handle >> 32) as u32);
        }
        Ok(Vq {
            base: region,
            bytes: pages * PAGE,
            num,
            align,
            vdev_notify,
            vring_notify,
            handle,
            last_used: 0,
            buf_handle: [0; RX_SLOTS],
            buf_pa: [0; RX_SLOTS],
        })
    };
    let mut rx = make(0, (&raw mut VRING_RX) as *mut u8 as usize)?;
    let tx = make(1, (&raw mut VRING_TX) as *mut u8 as usize)?;
    out.extend_from_slice(alloc::format!("rx: num A#{} align G#{:X} notify A#{}; tx: num A#{} align G#{:X} notify A#{}\n", rx.num, rx.align, rx.vring_notify, tx.num, tx.align, tx.vring_notify).as_bytes());
    // Share each rx message buffer and post it (device writes replies into it).
    for i in 0..rx.num as usize {
        let b = pa(unsafe { (&raw const RXBUF[i]) as *const u8 });
        let h = crate::ffa::mem_share(b, 1).map_err(|_| "share rxbuf failed")?;
        rx.buf_handle[i] = h;
        rx.buf_pa[i] = b;
        rx.set_desc(i, h, PAGE as u32, VRING_DESC_F_WRITE);
        rx.offer(i as u16);
    }
    // One tx buffer, shared; its handle is reused for every outgoing message.
    let tb = pa((&raw const TXBUF) as *const u8);
    let th = crate::ffa::mem_share(tb, 1).map_err(|_| "share txbuf failed")?;
    let mut tx = tx;
    tx.buf_handle[0] = th;
    tx.buf_pa[0] = tb;
    // Finish the virtio handshake in the descriptor before START, as the virtio core does: accept the device's features (gfeatures = dfeatures) and drive status to ACKNOWLEDGE|DRIVER|DRIVER_OK|FEATURES_OK (0x0F). Trusty reads these at START and only then brings the tipc device up and emits GO_ONLINE.
    let dfeatures = rd32(vdev + 8);
    unsafe {
        write_volatile((descr + vdev + 12) as *mut u32, dfeatures); // gfeatures
        write_volatile((descr + vdev + 20) as *mut u8, 0x0F); // status
    }
    unsafe { cache_clean(descr, PAGE) };
    let r = std_call(SMC_SC_VIRTIO_START, descr_id & 0xFFFF_FFFF, descr_id >> 32, sz as u64);
    if r != 0 {
        return Err("VIRTIO_START failed");
    }
    unsafe {
        RXVQ = Some(rx);
        TXVQ = Some(tx);
    }
    let dfeatures = rd32(vdev + 8);
    out.extend_from_slice(alloc::format!("GET_DESCR sz A#{sz}, START ret A#{r}, dfeatures G#{dfeatures:X}\n").as_bytes());
    // Probe the tx direction: send a CONN_REQ, kick tx, pump NOP, and see whether Trusty consumes our tx buffer (tx used.idx advances) and whether anything lands on rx. This isolates whether the vring plumbing works at all.
    let mut p = alloc::vec![0u8; 8 + 256];
    p[0..4].copy_from_slice(&CTRL_CONN_REQ.to_le_bytes());
    p[4..8].copy_from_slice(&256u32.to_le_bytes());
    let port = b"com.android.trusty.gsa.hwmgr.aoc";
    p[8..8 + port.len()].copy_from_slice(port);
    // After START: did Trusty touch the descriptor (status/gfeatures/vring handle)?
    unsafe { cache_invalidate(descr, PAGE) };
    out.extend_from_slice(alloc::format!("descr after START: status G#{:X} gfeatures G#{:X} rx-da G#{:X} rx-pa G#{:X}\n", unsafe { read_volatile((descr + vdev + 20) as *const u8) }, rd32(vdev + 12), rd32(vdev + 24), rd32(vdev + 24 + 16)).as_bytes());
    send(TIPC_CTRL_ADDR, &p);
    let txq = unsafe { (*(&raw const TXVQ)).as_ref().unwrap() };
    let rxq = unsafe { (*(&raw const RXVQ)).as_ref().unwrap() };
    let ks = kick(txq);
    // The queued (non-secure) kick variant, used when api >= SMP_NOP.
    let knc = std_call(0x3200_0019, txq.vdev_notify as u64, txq.vring_notify as u64, 0);
    out.extend_from_slice(alloc::format!("kick tx: SC ret A#{ks}, NC ret A#{knc}\n").as_bytes());
    kick(rxq);
    for _ in 0..4000 {
        std_call(SMC_SC_NOP, 0, 0, 0);
    }
    let txq = unsafe { (*(&raw mut TXVQ)).as_mut().unwrap() };
    let rxq = unsafe { (*(&raw mut RXVQ)).as_mut().unwrap() };
    unsafe {
        cache_invalidate(txq.base, txq.bytes);
        cache_invalidate(rxq.base, rxq.bytes);
    }
    let txu = unsafe { read_volatile((txq.used() + 2) as *const u16) };
    let rxu = unsafe { read_volatile((rxq.used() + 2) as *const u16) };
    out.extend_from_slice(alloc::format!("after CONN_REQ: tx used.idx A#{txu} (sent 1), rx used.idx A#{rxu}\n").as_bytes());
    Ok(())
}

/// Receive the next tipc message: poll the rx queue, return (src, dst, payload bytes copied into `buf`) and re-post the buffer. Pumps NOP while idle up to a bounded spin.
fn recv(buf: &mut [u8]) -> Option<(u32, u32, usize)> {
    let rx = unsafe { (*(&raw mut RXVQ)).as_mut().unwrap() };
    for _ in 0..4096 {
        if let Some((id, len)) = rx.reap() {
            let bp = rx.buf_pa[id as usize];
            unsafe { cache_invalidate(bp, PAGE) };
            // tipc_msg_hdr: src, dst at +0/+4, len at +12.
            let src = unsafe { read_volatile(bp as *const u32) };
            let dst = unsafe { read_volatile((bp + 4) as *const u32) };
            let plen = unsafe { read_volatile((bp + 12) as *const u16) } as usize;
            let n = plen.min(buf.len()).min(len as usize);
            unsafe { core::ptr::copy_nonoverlapping((bp + 16) as *const u8, buf.as_mut_ptr(), n) };
            // Re-post the buffer.
            rx.set_desc(id as usize, rx.buf_handle[id as usize], PAGE as u32, VRING_DESC_F_WRITE);
            rx.offer(id);
            kick(rx);
            return Some((src, dst, n));
        }
        nop();
    }
    None
}

/// Send one tipc message: header (src=LOCAL, dst) + payload, on the tx queue, and kick.
fn send(dst: u32, payload: &[u8]) {
    let tx = unsafe { (*(&raw mut TXVQ)).as_mut().unwrap() };
    let bp = tx.buf_pa[0];
    unsafe {
        write_volatile(bp as *mut u32, LOCAL_ADDR); // src
        write_volatile((bp + 4) as *mut u32, dst); // dst
        write_volatile((bp + 8) as *mut u16, 0); // reserved
        write_volatile((bp + 10) as *mut u16, 0); // shm_cnt
        write_volatile((bp + 12) as *mut u16, payload.len() as u16); // len
        write_volatile((bp + 14) as *mut u16, 0); // flags
        core::ptr::copy_nonoverlapping(payload.as_ptr(), (bp + 16) as *mut u8, payload.len());
        cache_clean(bp, PAGE);
    }
    tx.set_desc(0, tx.buf_handle[0], (16 + payload.len()) as u32, 0);
    tx.offer(0);
    kick(tx);
    nop();
}

/// Open the named tipc port and return its remote channel address.
fn connect(port: &str, out: &mut alloc::vec::Vec<u8>) -> Result<u32, &'static str> {
    let mut buf = [0u8; 512];
    // Wait for GO_ONLINE on the control plane.
    let mut online = false;
    for _ in 0..8 {
        if let Some((_, dst, n)) = recv(&mut buf) {
            if dst == TIPC_CTRL_ADDR && n >= 8 && u32::from_le_bytes(buf[0..4].try_into().unwrap()) == CTRL_GO_ONLINE {
                online = true;
                break;
            }
        }
    }
    if !online {
        return Err("no GO_ONLINE from Trusty");
    }
    out.extend_from_slice(b"tipc online\n");
    // CONN_REQ: ctrl msg {type=CONN_REQ, body_len=256} + name[256].
    let mut p = alloc::vec![0u8; 8 + 256];
    p[0..4].copy_from_slice(&CTRL_CONN_REQ.to_le_bytes());
    p[4..8].copy_from_slice(&256u32.to_le_bytes());
    p[8..8 + port.len()].copy_from_slice(port.as_bytes());
    send(TIPC_CTRL_ADDR, &p);
    // CONN_RSP: ctrl msg {type=CONN_RSP} + {target, status, remote, ...}.
    for _ in 0..8 {
        if let Some((_, dst, n)) = recv(&mut buf) {
            if dst == TIPC_CTRL_ADDR && n >= 8 + 12 && u32::from_le_bytes(buf[0..4].try_into().unwrap()) == CTRL_CONN_RSP {
                let status = u32::from_le_bytes(buf[12..16].try_into().unwrap());
                let remote = u32::from_le_bytes(buf[16..20].try_into().unwrap());
                if status != 0 {
                    return Err("CONN_RSP status != 0");
                }
                out.extend_from_slice(alloc::format!("connected: remote A#{remote}\n").as_bytes());
                return Ok(remote);
            }
        }
    }
    Err("no CONN_RSP")
}

/// `diag trusty aoc`: the whole path — FF-A up, virtio up, connect to GSA's AoC hwmgr port, send START, read the state back.
pub fn aoc_start(out: &mut alloc::vec::Vec<u8>) {
    if let Err(e) = crate::ffa::init() {
        out.extend_from_slice(alloc::format!("FF-A init failed: A#{e}\n").as_bytes());
        return;
    }
    // Select the Trusty API version first, as Linux's probe does; without it Trusty does not run the modern virtio path.
    let api = crate::ffa::smc8([SMC_FC_API_VERSION, TRUSTY_API_VERSION_CURRENT, 0, 0, 0, 0, 0, 0])[0];
    out.extend_from_slice(alloc::format!("Trusty API version selected: A#{}\n", api as i64).as_bytes());
    sched_share(out);
    if let Err(e) = virtio_bringup(out) {
        out.extend_from_slice(alloc::format!("virtio bring-up failed: {e}\n").as_bytes());
        return;
    }
    let remote = match connect("com.android.trusty.gsa.hwmgr.aoc", out) {
        Ok(r) => r,
        Err(e) => {
            out.extend_from_slice(alloc::format!("connect failed: {e}\n").as_bytes());
            return;
        }
    };
    unsafe { REMOTE = remote };
    // GSA request: hwmgr_req_hdr.cmd = HWMGR_CMD_STATE_CMD (1), hwmgr_state_cmd_req.cmd = GSA_AOC_START (1).
    let req = [1u32.to_le_bytes(), 1u32.to_le_bytes()].concat();
    send(remote, &req);
    let mut buf = [0u8; 64];
    match recv(&mut buf) {
        Some((_, _, n)) if n >= 12 => {
            let cmd = u32::from_le_bytes(buf[0..4].try_into().unwrap());
            let err = u32::from_le_bytes(buf[4..8].try_into().unwrap());
            let state = u32::from_le_bytes(buf[8..12].try_into().unwrap());
            out.extend_from_slice(alloc::format!("GSA reply: cmd G#{cmd:X} err A#{err} state A#{state} (2 = running)\n").as_bytes());
        }
        Some((_, _, n)) => out.extend_from_slice(alloc::format!("short GSA reply: A#{n} bytes\n").as_bytes()),
        None => out.extend_from_slice(b"no GSA reply\n"),
    }
}
