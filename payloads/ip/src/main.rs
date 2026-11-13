//! The IP process: ARP + IPv4 (DHCP, echo) and IPv6 (neighbour discovery, router solicitation, SLAAC, echo), with UDP for applications and a small client-only TCP (owner, 2026-10-26: fetches over plain HTTP, like Broadcom's LTO file; nothing listens). A client of two links — the WiFi driver's `wlan` endpoint (Ethernet frames) and the cellular data call's `rmnet` (raw IP packets: no link layer, the addresses and DNS servers given by the call, no DHCP or SLAAC) — and the server of `udp` (datagrams and connections for apps). Photon itself needs only UDP (WIFI.md).
//!
//! Both links stay up; a packet leaves on WiFi when WiFi holds an address of its family, else on the cellular call (owner, 2026-10-29: both up, prefer WiFi; IPv6 preferred, which the apps choose by asking for family 6). Link-scope and multicast traffic (Photon's LAN beacons) stays on WiFi. A TCP connection keeps the link and source it opened on. Everything it originates goes out with TTL and hop limit 63, both families on both links (`HOP`; owner, 2026-10-19 and 2026-10-31).
//!
//! Spawn after the WiFi driver serves `wlan` (tools/husky-wifi.sh 14): ferros-bridge spawn ip.bin --name ip --use wlan --serve udp --entropy --use rmnet
//! Grant slots: 0 console, 1 wlan (use), 2 udp (serve), 3 entropy, 4 rmnet (use; served by the modem driver, payloads/modem: its calls carry CALL_LIVE, so no driver means no cellular link, not a hang). WiFi frames go both ways in batches, one call each (`wlan` op 8, payloads/pcie1 `WLAN`: up to 128 KiB of received frames a reply, the frames ip sends batched until the next call, which comes at once), or one frame a call with a driver that has no op 8 (asked in `wifi_up`).
//!
//! `udp` requests (first byte):
//! - 1 SEND: family (4 or 6), address (16 bytes; IPv4 in the first 4), destination port (u16 BE), source port (u16 BE), payload → [0] sent, [1] resolving the next hop (retry), [2] no address yet, [3] bad request.
//! - 2 RECV: port (u16 BE) → family, address (16), source port, destination port, payload, or nothing (length 0). A RECV on a port, or a SEND from it, opens it: datagrams are kept only for the last eight ports asked for (a broadcast to a port nobody reads once filled the queue for good), the oldest giving way when the queue is full. IPv4 multicast and IPv6 link-scope multicast (ff02::; Photon's LAN beacons go to 239.104.199.144 and ff02::68c7:9014, port 4384) are delivered like unicast. A fourth byte, a wait in seconds (1 to 255), holds the call while the port has nothing: answered as a datagram for it lands, else with nothing when the wait runs out (a doorbell for a blocking reader, never a poll: owner, 2026-11-10, "No polling!"; 2026-11-11, Mahere fetching its map cells, "we have tcp and http so let's get the full meal deal wired up with mahere"). Without it, or 0, the answer is at once, as before.
//! - 3 INFO: → MAC (6), IPv4 address, mask, router (4 each), IPv6 link-local (16), IPv6 global (16), flags (bit 0 IPv4 bound, bit 1 IPv6 global, bit 2 the cellular call is up, bit 3 a person is present, so applications are served), the IPv4 DNS server (4), then the IPv6 DNS server (16): 71 bytes. The addresses and DNS servers are the ones traffic of each family leaves with: WiFi's when it has them (DHCP's option 6; the router advertisement's RDNSS), else the call's; a server of zeros when that link named none.
//!
//! While nobody is present (bit 3 clear) every request but INFO is answered with nothing: a SEND sends nothing, a RECV brings nothing, a CONNECT opens nothing. An application asks INFO first and says so (2026-11-12: Mahere and httptest said "DNS gave no address" all night on a locked device, their questions never sent).
//! - 4 TCP CONNECT: family, address (16), port (u16 BE) → [0, handle] connecting, [2] no address yet, [4] no free connection, [6] no memory for its buffers (the arc could not grow).
//! - 5 TCP SEND: handle, data (up to 4094 bytes: a call carries 4096, process.rs SYS_CALL) → [0, accepted u16 BE] (what the send buffer took; send the rest later), [5] not a connection.
//! - 6 TCP RECV: handle, wait, most (u32 BE) → [state, data…]: state 1 connecting, 2 open, 3 the peer closed (no more data once this is empty), 4 failed (reset or timed out), 5 not a connection (or a request shorter than its 7 bytes). The data is what the connection has taken in, up to `most` and at most 2^18 bytes (`RECV_MAX`, the kernel's largest reply), so the caller's reply buffer holds 1 + most (the kernel cuts a reply at the caller's length). With `wait` 1 to 255 (seconds) the call is held while the connection has nothing new: answered as data lands, as its state changes (it opens, the peer closes, it fails), or when the wait runs out (then its state and no data); 0 answers at once. A held receive is its app asking (no idle reaping while it waits); a second held receive on the same connection answers the first as things stand.
//!
//! Held receives: up to sixteen at once (`HELDN`), the oldest answered as things stand when a ninth comes. While nobody is present each is answered with nothing, as any request would be.
//! - 7 TCP CLOSE: handle → [0]: a FIN after the data already sent; the connection is freed once both sides have closed (or 16 s on). A connection its app has not asked about (send, receive, close) for 32 s is reset and freed: an app that dies leaves no slot behind.
//!
//! No op carries a version: every caller is in this repo or Mahere's and changes with it (owner, 2026-11-11: "No existing callers yet, so no need to version it").
//! - 8 JOIN: family, group address (16) → [0]: join a multicast group — an IGMPv2 membership report (IPv4) or an MLDv1 listener report (IPv6) now and every 60 s, so a snooping access point forwards the group to us (2026-10-26: the desktop's Photon beacons to 239.104.199.144 never reached the device until it reported itself a member; broadcast did). The firmware already passes all multicast; the switch was the filter. Up to eight groups.
//! - 9 LINK: mode → [0]: 0 both links, WiFi preferred (the default); 1 cellular only; 2 WiFi only. A test can take the cellular path with WiFi up; existing connections keep their link.
//! - 10 RESOLVE: family (0 either, 4, 6), wait (seconds), host name (dotted, up to 253 bytes) → status, family, address (16; IPv4 in the first 4), TTL left in seconds (u32 BE): 22 bytes. The device's one resolver (2026-11-12; httptest, the kist stower, fgtw and Mahere each carried a copy of their own, drifted apart), its pure parts in ferros_dns (tested on the host).
//!   - Status: 0 found; 1 still asking (wait 0, or the wait ran out first: the lookup goes on and its answer is kept); 2 no such address (the name does not exist, or has no address of the families asked); 3 not a host name (or a family other than 0, 4, 6); 4 no way to ask (no route of the families asked, or no DNS server, or no entropy for the question's id); 5 no answer (the servers silent or failing, four tries a second apart); 6 busy (every cache record in a lookup).
//!   - Family 0 asks for the family the route will use: AAAA where IPv6 has a route (IPv6 preferred: owner, 2026-10-29), then A where IPv4 has one. 4 or 6 asks for that type whatever the routes.
//!   - Wait 1 to 255 holds the call until the lookup ends, that long at most; 0 answers at once from the cache (a lookup started when it holds nothing fresh, the last good address meanwhile, else "still asking"). Held RESOLVEs: sixteen (`RQN`), the oldest answered "still asking" when a seventeenth comes.
//!   - The cache (`ferros_dns::Cache`): 32 records (a name and a type each), searched in order, the least recently asked giving way. An address holds for its TTL (the least along a chain of aliases), then is kept as the last good one: asked for, it is answered at once while a lookup for it runs, and a lookup that brings no answer keeps it for another TTL (Mahere's rules, 2026-11-11, now every client's). No such address is kept for the SOA's negative TTL (RFC 2308 5), at most 256 s (8 s without an SOA). A TCP connection that opens clears its address's failures; four failures in a row (resets, or what it sent unanswered) make the address stale, so its name is asked again.
//!   - One lookup a name and type, however many ask: the calls hold on it. A question goes from the resolver's one port (random for the session) with a random id (the entropy grant, each question), to the DNS server of each family's link (INFO's, IPv6's first, turn about), four tries a second apart (the one timer: a lost datagram never comes back); a try ip cannot send yet for want of the next hop goes again 2^-6 s on. An answer counts only from port 53 of a server a try went to, with the id, repeating the question (`ferros_dns::read`); a SERVFAIL or REFUSED moves to the next try at once. Nothing an application sends may leave from the resolver's port (op 1 answers [3]).
//!   - Each lookup's end is a console line: the type, found or not or no answer, its time and tries; never the name or the address (they are the person's).
//!
//! DHCPv4 (`dhcp_in`, `dhcp_retry`): DISCOVER, OFFER, REQUEST, ACK, retransmitted every 2 s until bound; four REQUESTs unanswered give the offer up for a fresh DISCOVER (RFC 2131 4.4.1). What it sent and heard since WiFi came up is a console line every 16th retransmit while unbound and once at the bind: `DHCP <no lease yet|bound>: A#<n> DISCOVERs and A#<n> REQUESTs out; A#<n> OFFERs, A#<n> ACKs, A#<n> NAKs in, A#<n> replies to other transactions; state A#<1 discovering, 2 requesting, 3 bound>[, lease A#<s> s]`. No renewal yet: the lease is said, not kept to.
//!
//! TCP here, client only, re-engineered for throughput on 2026-11-11 (owner: "sounds like it needs re-engineered", after Mahere's 1.6 MB base cells crawled thru two connections of a 4 KiB window, about 80 KB/s at a 50 ms round trip). The protocol's pure parts are ferros_tcp, tested on the host.
//! - Sixteen connections (`TCPN`). An idle one costs its control state alone (`Tcb`, a static table); an open one holds a block of the arc (`Blocks`): a 1 MiB receive buffer and a 256 KiB send buffer. The kernel maps 4 MiB of heap at spawn (three blocks); the arc grows for more as connections open (SYS_ARC, docs/ARCS.md) and shrinks again when the highest blocks are free.
//! - The receive window is the free room of the 1 MiB buffer, said with window scaling (RFC 7323, shift 5); a peer that offers no scaling gets 64 KiB of it. Its right edge moves by a segment, or half the buffer, at least (silly-window avoidance), and a read that reopens a window shrunk under half says so at once.
//! - Segments past a hole are kept in place in the buffer (eight ranges, `ferros_tcp::Ooo`) and delivered when the hole fills; each is ACKed at once with SACK blocks (RFC 2018), so the peer resends only what is missing.
//! - ACKs: every second in-order segment and one with PSH at once (RFC 5681 4.2, RFC 1122 4.2.3.2), an odd one at the end of the batch of frames it came in (after each exchange with the WiFi driver, and at the end of each pass over the links: `tcp_acks`); at once for FIN, out-of-order data, a filled hole or a duplicate. No delayed-ACK timer: the batch is microseconds, and the loop does not sleep while an ACK waits to go. The ACKs of a batch go out together in the next exchange. (From 2026-11-11 to 2026-11-12 a batch of up to 88 segments was ACKed once, and senders starved: docs/WIFI.md, "ACKs".)
//! - Sending within the peer's window and a congestion window: slow start, congestion avoidance, fast retransmit on three duplicate ACKs and NewReno recovery (RFC 5681, RFC 6582). SACK blocks the peer sends are read but not used yet: NewReno resends one hole a round trip. The retransmission timeout comes from measured round trips (RFC 6298, Karn's rule), 250 ms to 8 s, doubling at each expiry; the eighth expiry fails the connection. That timer is the one timer here, and it must be: a segment lost with nothing after it brings no ACK to say so. A closed peer window is probed on the same timer.
//! - The MSS from the link the connection leaves on: WiFi's 1500-byte MTU, the cellular call's MTU, less the headers of its family; 1200 when the call reports nothing believable.
//! - Each connection's measures go to the console when it is freed (`report`): bytes each way, round trip, retransmissions, out-of-order holding, the buffer's use, the peak rate.
//!   The line: `tcp A#<handle> port A#<port> <how it ended>: in A#<bytes> bytes, out A#<bytes>, open A#<ms> ms, peak A#<x.y> MiB/s, average A#<x.y> MiB/s, round trip A#<x.y> ms (variation A#<x.y>, timeout A#<x.y>), resent A#<n> segments (fast retransmits A#<n>, expiries A#<n>), out of order A#<n> segments (most held A#<bytes> bytes), waiting for the app most A#<bytes> of A#<window>, MSS A#<out> out A#<in> in, scale A#<out> out A#<in> in, SACK, congestion window A#<bytes>, segments in A#<n>, ACKs out A#<n>, A#<x.yy> a segment (most between two A#<bytes> bytes)`. For a WiFi connection a second line follows, the WiFi path since the last report (`Path`): `WiFi path since the last report: A#<n> passes (longest A#<us> us), A#<n> asleep for A#<ms> ms; A#<n> calls to the driver (A#<us> us on average, A#<us> us longest); A#<n> frames in (most A#<n> a call), A#<n> out (A#<n> refused)`.
//! - No listening, no TIME-WAIT (a client's ports are random), no timestamps, no keep-alives. Every incoming segment's checksum is checked; a reset counts only at the next expected byte, and one elsewhere in the window, or a SYN on an open connection, gets a challenge ACK (RFC 5961).
//!
//! Checksums are ones'-complement sums (RFC 1071: wide adds, one fold at the end). IPv4 UDP goes out with checksum 0 (allowed); IPv6 UDP and every ICMP carry one. The IPv6 interface identifier is random per session (from the entropy right) until a persistent device secret is granted (then RFC 7217 stable-privacy). No duplicate address detection yet.
//!
//! Native payload rule (KERNEL.md): unrelocated at its arc base — no `match` yielding string slices (pointer tables).

#![no_std]
#![no_main]

use ferros_sys::nr::{CALL, ENTROPY as ENTROPY_SYS, RECV, REPLY};
use ferros_sys::{now, svc, CALL_LIVE, ERR, NOTHING};

const S_CONSOLE: u64 = 0;
const S_WLAN: u64 = 1;
const S_UDP: u64 = 2;
const S_ENTROPY: u64 = 3;
const S_RMNET: u64 = 4;
/// The hop limit of everything this stack originates, IPv4 TTL and IPv6 hop limit alike, on WiFi and cellular: 63, not the usual 64 (owner, 2026-10-19; for all, 2026-10-31), as the husky Android kernel is built (IPDEFTTL, IPV6_DEFAULT_HOPLIMIT). One place, so a revert is one line. Not for what a protocol fixes: neighbour discovery's 255 (RFC 4861) and the 1 of IGMP and MLD reports.
const HOP: u8 = 63;

/// Datagrams held for the apps: a Photon Transport blast is up to 256 chunks at once (photon_core::pt), and a fleet frame is 30-50 of them; 64 slots in `.data` (an arc's stack is 64 KiB, the queue 96 KB) — the oldest gives way when full, PT re-sends what was lost.
const QN: usize = 64;

struct Datagram {
    used: bool,
    fam: u8,
    addr: [u8; 16],
    sport: u16,
    dport: u16,
    len: usize,
    seq: u32,
    data: [u8; 1472],
}

struct Ip {
    grants: u64,
    base: u64,
    freq: u64,
    log: [u8; 1024],
    n: usize,
    mac: [u8; 6],
    v4: [u8; 4],
    mask: [u8; 4],
    router: [u8; 4],
    bound: bool,
    dhcp_state: u8,
    xid: u32,
    offer: [u8; 4],
    server: [u8; 4],
    dhcp_t: u64,
    iid: [u8; 8],
    ll: [u8; 16],
    global: [u8; 16],
    have_global: bool,
    r6: [u8; 16],
    r6_mac: [u8; 6],
    have_r6: bool,
    rs_t: u64,
    rs_n: u32,
    arp: [([u8; 4], [u8; 6]); 8],
    arp_n: usize,
    nd: [([u8; 16], [u8; 6]); 8],
    nd_n: usize,
    q: &'static mut [Datagram; QN],
    frames: u32,
    /// WiFi's DNS servers: IPv4 from DHCP (option 6), IPv6 from the router's advertisement (RDNSS, RFC 8106). Zero for none; both forgotten with the network (`wifi_down`).
    dns: [u8; 4],
    dns6: [u8; 16],
    ports: [u16; 8],
    ports_at: usize,
    qseq: u32,
    /// The connections' control state (a static table) and their buffers' blocks of the arc.
    tcb: &'static mut [Tcb; TCPN],
    blocks: Blocks,
    eph: u16,
    seed: u32,
    groups: [(u8, [u8; 16]); 8],
    groups_n: usize,
    groups_t: u64,
    /// WiFi's driver answers on `wlan` (`wifi_up`); false until it does, and again after a call to it fails.
    wifi: bool,
    /// When the driver was last asked for its MAC.
    wifi_t: u64,
    cell: Cell,
    /// When the cellular call's info was last asked for.
    cell_t: u64,
    /// Whether a person is present (SYS_FACTS 83), and when it was last asked (once a second): while nobody is, applications get nothing but the addresses (op 3), AUTH.md "Radios while locked".
    person: bool,
    person_t: u64,
    /// The screen off (read with `person`, once a second): idle passes back off from 1 to 8 ms, `backoff` the shift of the next idle sleep (0 to 3).
    dark: bool,
    backoff: u32,
    /// LINK mode (request 9): 0 both, WiFi preferred; 1 cellular only; 2 WiFi only.
    mode: u8,
    /// Receives held until something comes (ops 2 and 6 with a wait), answered by `answer_held`.
    held: [Held; HELDN],
    /// Stamps the holds in arrival order: the oldest gives way when all are taken.
    held_seq: u32,
    /// The WiFi driver exchanges frames in batches (`wlan` op 8; `wifi_up` asks): frames to it wait in `txb` (the op's header, then each frame's u16 BE length and bytes; `txn` bytes used) until the next exchange. False: a driver without op 8, one frame a call (ops 1 and 2).
    batch: bool,
    txb: &'static mut [u8; MSG_MAX],
    txn: usize,
    /// The WiFi path's measures since the last connection report.
    path: Path,
    /// The WiFi doorbell (`WIFI_RING`): in use (armed, the idle wait on it), its helper thread started, and a driver found without it (never armed again).
    ring: bool,
    ring_thread: bool,
    ring_none: bool,
    /// The resolver (op 10): a static, not the 64 KiB stack.
    res: &'static mut Dns,
    /// DHCPv4's measures since WiFi came up (`wifi_up`), said every 16th retransmit while unbound and once at the bind (2026-11-13: the 8a got no lease for minutes on end while IPv6 worked, and nothing said why).
    dhcp_n: DhcpCount,
}

/// What DHCPv4 sent and heard since WiFi came up (`DhcpCount::say`).
#[derive(Default, Clone, Copy)]
struct DhcpCount {
    /// DISCOVERs and REQUESTs out.
    discovers: u32,
    requests: u32,
    /// OFFERs, ACKs and NAKs in for our transaction; server replies for another transaction (another client's, or ours from before a restart).
    offers: u32,
    acks: u32,
    naks: u32,
    others: u32,
    /// REQUESTs in a row with no answer: four, and the offer is given up for a fresh DISCOVER (RFC 2131 4.4.1: a client that hears no ACK goes back to INIT).
    unanswered: u32,
    /// The lease the ACK gave (option 51), in seconds; 0 none said.
    lease_s: u32,
}

/// The WiFi path's measures between two connection reports (`report`), said after each on a line of their own (owner, 2026-11-11: measure the path before fixing it). Counter ticks where a time is kept.
#[derive(Default)]
struct Path {
    /// Passes of the main loop and the longest; the passes that ended asleep, and the ticks asleep (a sleep ends at its deadline: 1 ms, up to 8 with the screen off).
    passes: u32,
    pass_most: u64,
    sleeps: u32,
    slept_t: u64,
    /// Calls to the WiFi driver that moved frames (exchanges, or one frame a call to a driver without op 8): how many, their ticks in all (the wait for the driver to be scheduled is in them), the longest.
    calls: u32,
    call_t: u64,
    call_most: u64,
    /// Frames in, the most in one call; frames out, and those the driver refused.
    frames_in: u32,
    in_most: u32,
    frames_out: u32,
    refused: u32,
    /// The doorbell (`WIFI_RING`): the rings taken, and the idle waits ended by a ring (or the word changed before the wait), by an application's call, by the timer.
    rings: u32,
    by_ring: u32,
    by_call: u32,
    by_timer: u32,
    /// When this window began (0: not yet), for the idle report every 64 s or more (`main`): with no timer bringing passes, the window's passes and waits are the stack's wakes at rest.
    t0: u64,
}

impl Path {
    /// `WiFi path since the last report: A#<n> passes (longest A#<us> us), A#<n> asleep for A#<ms> ms; A#<n> calls to the driver (A#<us> us on average, A#<us> us longest); A#<n> frames in (most A#<n> a call), A#<n> out (A#<n> refused)`.
    fn say(&self, ip: &mut Ip) {
        let f = ip.freq;
        let us = |t: u64| t * 1_000_000 / f;
        let (pass_most, slept, call_most) = (us(self.pass_most), us(self.slept_t) / 1000, us(self.call_most));
        ip.put(b"WiFi path since the last report");
        if self.t0 != 0 {
            ip.put(b" (A#");
            ip.dec(us(now().wrapping_sub(self.t0)) / 1000);
            ip.put(b" ms)");
        }
        ip.put(b": A#");
        ip.dec(self.passes as u64);
        ip.put(b" passes (longest A#");
        ip.dec(pass_most);
        ip.put(b" us), A#");
        ip.dec(self.sleeps as u64);
        ip.put(b" asleep for A#");
        ip.dec(slept);
        ip.put(b" ms; A#");
        ip.dec(self.calls as u64);
        ip.put(b" calls to the driver");
        // WHY the test: the average is per call, and none may have moved frames since the last report.
        if self.calls > 0 {
            let avg = us(self.call_t) / self.calls as u64;
            ip.put(b" (A#");
            ip.dec(avg);
            ip.put(b" us on average, A#");
            ip.dec(call_most);
            ip.put(b" us longest)");
        }
        ip.put(b"; A#");
        ip.dec(self.frames_in as u64);
        ip.put(b" frames in (most A#");
        ip.dec(self.in_most as u64);
        ip.put(b" a call), A#");
        ip.dec(self.frames_out as u64);
        ip.put(b" out (A#");
        ip.dec(self.refused as u64);
        ip.put(b" refused)");
        // The doorbell's own (`WIFI_RING`): its rings, and how the idle waits ended (a ring, an application's call, the timer).
        if WIFI_RING {
            ip.put(b"; doorbell A#");
            ip.dec(self.rings as u64);
            ip.put(b" rings, idle waits ended by a ring A#");
            ip.dec(self.by_ring as u64);
            ip.put(b", a call A#");
            ip.dec(self.by_call as u64);
            ip.put(b", the timer A#");
            ip.dec(self.by_timer as u64);
        }
        ip.put(b"\n");
    }
}

/// Frames taken from each link a pass, and application requests: 64 frames (a 1 MiB window in flight is about 700 segments; at 16 a pass a busy link waited on its own queue), eight requests.
const FRAMES_PASS: usize = 1 << 6;
const REQS_PASS: usize = 1 << 3;
/// Exchanges with the WiFi driver a pass while it says more frames wait, and the most bytes of frames one takes in (128 KiB, about 88 full frames: the driver's own largest reply, pcie1 `REPLY_MAX`).
const EXCH_PASS: usize = 1 << 2;
const EXCH_MAX: usize = 1 << 17;
/// An exchange's header: the op and the most bytes its reply may hold (u32 BE). Its reply's header: flags (bit 0 more frames wait) and the frames refused (u16 BE).
const EXCH_HEAD: usize = 5;
const EXCH_REPLY: usize = 3;
/// The exchanges' reply buffer (`wlan_batch`): a static, not the 64 KiB stack.
static mut EXCH: [u8; EXCH_MAX] = [0; EXCH_MAX];

/// The WiFi doorbell (the default since 2026-11-13; `FERROS_WIFI_IRQ=0` builds ask the driver every idle millisecond as before): a second thread holds a call to the driver (`wlan` op 9) that the driver answers only once frames wait, and rings `RING`; the main thread's idle wait is on `RING`, ended also by an application's call (`wait_world_or_call`) and by its own next timer, so an idle stack stops asking the driver a thousand times a second and the driver can sleep until its chip rings (payloads/pcie1 `WIFI_IRQ`). A driver without op 9 answers it with nothing, and the stack asks it every idle millisecond then.
const WIFI_RING: bool = match option_env!("FERROS_WIFI_IRQ") {
    Some(v) => !(v.len() == 1 && v.as_bytes()[0] == b'0'),
    None => true,
};
/// The doorbell's word: 0 armed (the helper's call held at the driver, or about to be), 1 frames wait at the driver, 2 the driver gone (the call failed: the main thread re-arms it once `wifi_up` finds a driver again), 3 a driver without the doorbell (op 9 answered with nothing: never armed again).
static RING: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The helper's stack: it only calls and waits.
static mut RING_STACK: [u8; 1 << 13] = [0; 1 << 13];

/// The doorbell's helper thread (`WIFI_RING`): the held call, its answer into `RING`, the main thread woken, then a wait until the main thread has taken the frames (`RING` back to 0) before asking again.
extern "C" fn ring_thread(grants: u64) -> ! {
    use core::sync::atomic::Ordering::{Acquire, Release};
    let base = ferros_sys::arc_base();
    let cap = grants + 0x204 + 32 * S_WLAN;
    let q = [9u8];
    let mut r = [0u8; 1];
    loop {
        let n = svc(CALL, cap, q.as_ptr() as u64 - base, 1, r.as_mut_ptr() as u64 - base, 1 | CALL_LIVE);
        // A 0 in the answer: the driver let this ask go for a newer one (`pcie1` keeps one), so ask again.
        let v = match n {
            ERR => 2,
            0 => 3,
            _ if r[0] == 1 => 1,
            _ => continue,
        };
        RING.store(v, Release);
        ferros_sys::wake(&RING, 2);
        while RING.load(Acquire) == v {
            ferros_sys::wait_world(&RING, v, 0);
        }
    }
}

/// Held receives at once: sixteen, a reader on each of the connections, or eight UDP readers (as many as the ports kept for them) and eight connections.
const HELDN: usize = 1 << 4;

/// One held receive: its reply token, when its wait runs out, and what it waits on (a UDP port, or a TCP connection's handle when `tcp`, with the connection's state when it was held and the most data its reader takes).
#[derive(Clone, Copy)]
struct Held {
    used: bool,
    tcp: bool,
    port: u16,
    handle: u8,
    state: u8,
    most: u32,
    seq: u32,
    until: u64,
    tok: [u8; 32],
}
const HELD_FREE: Held = Held { used: false, tcp: false, port: 0, handle: 0, state: 0, most: 0, seq: 0, until: 0, tok: [0; 32] };

/// The resolver's cache (op 10, `ferros_dns::Cache`): 32 records, a name and a type each, searched in order (a device's handful of hosts; no HashMap).
const RECN: usize = 1 << 5;
/// RESOLVE calls held at once, as many as the held receives (`HELDN`): Mahere's four loaders, the stower, fgtw, httptest, with room.
const RQN: usize = 1 << 4;
/// Tries a question gets before its lookup ends with no answer: four, a second each, turn about between the two families' servers when there are two (Mahere asked four times, the others three).
const DNS_TRIES: usize = 1 << 2;
/// How long a try waits for its answer: a second, the one timer here, and it must be: a question or its answer lost on the way (UDP, nothing resends it) brings nothing to say so, and only a deadline asks again. A server's answer comes in tens of milliseconds (the 8a, 2026-11-12: 84 ms over IPv4, 253 ms over IPv6), so a second is a lost datagram.
const DNS_WAIT_LOG2: u32 = 0;
/// A question ip could not send yet because it is finding the next hop (`udp_send`'s 1: an ARP or neighbour solicitation went instead) is sent again 2^-6 s on, up to 2^6 times before the try counts as lost: nothing rings when the neighbour answers, and the old clients asked every 2 ms.
const DNS_HOP_LOG2: u32 = 6;
const DNS_HOPS: u8 = 1 << 6;
/// A RESOLVE answer: status, family, address (16), TTL left in seconds (u32 BE).
const RES_LEN: usize = 22;
/// RESOLVE's statuses (the header, op 10).
const RES_FOUND: u8 = 0;
const RES_ASKING: u8 = 1;
const RES_NO_NAME: u8 = 2;
const RES_BAD: u8 = 3;
const RES_NO_WAY: u8 = 4;
const RES_NO_ANSWER: u8 = 5;
const RES_BUSY: u8 = 6;

/// A lookup in flight for cache record k (the same index): the question's id (from the entropy grant), the try it is on, whether that try waits on the next hop (and how often it has), when the try runs out, when the lookup began, and the server each try went to (an answer is taken only from one of them, from port 53).
#[derive(Clone, Copy)]
struct Ask {
    id: u16,
    tries: usize,
    hop: bool,
    hops: u8,
    due: u64,
    t0: u64,
    asked: [(u8, [u8; 16]); DNS_TRIES],
}
const ASK_FREE: Ask = Ask { id: 0, tries: 0, hop: false, hops: 0, due: 0, t0: 0, asked: [(0, [0; 16]); DNS_TRIES] };

/// A RESOLVE held until its lookup ends (or its wait runs out): the reply token, the name, the record types to ask in order (AAAA then A for either family) and the one it is on, the record whose lookup it waits on, the worst status so far (`res_worse`), when its wait runs out, its arrival (the oldest gives way).
#[derive(Clone, Copy)]
struct Rq {
    used: bool,
    tok: [u8; 32],
    name: ferros_dns::Name,
    order: [u16; 2],
    n: usize,
    phase: usize,
    rec: usize,
    worst: u8,
    until: u64,
    seq: u32,
}
const RQ_FREE: Rq = Rq { used: false, tok: [0; 32], name: ferros_dns::Name::EMPTY, order: [0; 2], n: 0, phase: 0, rec: 0, worst: RES_NO_WAY, until: 0, seq: 0 };

/// The resolver (op 10): the device's one cache, the lookups in flight, the calls held on them, and the one port its questions leave from (random for the session, from the entropy grant: RFC 5452 9.2).
struct Dns {
    cache: ferros_dns::Cache<RECN>,
    asks: [Ask; RECN],
    rq: [Rq; RQN],
    rq_seq: u32,
    port: u16,
}

/// Of two final statuses for a RESOLVE over two families, the one to say: no answer (the servers silent: nobody knows) over no such address over no way to ask.
fn res_worse(a: u8, b: u8) -> u8 {
    let rank = |s: u8| match s {
        RES_NO_ANSWER => 2,
        RES_NO_NAME => 1,
        _ => 0,
    };
    if rank(b) > rank(a) { b } else { a }
}

/// A RESOLVE answer: `status`, and for a found address its family, the address and the seconds it holds.
fn res_answer(status: u8, fam: u8, addr: [u8; 16], ttl: u32) -> [u8; RES_LEN] {
    let mut a = [0u8; RES_LEN];
    a[0] = status;
    a[1] = fam;
    a[2..18].copy_from_slice(&addr);
    a[18..22].copy_from_slice(&ttl.to_be_bytes());
    a
}

/// A RESOLVE answer into `serve`'s reply buffer; its length.
fn res_put(out: &mut [u8], a: [u8; RES_LEN]) -> usize {
    out[..RES_LEN].copy_from_slice(&a);
    RES_LEN
}

/// The family of a record type's addresses.
fn fam_of(qtype: u16) -> u8 {
    if qtype == ferros_dns::AAAA { 6 } else { 4 }
}

/// The cellular data call, as `rmnet` INFO gives it (modem.rs `rmnet_call`): flags (bit 0 up, bit 1 IPv4, bit 2 IPv6), MTU, addresses, the first DNS server of each family. The addresses are the line's: never printed.
#[derive(Clone, Copy)]
struct Cell {
    flags: u8,
    mtu: u16,
    v4: [u8; 4],
    v6: [u8; 16],
    dns4: [u8; 4],
    dns6: [u8; 16],
}
const CELL_DOWN: Cell = Cell { flags: 0, mtu: 0, v4: [0; 4], v6: [0; 16], dns4: [0; 4], dns6: [0; 16] };

/// Connections at once: sixteen (owner, 2026-11-11, the re-engineering's plan; two until then, and Mahere held one of them at a time). An idle one costs its `Tcb` alone.
const TCPN: usize = 1 << 4;
/// A connection's receive buffer: 1 MiB, the most its window says with scaling (2^20 >> `RCV_WS` fits the 16-bit field). At a 50 ms round trip that is 20 MB/s from one connection before the window, not the link, is what holds it back.
const RXB: usize = 1 << 20;
/// A connection's send buffer: 256 KiB (an upload's window in flight; the requests Mahere and the stower send are a few hundred bytes).
const TXB: usize = 1 << 18;
/// A connection's buffers together, one block of the arc: the receive buffer, then the send buffer.
const BLOCK: usize = RXB + TXB;
/// Our window scale shift: 5, the least that says 1 MiB in 16 bits.
const RCV_WS: u8 = 5;
/// The window a peer that offers no scaling is given: 65535 bytes of the buffer, the most an unscaled field says.
const NO_WS_WIN: u32 = 65535;
/// The largest payload a segment carries here: 1460, an Ethernet MTU of 1500 less IPv4's and TCP's headers (the packet buffers are 1500 bytes).
const MSS_MAX: usize = 1460;
/// The data a TCP RECV reply carries at most: 2^18, the kernel's largest reply (process.rs SYS_REPLY: `STORE_MAX`, 256 KiB, plus 8 for a store's value), the state byte in front.
const RECV_MAX: usize = 1 << 18;
/// The largest request: 4096 bytes, all a call carries (process.rs SYS_CALL reads at most 4096).
const MSG_MAX: usize = 1 << 12;
/// The heap's ceiling in the image header (`ferros_sys::entry!(…, grows HEAP_CEILING)`): 32 MiB, room for every connection's block above what the kernel maps at spawn, in either mode (masked arcs grow by doubling).
const HEAP_CEILING: usize = 1 << 25;
/// The heap the kernel maps after .bss at spawn for an image whose heap grows (process.rs `HEAP_START`): 4 MiB, three blocks.
const HEAP_START: usize = 4 << 20;
/// Retransmission expiries a connection survives: the eighth fails it (from 250 ms, doubling to the 8 s ceiling: about 30 s in all).
const TRIES: u32 = 1 << 3;
/// `blk` while a connection holds no block.
const NO_BLOCK: u8 = u8::MAX;
/// Segment flags.
const FIN: u8 = 1;
const SYN: u8 = 2;
const RST: u8 = 4;
const PSH: u8 = 8;
const ACK: u8 = 16;
/// Why a connection ended, for its report.
const WHY_CLOSED: u8 = 0;
const WHY_RESET: u8 = 1;
const WHY_TIMEOUT: u8 = 2;
const WHY_IDLE: u8 = 3;
const WHY_LINGER: u8 = 4;

/// One client connection (see the header): state 0 free, 1 SYN sent, 2 open, 3 the peer closed, 4 failed. It keeps the link and source address it opened with (`cell`, `src`), and its buffers' block while it is not free (`blk`).
/// The receive buffer is a ring: the `rx_len` bytes from `rx_head` are in order for the app, and the byte at sequence number s past `rcv_nxt` lives `s - rcv_nxt` past them (`Ooo` says which such ranges are there). The send buffer is a ring too: `tx_len` bytes from `tx_head`, the first of them at `snd_una`.
#[derive(Clone, Copy)]
struct Tcb {
    state: u8,
    fam: u8,
    cell: bool,
    src: [u8; 16],
    addr: [u8; 16],
    rport: u16,
    lport: u16,
    blk: u8,
    iss: u32,
    snd_una: u32,
    snd_nxt: u32,
    snd_max: u32,
    /// The peer's window in bytes (its field shifted by its scale), and the segment that last set it (RFC 9293 3.10.7.4, SND.WL1 and SND.WL2).
    snd_wnd: u32,
    wl1: u32,
    wl2: u32,
    /// The peer's scale shift and ours (both 0 unless both SYNs carried one), and whether the peer permits SACK.
    snd_ws: u8,
    rcv_ws: u8,
    sack_ok: bool,
    /// The segment size we send (the peer's MSS and our link's, the smaller) and the one we said we take.
    smss: u32,
    rmss: u32,
    rcv_nxt: u32,
    /// The right edge last advertised (`rcv_nxt` plus the window then), and the most the window may be (`RXB`, or `NO_WS_WIN` without scaling).
    rcv_adv: u32,
    rcv_total: u32,
    rx_head: u32,
    rx_len: u32,
    tx_head: u32,
    tx_len: u32,
    ooo: ferros_tcp::Ooo,
    /// In-order segments taken since our last ACK (the second is ACKed at once); an ACK owed at the end of the batch.
    unacked: u32,
    owed: bool,
    fin_queued: bool,
    fin_sent: bool,
    fin_ever: bool,
    fin_acked: bool,
    /// The peer's FIN: taken (`peer_fin`), or seen past a hole at `fin_seq` (`fin_in`) and taken once the data before it is.
    peer_fin: bool,
    fin_in: bool,
    fin_seq: u32,
    rtt: ferros_tcp::Rtt,
    /// When the retransmission timer fires (0: not running) and the expiries in a row.
    rto_t: u64,
    tries: u32,
    /// The segment being timed for a round trip (Karn: never a retransmitted one): the byte whose ACK ends it, and when it left.
    timing: bool,
    rtt_seq: u32,
    rtt_t: u64,
    cc: ferros_tcp::Cc,
    closed_t: u64,
    /// When its app last asked anything of it (connect, send, receive, close): an app that died or stopped asking leaves it to `tcp_tick`, which frees it 32 s on (2026-10-31: two stalled downloads, their apps killed, held both slots and every connect after was refused).
    touched: u64,
    /// Measures for the report (`report`): when it was asked for, when it opened, when data last came; bytes in (delivered in order) and out (acknowledged); segments resent (fast retransmits among them) and timer expiries; segments kept out of order and the most bytes so held; the most bytes waiting for the app; the peak rate over quarter seconds (bytes a second) and where its current quarter began.
    asked_t: u64,
    up_t: u64,
    last_in_t: u64,
    bytes_in: u64,
    bytes_out: u64,
    resent: u32,
    fast: u32,
    expiries: u32,
    ooo_segs: u32,
    ooo_most: u32,
    rx_most: u32,
    peak: u64,
    rate_t: u64,
    rate_b: u64,
    why: u8,
    /// Segments that carried data, and our ACKs with no data; the bytes taken in since our last ACK, and the most of them (`report`).
    segs_in: u32,
    acks_out: u32,
    since_ack: u32,
    burst_most: u32,
}
const TCB_FREE: Tcb = Tcb {
    state: 0,
    fam: 0,
    cell: false,
    src: [0; 16],
    addr: [0; 16],
    rport: 0,
    lport: 0,
    blk: NO_BLOCK,
    iss: 0,
    snd_una: 0,
    snd_nxt: 0,
    snd_max: 0,
    snd_wnd: 0,
    wl1: 0,
    wl2: 0,
    snd_ws: 0,
    rcv_ws: 0,
    sack_ok: false,
    smss: 0,
    rmss: 0,
    rcv_nxt: 0,
    rcv_adv: 0,
    rcv_total: 0,
    rx_head: 0,
    rx_len: 0,
    tx_head: 0,
    tx_len: 0,
    ooo: ferros_tcp::Ooo::EMPTY,
    unacked: 0,
    owed: false,
    fin_queued: false,
    fin_sent: false,
    fin_ever: false,
    fin_acked: false,
    peer_fin: false,
    fin_in: false,
    fin_seq: 0,
    rtt: ferros_tcp::Rtt::new(0),
    rto_t: 0,
    tries: 0,
    timing: false,
    rtt_seq: 0,
    rtt_t: 0,
    cc: ferros_tcp::Cc { cwnd: 0, ssthresh: 0, dup: 0, recover: 0, recovering: false },
    closed_t: 0,
    touched: 0,
    asked_t: 0,
    up_t: 0,
    last_in_t: 0,
    bytes_in: 0,
    bytes_out: 0,
    resent: 0,
    fast: 0,
    expiries: 0,
    ooo_segs: 0,
    ooo_most: 0,
    rx_most: 0,
    peak: 0,
    rate_t: 0,
    rate_b: 0,
    why: 0,
    segs_in: 0,
    acks_out: 0,
    since_ack: 0,
    burst_most: 0,
};

/// The connections' buffers, a block (`BLOCK`) each, from the arc (owner, 2026-11-11: taken when a connection opens, given back when it closes, from the arc grown on demand). Block k lives in the heap the kernel maps at spawn while k < `start`, else `k - start` blocks above the arc's top at spawn (`top0`, an arc offset), which the arc grows to reach (SYS_ARC, `ferros_sys::arc`). A connection takes the lowest free block, so the arc stays as low as it can, and when the highest blocks are free it shrinks back (the kernel rounds as the arc's mode needs: a masked arc keeps its power of two until it can halve).
/// A kernel without SYS_ARC maps the whole declared heap at spawn: every block is then in it, and nothing grows (`grows` false).
struct Blocks {
    heap: usize,
    start: usize,
    top0: usize,
    top: usize,
    grows: bool,
    /// Bit k: block k is a connection's.
    used: u32,
}

impl Blocks {
    /// Block k's address.
    fn addr(&self, k: u8) -> usize {
        let k = k as usize;
        if k < self.start { self.heap + k * BLOCK } else { ferros_sys::arc_base() as usize + self.top0 + (k - self.start) * BLOCK }
    }

    /// The arc's top that holds blocks `0..n`.
    fn need(&self, n: usize) -> usize {
        if n <= self.start { self.top0 } else { self.top0 + (n - self.start) * BLOCK }
    }

    /// The lowest free block, the arc grown to hold it when it lies above the top; None when every connection holds one or the kernel refuses the growth (its ceiling, or too few free frames).
    fn take(&mut self) -> Option<u8> {
        let k = (!self.used).trailing_zeros() as usize;
        if k >= TCPN {
            return None;
        }
        let need = self.need(k + 1);
        if need > self.top {
            if !self.grows {
                return None;
            }
            let (top, _) = ferros_sys::arc::grow(need - self.top).ok()?;
            self.top = top;
        }
        self.used |= 1 << k;
        Some(k as u8)
    }

    /// Block k given back, and the arc shrunk to the highest block still taken.
    fn give(&mut self, k: u8) {
        self.used &= !(1u32 << k);
        if !self.grows {
            return;
        }
        let want = self.need(32 - self.used.leading_zeros() as usize);
        if want < self.top {
            if let Ok((top, _)) = ferros_sys::arc::shrink(want) {
                self.top = top;
            }
        }
    }
}

/// `data` into the ring `buf` (a power of two long) from position `at`, wrapping at its end.
fn ring_put(buf: &mut [u8], at: usize, data: &[u8]) {
    let at = at & (buf.len() - 1);
    let first = data.len().min(buf.len() - at);
    buf[at..at + first].copy_from_slice(&data[..first]);
    buf[..data.len() - first].copy_from_slice(&data[first..]);
}

/// `out.len()` bytes of the ring `buf` (a power of two long) from position `at`, wrapping at its end.
fn ring_get(buf: &[u8], at: usize, out: &mut [u8]) {
    let at = at & (buf.len() - 1);
    let first = out.len().min(buf.len() - at);
    out[..first].copy_from_slice(&buf[at..at + first]);
    let rest = out.len() - first;
    out[first..].copy_from_slice(&buf[..rest]);
}

impl Ip {
    fn cap(&self, slot: u64) -> u64 {
        self.grants + 0x204 + 32 * slot
    }
    fn off<T>(&self, p: *const T) -> u64 {
        p as u64 - self.base
    }
    fn put(&mut self, s: &[u8]) {
        for &c in s {
            if self.n < self.log.len() {
                self.log[self.n] = c;
                self.n += 1;
            }
        }
    }
    fn flush(&mut self) {
        if self.n > 0 {
            let mut r = [0u8; 16];
            svc(CALL, self.cap(S_CONSOLE), self.off(self.log.as_ptr()), self.n as u64, self.off(r.as_mut_ptr()), 16);
            self.n = 0;
        }
    }
    fn v4_put(&mut self, a: [u8; 4]) {
        self.put(b"A#");
        for (k, b) in a.iter().enumerate() {
            if k > 0 {
                self.put(b".");
            }
            self.dec(*b as u64);
        }
    }
    fn dec(&mut self, mut v: u64) {
        let mut b = [0u8; 20];
        let mut i = b.len();
        loop {
            i -= 1;
            b[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        let d = b;
        self.put(&d[i..]);
    }
    /// G#fe80:0000:…: one base prefix, eight full groups.
    fn v6_put(&mut self, a: [u8; 16]) {
        let h = b"0123456789abcdef";
        self.put(b"G#");
        for g in 0..8 {
            if g > 0 {
                self.put(b":");
            }
            for k in 0..2 {
                let b = a[2 * g + k];
                self.put(&[h[(b >> 4) as usize], h[(b & 15) as usize]]);
            }
        }
    }

    /// A frame to the driver: into the batch for the next exchange (`batch`), else at once as [1][Ethernet frame].
    fn eth_send(&mut self, dst: [u8; 6], ty: u16, payload: &[u8]) {
        let mut f = [0u8; 1600];
        let n = 1 + 14 + payload.len();
        if n > f.len() {
            return;
        }
        f[0] = 1;
        f[1..7].copy_from_slice(&dst);
        f[7..13].copy_from_slice(&self.mac);
        f[13..15].copy_from_slice(&ty.to_be_bytes());
        f[15..n].copy_from_slice(payload);
        // WHY the floor: an Ethernet frame is 60 bytes at least (IEEE 802.3, without its FCS); a shorter one (an ARP, 42) is padded with zeros, as every host pads it.
        let n = n.max(1 + 60);
        if self.batch {
            // A full batch goes first, in an exchange that takes no frames back (they wait for the pass's own).
            if self.txn + 2 + n - 1 > self.txb.len() {
                let mut head = [0u8; EXCH_REPLY];
                self.exchange(&mut head);
            }
            let at = self.txn;
            self.txb[at..at + 2].copy_from_slice(&((n - 1) as u16).to_be_bytes());
            self.txb[at + 2..at + 1 + n].copy_from_slice(&f[1..n]);
            self.txn = at + 1 + n;
            return;
        }
        let mut r = [0u8; 16];
        let t0 = now();
        svc(CALL, self.cap(S_WLAN), self.off(f.as_ptr()), n as u64, self.off(r.as_mut_ptr()), 16 | CALL_LIVE);
        self.path_call(t0);
        self.path.frames_out += 1;
    }

    /// A call to the WiFi driver that started at `t0`, done: its time into the measures.
    fn path_call(&mut self, t0: u64) {
        let dt = now().wrapping_sub(t0);
        self.path.calls += 1;
        self.path.call_t += dt;
        // WHY the max: the high water, for the measure.
        self.path.call_most = self.path.call_most.max(dt);
    }

    /// One exchange with the WiFi driver (`wlan` op 8): the frames batched in `txb` out, and as much reply as `into` holds back (its `EXCH_REPLY`-byte header, then frames). Returns the reply's length: 0 from a driver without op 8, ERR when no driver serves `wlan` (WiFi goes down). The batch is empty after it either way.
    fn exchange(&mut self, into: &mut [u8]) -> u64 {
        let sent = self.txn > EXCH_HEAD;
        self.txb[0] = 8;
        self.txb[1..EXCH_HEAD].copy_from_slice(&(into.len() as u32).to_be_bytes());
        let t0 = now();
        let n = svc(CALL, self.cap(S_WLAN), self.off(self.txb.as_ptr()), self.txn as u64, self.off(into.as_mut_ptr()), into.len() as u64 | CALL_LIVE);
        if sent || (n != ERR && n > EXCH_REPLY as u64) {
            self.path_call(t0);
        }
        if sent {
            // The frames each carried their length: counted back out of the batch for the measure.
            let mut at = EXCH_HEAD;
            while at + 2 <= self.txn {
                at += 2 + u16::from_be_bytes([self.txb[at], self.txb[at + 1]]) as usize;
                self.path.frames_out += 1;
            }
            // WHY the test: the reply's header is the driver's word; a reply too short to hold it says nothing of refusals.
            if n != ERR && n >= EXCH_REPLY as u64 {
                self.path.refused += u16::from_be_bytes([into[1], into[2]]) as u32;
            }
        }
        self.txn = EXCH_HEAD;
        if n == ERR {
            self.wifi_down();
        }
        n
    }

    /// When the idle wait ends at the latest (it ends sooner on the WiFi doorbell, an application's call or a preference changing): the next idle millisecond (shifted by the screen-off backoff) while the cellular call is up, as `rmnet` is asked each pass until the modem rings a doorbell of its own; else the earliest of the protocol timers that are running: a connection's retransmit timer, a held receive's deadline, the resolver's tries and held lookups, DHCPv4's 2 s retransmit while unbound, the router solicitation's 4 s while IPv6 has no global address, the membership reports' 60 s while a group is joined, the WiFi driver looked for every 4 s while there is none, and the cellular call's INFO every second until it is up (both asks of a driver that has no doorbell for them yet). A time already past ends the wait at once, and the pass handles it. No backstop: until 2026-11-13 the wait also ended every 2^6 ms whatever was running, some 16 wakes a second for nothing.
    fn idle_until(&self, t0: u64) -> u64 {
        if self.cell.flags & 1 != 0 {
            return t0 + (self.freq / 1000 << self.backoff);
        }
        let f = self.freq;
        // The cellular call's INFO, every second until it is up.
        let mut t = self.cell_t + f;
        // WHY the mins: the earliest of the deadlines, not a bound on anyone's value.
        for c in self.tcb.iter().filter(|c| c.rto_t != 0) {
            t = t.min(c.rto_t);
        }
        for h in self.held.iter().filter(|h| h.used) {
            t = t.min(h.until);
        }
        // The resolver's: a try running out, a held RESOLVE's wait.
        for k in (0..RECN).filter(|&k| self.res.cache.r[k].asking) {
            t = t.min(self.res.asks[k].due);
        }
        for r in self.res.rq.iter().filter(|r| r.used) {
            t = t.min(r.until);
        }
        if self.wifi && !self.bound {
            t = t.min(self.dhcp_t + 2 * f);
        }
        if self.wifi && !self.have_global && self.rs_n < 3 {
            t = t.min(self.rs_t + 4 * f);
        }
        if self.groups_n > 0 {
            t = t.min(self.groups_t + 60 * f);
        }
        if !self.wifi {
            t = t.min(self.wifi_t + 4 * f);
        }
        t
    }

    /// The frames from the WiFi driver, a batch a call, while it says more wait or ACKs wait to go (`EXCH_PASS` calls at most): each taken (`wlan_in`), then the ACKs the batch owes (`tcp_acks`), which go with the next exchange. Returns whether any frame came or went.
    fn wlan_batch(&mut self) -> bool {
        // SAFETY: EXCH is the exchanges' reply buffer, used here alone (a batch flushed mid-pass by `eth_send` takes its reply in a buffer of its own), from the one thread.
        let r = unsafe { &mut *(&raw mut EXCH) };
        let mut moved = self.txn > EXCH_HEAD;
        for _ in 0..EXCH_PASS {
            let n = self.exchange(r);
            if n == ERR {
                break;
            }
            if n < EXCH_REPLY as u64 {
                // A driver without op 8 (replaced while this ran): one frame a call from now on.
                self.batch = false;
                self.put(b"WiFi driver answers one frame a call\n");
                break;
            }
            let n = n as usize;
            let more = r[0] & 1 != 0;
            let mut at = EXCH_REPLY;
            let mut k = 0u32;
            // Each frame its u16 BE length and bytes; a length that runs past the reply ends it (the reply is the driver's word).
            while at + 2 <= n {
                let l = u16::from_be_bytes([r[at], r[at + 1]]) as usize;
                at += 2;
                if at + l > n {
                    break;
                }
                self.wlan_in(&r[at..at + l]);
                at += l;
                k += 1;
            }
            self.path.frames_in += k;
            // WHY the max: the high water, for the measure.
            self.path.in_most = self.path.in_most.max(k);
            moved |= k > 0;
            self.tcp_acks();
            // Again while frames wait at the driver, or while this batch's ACKs wait here: the next exchange carries them out at once (and takes in what came meanwhile).
            if !more && self.txn == EXCH_HEAD {
                break;
            }
        }
        moved
    }

    /// One Ethernet frame from the WiFi driver: ARP, IPv4 or IPv6 by its EtherType, anything else ignored. WHY the length test: the frame is the driver's word; one without a whole Ethernet header is nothing.
    fn wlan_in(&mut self, f: &[u8]) {
        if f.len() < 14 {
            return;
        }
        self.frames += 1;
        let src_mac: [u8; 6] = f[6..12].try_into().unwrap();
        let ty = u16::from_be_bytes([f[12], f[13]]);
        if ty == 0x0806 {
            self.arp_in(&f[14..]);
        } else if ty == 0x0800 {
            self.ipv4_in(false, src_mac, &f[14..]);
        } else if ty == 0x86DD {
            self.ipv6_in(false, src_mac, &f[14..]);
        }
    }

    // ---- IPv4 ----

    fn ipv4_send(&mut self, dst_mac: [u8; 6], src: [u8; 4], dst: [u8; 4], proto: u8, payload: &[u8]) {
        self.ipv4_send_ttl(dst_mac, src, dst, proto, payload, HOP);
    }

    fn ipv4_send_ttl(&mut self, dst_mac: [u8; 6], src: [u8; 4], dst: [u8; 4], proto: u8, payload: &[u8], ttl: u8) {
        let mut p = [0u8; 1500];
        let total = ip4(&mut p, src, dst, proto, payload, ttl);
        if total != 0 {
            self.eth_send(dst_mac, 0x0800, &p[..total]);
        }
    }

    /// A packet up the cellular call: [1][IP packet]. A full UL queue drops it, as a full transmit ring would (TCP retransmits; a datagram is best effort).
    fn cell_send(&mut self, pkt: &[u8]) {
        let mut f = [0u8; 1 + 1500];
        f[0] = 1;
        f[1..1 + pkt.len()].copy_from_slice(pkt);
        let mut r = [0u8; 16];
        svc(CALL, self.cap(S_RMNET), self.off(f.as_ptr()), 1 + pkt.len() as u64, self.off(r.as_mut_ptr()), 16 | CALL_LIVE);
    }

    /// Ask the WiFi driver for its MAC and whether it is linked ([3] → MAC, linked; CALL_LIVE: an error at once when no driver serves `wlan`); linked, WiFi is up and DHCP and a router solicitation start. The driver answers before any join too (the network page, since 2026-11-09), unlinked and under its scanning MAC, which a join replaces: taken as up then, DHCP went out under an address the access point never associated and never bound, and the attest screen waited on a network that never came (owner, 2026-11-09: "No answer yet. Still looking").
    fn wifi_up(&mut self) {
        self.wifi_t = now();
        let mut info = [0u8; 16];
        let q = [3u8];
        let n = svc(CALL, self.cap(S_WLAN), self.off(q.as_ptr()), 1, self.off(info.as_mut_ptr()), 16 | CALL_LIVE);
        if n == ERR || n < 7 || info[6] == 0 {
            return;
        }
        self.mac.copy_from_slice(&info[..6]);
        self.wifi = true;
        // Whether it exchanges frames in batches (op 8): an empty exchange taking no frames back is answered with its 3-byte header, by a driver without the op with nothing.
        let q = [8u8, 0, 0, 0, EXCH_REPLY as u8];
        let mut head = [0u8; EXCH_REPLY];
        let n = svc(CALL, self.cap(S_WLAN), self.off(q.as_ptr()), q.len() as u64, self.off(head.as_mut_ptr()), EXCH_REPLY as u64 | CALL_LIVE);
        self.batch = n == EXCH_REPLY as u64;
        self.txn = EXCH_HEAD;
        self.put(if self.batch { b"WiFi driver answers, frames in batches\n".as_slice() } else { b"WiFi driver answers, one frame a call\n" });
        // The doorbell armed (`WIFI_RING`): its helper started the first time, asked again after a driver that went away.
        if WIFI_RING && self.batch && !self.ring_none {
            if self.ring_thread {
                RING.store(0, core::sync::atomic::Ordering::Release);
                ferros_sys::wake(&RING, 2);
            } else {
                // SAFETY: the helper's stack, given to it alone, once.
                let stack = unsafe { &mut *(&raw mut RING_STACK) };
                self.ring_thread = ferros_sys::thread_spawn(ring_thread, stack, self.grants) != ERR;
            }
            self.ring = self.ring_thread;
            self.put(if self.ring { b"WiFi doorbell armed: idle waits for frames, not the clock\n".as_slice() } else { b"WiFi doorbell: no thread, asking every idle millisecond\n" });
        }
        self.dhcp_state = 1;
        self.dhcp_n = DhcpCount::default();
        self.dhcp_send(1);
        self.rs_n = 0;
        self.router_solicit();
    }

    /// The WiFi driver stopped answering: its addresses go, so traffic takes the cellular call; `wifi_up` tries again every 4 s.
    fn wifi_down(&mut self) {
        self.wifi = false;
        self.wifi_t = now();
        self.bound = false;
        self.have_global = false;
        self.have_r6 = false;
        // The network's DNS servers go with it: a server kept from it would be asked from the next network (or the call) and answer nothing.
        self.dns = [0; 4];
        self.dns6 = [0; 16];
        self.arp_n = 0;
        self.nd_n = 0;
        // Frames batched for it go with it: nothing would send them, and a batch waiting keeps the loop from sleeping.
        self.txn = EXCH_HEAD;
        // The doorbell's held call failed with it (the helper waits, `RING` 2, until `wifi_up` re-arms it).
        self.ring = false;
        self.put(b"WiFi driver gone\n");
    }

    /// The cellular call's state from `rmnet` INFO (43 bytes: flags, MTU BE, IPv4, IPv6, DNS IPv4, DNS IPv6). No grant, or the call not up: down. A change is logged by its shape, never its addresses.
    fn cell_info(&mut self) {
        let mut r = [0u8; 43];
        let q = [3u8];
        let n = svc(CALL, self.cap(S_RMNET), self.off(q.as_ptr()), 1, self.off(r.as_mut_ptr()), r.len() as u64 | CALL_LIVE);
        let c = if n == r.len() as u64 && r[0] & 1 != 0 {
            Cell { flags: r[0], mtu: u16::from_be_bytes([r[1], r[2]]), v4: r[3..7].try_into().unwrap(), v6: r[7..23].try_into().unwrap(), dns4: r[23..27].try_into().unwrap(), dns6: r[27..43].try_into().unwrap() }
        } else {
            CELL_DOWN
        };
        let was = self.cell;
        self.cell = c;
        if c.flags != was.flags || c.v4 != was.v4 || c.v6 != was.v6 || c.mtu != was.mtu {
            if c.flags & 1 == 0 {
                self.put(b"cellular call down\n");
            } else {
                self.put(b"cellular call up: IPv4 ");
                self.put(if c.flags & 2 != 0 { b"yes" } else { b"no" });
                self.put(b", IPv6 ");
                self.put(if c.flags & 4 != 0 { b"yes" } else { b"no" });
                self.put(b", MTU A#");
                self.dec(c.mtu as u64);
                self.put(b"\n");
            }
        }
    }

    /// The link and source address for an IPv4 destination: WiFi once DHCP has bound it, else the cellular call if it has an IPv4 address; broadcast, multicast and the WiFi subnet's broadcast stay on WiFi. None: no address of this family on an allowed link.
    fn pick4(&self, dst: &[u8; 4]) -> Option<(bool, [u8; 4])> {
        let local = *dst == [255; 4] || dst[0] & 0xF0 == 0xE0 || (self.bound && (0..4).all(|k| dst[k] == self.v4[k] | !self.mask[k]));
        if self.mode != 1 && self.bound {
            return Some((false, self.v4));
        }
        if self.mode != 2 && !local && self.cell.flags & 3 == 3 {
            return Some((true, self.cell.v4));
        }
        None
    }

    /// The link and source address for an IPv6 destination: link-local on WiFi from our link-local address; a global or multicast destination from WiFi's global address once SLAAC has one, else a global destination from the cellular call's address. None: no address on an allowed link.
    fn pick6(&self, dst: &[u8; 16]) -> Option<(bool, [u8; 16])> {
        let link_local = dst[0] == 0xFE && dst[1] & 0xC0 == 0x80;
        if self.mode != 1 && self.wifi && link_local {
            return Some((false, self.ll));
        }
        if self.mode != 1 && self.have_global && !link_local {
            return Some((false, self.global));
        }
        if self.mode != 2 && !link_local && dst[0] != 0xFF && self.cell.flags & 5 == 5 {
            return Some((true, self.cell.v6));
        }
        None
    }

    /// IPv4 out on the link `pick4` chose: up the cellular call (TTL `HOP`; a packet over the call's MTU is dropped, the senders keep under it) or onto WiFi through the next hop. False while WiFi resolves the next hop (the caller retries).
    fn out4(&mut self, cell: bool, src: [u8; 4], dst: [u8; 4], proto: u8, payload: &[u8]) -> bool {
        if cell {
            let mut p = [0u8; 1500];
            let n = ip4(&mut p, src, dst, proto, payload, HOP);
            if n != 0 && n <= self.cell.mtu as usize {
                self.cell_send(&p[..n]);
            }
            return true;
        }
        let Some(mac) = self.next_hop4(dst) else { return false };
        self.ipv4_send(mac, src, dst, proto, payload);
        true
    }

    /// IPv6 out on the link `pick6` chose, as `out4`.
    fn out6(&mut self, cell: bool, src: [u8; 16], dst: [u8; 16], nh: u8, payload: &[u8], hop: u8) -> bool {
        if cell {
            let mut p = [0u8; 1500];
            let n = ip6(&mut p, src, dst, nh, payload, hop);
            if n != 0 && n <= self.cell.mtu as usize {
                self.cell_send(&p[..n]);
            }
            return true;
        }
        let Some(mac) = self.next_hop6(dst) else { return false };
        self.ipv6_send(mac, src, dst, nh, payload, hop);
        true
    }

    /// The MAC for an IPv4 destination: broadcast (limited or this subnet's), a multicast group's (01:00:5e and the group's low 23 bits, as the IGMP report already sends), a cached neighbour (on-link) or the router's; None after sending an ARP request (the caller retries).
    /// Until 2026-11-10 only 255.255.255.255 counted as broadcast and multicast went to the router: the Pro ARPed for 192.168.1.255 at every beacon and never sent one, and its group beacons reached the router alone, so desktop Photon never heard it (seen in a capture on the desktop's WiFi).
    fn next_hop4(&mut self, dst: [u8; 4]) -> Option<[u8; 6]> {
        if dst == [255; 4] || (self.bound && (0..4).all(|k| dst[k] == self.v4[k] | !self.mask[k])) {
            return Some([0xFF; 6]);
        }
        if dst[0] & 0xF0 == 0xE0 {
            return Some([0x01, 0x00, 0x5E, dst[1] & 0x7F, dst[2], dst[3]]);
        }
        let on_link = (0..4).all(|k| dst[k] & self.mask[k] == self.v4[k] & self.mask[k]);
        let hop = if on_link { dst } else { self.router };
        if let Some(m) = self.arp[..self.arp_n].iter().find(|e| e.0 == hop).map(|e| e.1) {
            return Some(m);
        }
        let mut a = [0u8; 28];
        a[0..8].copy_from_slice(&[0, 1, 8, 0, 6, 4, 0, 1]);
        a[8..14].copy_from_slice(&self.mac);
        a[14..18].copy_from_slice(&self.v4);
        a[24..28].copy_from_slice(&hop);
        self.eth_send([0xFF; 6], 0x0806, &a);
        None
    }

    fn arp_learn(&mut self, ip: [u8; 4], mac: [u8; 6]) {
        if let Some(e) = self.arp[..self.arp_n].iter_mut().find(|e| e.0 == ip) {
            e.1 = mac;
        } else if self.arp_n < self.arp.len() {
            self.arp[self.arp_n] = (ip, mac);
            self.arp_n += 1;
        } else {
            self.arp[0] = (ip, mac);
        }
    }

    fn arp_in(&mut self, a: &[u8]) {
        if a.len() < 28 || a[0..6] != [0, 1, 8, 0, 6, 4] {
            return;
        }
        let sha: [u8; 6] = a[8..14].try_into().unwrap();
        let spa: [u8; 4] = a[14..18].try_into().unwrap();
        let tpa: [u8; 4] = a[24..28].try_into().unwrap();
        if spa != [0; 4] {
            self.arp_learn(spa, sha);
        }
        if a[7] == 1 && self.bound && tpa == self.v4 {
            let mut r = [0u8; 28];
            r[0..8].copy_from_slice(&[0, 1, 8, 0, 6, 4, 0, 2]);
            r[8..14].copy_from_slice(&self.mac);
            r[14..18].copy_from_slice(&self.v4);
            r[18..24].copy_from_slice(&sha);
            r[24..28].copy_from_slice(&spa);
            self.eth_send(sha, 0x0806, &r);
        }
    }

    /// An IPv4 packet in from WiFi (`cell` false: `src_mac` its sender) or from the cellular call (`cell` true: a raw IP packet, `src_mac` unused).
    fn ipv4_in(&mut self, cell: bool, src_mac: [u8; 6], p: &[u8]) {
        if p.len() < 20 || p[0] >> 4 != 4 {
            return;
        }
        let ihl = (p[0] & 15) as usize * 4;
        let total = (u16::from_be_bytes([p[2], p[3]]) as usize).min(p.len());
        // WHY: the header length is the sender's; under 20 the body would start inside the header, past the total it would be no packet.
        if ihl < 20 || total < ihl {
            return;
        }
        let src: [u8; 4] = p[12..16].try_into().unwrap();
        let dst: [u8; 4] = p[16..20].try_into().unwrap();
        // Unicast to us: WiFi's bound address, or the call's on the cellular link.
        let mine = if cell { self.cell.flags & 3 == 3 && dst == self.cell.v4 } else { self.bound && dst == self.v4 };
        // Ours: our address, or on WiFi broadcast (limited or the subnet's) or IPv4 multicast (224/4: Photon's LAN beacon group 239.104.199.144; the firmware passes all multicast, and only opened ports keep a datagram).
        let ours = mine || (!cell && (dst == [255; 4] || (self.bound && (0..4).all(|k| dst[k] == self.v4[k] | !self.mask[k])) || dst[0] & 0xF0 == 0xE0));
        let body = &p[ihl..total];
        match p[9] {
            // ICMP echo request → reply up the call, or to the sender's MAC on WiFi (no resolution needed).
            1 if mine && body.len() >= 8 && body[0] == 8 && cell => {
                let mut r = [0u8; 1480];
                let n = body.len().min(r.len());
                r[..n].copy_from_slice(&body[..n]);
                r[0] = 0;
                r[2] = 0;
                r[3] = 0;
                let c = csum(&r[..n], 0);
                r[2..4].copy_from_slice(&c.to_be_bytes());
                self.out4(true, dst, src, 1, &r[..n]);
            }
            1 if mine && body.len() >= 8 && body[0] == 8 => {
                let mut r = [0u8; 1480];
                let n = body.len().min(r.len());
                r[..n].copy_from_slice(&body[..n]);
                r[0] = 0;
                r[2] = 0;
                r[3] = 0;
                let c = csum(&r[..n], 0);
                r[2..4].copy_from_slice(&c.to_be_bytes());
                let me = self.v4;
                self.ipv4_send(src_mac, me, src, 1, &r[..n]);
                self.arp_learn(src, src_mac);
                self.put(b"ping from ");
                self.v4_put(src);
                self.put(b"\n");
            }
            6 if mine && body.len() >= 20 => {
                let (mut a, mut d) = ([0u8; 16], [0u8; 16]);
                a[..4].copy_from_slice(&src);
                d[..4].copy_from_slice(&dst);
                self.tcp_in(4, a, d, body);
            }
            17 if body.len() >= 8 => {
                let sport = u16::from_be_bytes([body[0], body[1]]);
                let dport = u16::from_be_bytes([body[2], body[3]]);
                let ulen = (u16::from_be_bytes([body[4], body[5]]) as usize).clamp(8, body.len());
                if dport == 68 && !cell {
                    self.dhcp_in(&body[8..ulen]);
                } else if ours {
                    let mut a = [0u8; 16];
                    a[..4].copy_from_slice(&src);
                    self.queue(4, a, sport, dport, &body[8..ulen]);
                }
            }
            _ => {}
        }
    }

    // ---- DHCPv4 ----

    fn dhcp_send(&mut self, ty: u8) {
        let mut b = [0u8; 300];
        b[0..4].copy_from_slice(&[1, 1, 6, 0]);
        b[4..8].copy_from_slice(&self.xid.to_be_bytes());
        b[10] = 0x80;
        b[28..34].copy_from_slice(&self.mac);
        b[236..240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]);
        let mut o = 240;
        b[o..o + 3].copy_from_slice(&[53, 1, ty]);
        o += 3;
        if ty == 3 {
            b[o..o + 2].copy_from_slice(&[50, 4]);
            b[o + 2..o + 6].copy_from_slice(&self.offer);
            b[o + 6..o + 8].copy_from_slice(&[54, 4]);
            b[o + 8..o + 12].copy_from_slice(&self.server);
            o += 12;
        }
        b[o..o + 5].copy_from_slice(&[55, 3, 1, 3, 6]);
        b[o + 5] = 255;
        let mut u = [0u8; 308];
        u[0..8].copy_from_slice(&[0, 68, 0, 67, (308u16 >> 8) as u8, 308u16 as u8, 0, 0]);
        u[8..].copy_from_slice(&b);
        self.ipv4_send([0xFF; 6], [0; 4], [255; 4], 17, &u);
        self.dhcp_t = now();
        if ty == 3 {
            self.dhcp_n.requests += 1;
        } else {
            self.dhcp_n.discovers += 1;
        }
    }

    /// DHCP's line: `DHCP <what>: A#<n> DISCOVERs and A#<n> REQUESTs out; A#<n> OFFERs, A#<n> ACKs, A#<n> NAKs in, A#<n> replies to other transactions`, then the state (1 discovering, 2 requesting, 3 bound).
    fn dhcp_say(&mut self, what: &[u8]) {
        let c = self.dhcp_n;
        self.put(b"DHCP ");
        self.put(what);
        self.put(b": A#");
        self.dec(c.discovers as u64);
        self.put(b" DISCOVERs and A#");
        self.dec(c.requests as u64);
        self.put(b" REQUESTs out; A#");
        self.dec(c.offers as u64);
        self.put(b" OFFERs, A#");
        self.dec(c.acks as u64);
        self.put(b" ACKs, A#");
        self.dec(c.naks as u64);
        self.put(b" NAKs in, A#");
        self.dec(c.others as u64);
        self.put(b" replies to other transactions; state A#");
        self.dec(self.dhcp_state as u64);
        if self.bound {
            self.put(b", lease A#");
            self.dec(c.lease_s as u64);
            self.put(b" s");
        }
        self.put(b"\n");
    }

    fn dhcp_in(&mut self, b: &[u8]) {
        if b.len() < 240 || b[0] != 2 {
            return;
        }
        if u32::from_be_bytes([b[4], b[5], b[6], b[7]]) != self.xid {
            self.dhcp_n.others += 1;
            return;
        }
        let yiaddr: [u8; 4] = b[16..20].try_into().unwrap();
        let (mut ty, mut server, mut mask, mut router) = (0u8, [0u8; 4], [0u8; 4], [0u8; 4]);
        let mut o = 240;
        while o + 1 < b.len() {
            let code = b[o];
            if code == 255 {
                break;
            }
            if code == 0 {
                o += 1;
                continue;
            }
            let l = b[o + 1] as usize;
            if o + 2 + l > b.len() {
                break;
            }
            let v = &b[o + 2..o + 2 + l];
            if l >= 4 {
                let v4: [u8; 4] = v[..4].try_into().unwrap();
                if code == 54 {
                    server = v4;
                } else if code == 1 {
                    mask = v4;
                } else if code == 3 {
                    router = v4;
                } else if code == 6 {
                    self.dns = v4;
                } else if code == 51 {
                    self.dhcp_n.lease_s = u32::from_be_bytes(v4);
                }
            }
            if code == 53 && l >= 1 {
                ty = v[0];
            }
            o += 2 + l;
        }
        match ty {
            2 => self.dhcp_n.offers += 1,
            5 => self.dhcp_n.acks += 1,
            6 => self.dhcp_n.naks += 1,
            _ => {}
        }
        if ty == 2 && self.dhcp_state == 1 {
            self.offer = yiaddr;
            self.server = server;
            self.dhcp_state = 2;
            self.dhcp_n.unanswered = 0;
            self.dhcp_send(3);
        } else if ty == 5 && self.dhcp_state == 2 {
            self.v4 = yiaddr;
            self.mask = mask;
            self.router = router;
            self.bound = true;
            self.dhcp_state = 3;
            self.put(b"IPv4 ");
            self.v4_put(yiaddr);
            self.put(b" mask ");
            self.v4_put(mask);
            self.put(b" router ");
            self.v4_put(router);
            self.put(b"\n");
            self.dhcp_say(b"bound");
        } else if ty == 6 {
            self.dhcp_state = 1;
            self.dhcp_send(1);
        }
    }

    /// DHCP's retransmit, every 2 s while WiFi is up and unbound (the main loop's timer): a REQUEST again while requesting, four in a row unanswered giving the offer up for a fresh DISCOVER (RFC 2131 4.4.1; until 2026-11-13 a REQUEST for an offer the server had forgotten went out every 2 s forever), else a DISCOVER. Every 16th says the measures (`dhcp_say`), so a network that gives no lease says why.
    fn dhcp_retry(&mut self) {
        if self.dhcp_state == 2 {
            self.dhcp_n.unanswered += 1;
            if self.dhcp_n.unanswered >= 4 {
                self.dhcp_state = 1;
            }
        }
        let ty = if self.dhcp_state == 2 { 3 } else { 1 };
        self.dhcp_send(ty);
        if (self.dhcp_n.discovers + self.dhcp_n.requests) & 15 == 0 {
            self.dhcp_say(b"no lease yet");
        }
    }

    // ---- IPv6 ----

    fn ipv6_send(&mut self, dst_mac: [u8; 6], src: [u8; 16], dst: [u8; 16], nh: u8, payload: &[u8], hop: u8) {
        let mut p = [0u8; 1500];
        let total = ip6(&mut p, src, dst, nh, payload, hop);
        if total != 0 {
            self.eth_send(dst_mac, 0x86DD, &p[..total]);
        }
    }

    /// ICMPv6 / UDP over IPv6: the checksum covers a pseudo-header (source, destination, upper-layer length, next header).
    fn sum6(src: &[u8; 16], dst: &[u8; 16], nh: u8, data: &[u8]) -> u16 {
        let mut ph = [0u8; 40];
        ph[..16].copy_from_slice(src);
        ph[16..32].copy_from_slice(dst);
        ph[32..36].copy_from_slice(&(data.len() as u32).to_be_bytes());
        ph[39] = nh;
        csum(data, sum(&ph))
    }

    /// An ICMPv6 message at hop limit 255, as neighbour discovery requires (RFC 4861: a receiver drops ND with any other).
    fn icmp6_send(&mut self, dst_mac: [u8; 6], src: [u8; 16], dst: [u8; 16], msg: &mut [u8]) {
        self.icmp6_send_hop(dst_mac, src, dst, msg, 255);
    }

    fn icmp6_send_hop(&mut self, dst_mac: [u8; 6], src: [u8; 16], dst: [u8; 16], msg: &mut [u8], hop: u8) {
        msg[2] = 0;
        msg[3] = 0;
        let c = Self::sum6(&src, &dst, 58, msg);
        msg[2..4].copy_from_slice(&c.to_be_bytes());
        self.ipv6_send(dst_mac, src, dst, 58, msg, hop);
    }

    fn multicast_mac(dst: &[u8; 16]) -> [u8; 6] {
        [0x33, 0x33, dst[12], dst[13], dst[14], dst[15]]
    }

    /// Tell the network we listen to a multicast group: IGMPv2 membership report (type G#16, to the group, TTL 1) or MLDv1 listener report (ICMPv6 131 from our link-local, hop limit 1, behind the Router Alert hop-by-hop option RFC 2710 asks for).
    fn group_report(&mut self, fam: u8, group: [u8; 16]) {
        if fam == 4 {
            if !self.bound {
                return;
            }
            let g: [u8; 4] = group[..4].try_into().unwrap();
            let mut r = [0x16u8, 0, 0, 0, g[0], g[1], g[2], g[3]];
            let c = csum(&r, 0);
            r[2..4].copy_from_slice(&c.to_be_bytes());
            let mac = [0x01, 0x00, 0x5E, g[1] & 0x7F, g[2], g[3]];
            let me = self.v4;
            self.ipv4_send_ttl(mac, me, g, 2, &r, 1);
        } else {
            // Hop-by-hop header (next 58, one 8-byte unit): Router Alert (type 5, length 2, value 0 = MLD), PadN of two.
            let mut p = [0u8; 8 + 24];
            p[0] = 58;
            p[2..8].copy_from_slice(&[5, 2, 0, 0, 1, 0]);
            let m = &mut p[8..];
            m[0] = 131;
            m[8..24].copy_from_slice(&group);
            let ll = self.ll;
            let c = Self::sum6(&ll, &group, 58, &p[8..]);
            p[10..12].copy_from_slice(&c.to_be_bytes());
            self.ipv6_send(Self::multicast_mac(&group), ll, group, 0, &p, 1);
        }
    }

    fn group_reports(&mut self) {
        for i in 0..self.groups_n {
            let (fam, g) = self.groups[i];
            self.group_report(fam, g);
        }
    }

    fn router_solicit(&mut self) {
        let mut m = [0u8; 16];
        m[0] = 133;
        m[8] = 1;
        m[9] = 1;
        m[10..16].copy_from_slice(&self.mac);
        let dst = [0xFF, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2];
        let ll = self.ll;
        self.icmp6_send(Self::multicast_mac(&dst), ll, dst, &mut m);
        self.rs_t = now();
        self.rs_n += 1;
    }

    fn is_ours6(&self, a: &[u8; 16]) -> bool {
        *a == self.ll || (self.have_global && *a == self.global)
    }

    fn nd_learn(&mut self, ip: [u8; 16], mac: [u8; 6]) {
        if let Some(e) = self.nd[..self.nd_n].iter_mut().find(|e| e.0 == ip) {
            e.1 = mac;
        } else if self.nd_n < self.nd.len() {
            self.nd[self.nd_n] = (ip, mac);
            self.nd_n += 1;
        } else {
            self.nd[0] = (ip, mac);
        }
    }

    /// The MAC for an IPv6 destination: multicast mapping, a cached neighbour, or the router for a global address; None after sending a neighbour solicitation (the caller retries).
    fn next_hop6(&mut self, dst: [u8; 16]) -> Option<[u8; 6]> {
        if dst[0] == 0xFF {
            return Some(Self::multicast_mac(&dst));
        }
        if let Some(m) = self.nd[..self.nd_n].iter().find(|e| e.0 == dst).map(|e| e.1) {
            return Some(m);
        }
        let link_local = dst[0] == 0xFE && dst[1] & 0xC0 == 0x80;
        if !link_local && self.have_r6 {
            return Some(self.r6_mac);
        }
        // Neighbour solicitation to the target's solicited-node group ff02::1:ffXX:XXXX.
        let mut sn = [0xFF, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0xFF, 0, 0, 0];
        sn[13..16].copy_from_slice(&dst[13..16]);
        let mut m = [0u8; 32];
        m[0] = 135;
        m[8..24].copy_from_slice(&dst);
        m[24] = 1;
        m[25] = 1;
        m[26..32].copy_from_slice(&self.mac);
        let src = if link_local || !self.have_global { self.ll } else { self.global };
        self.icmp6_send(Self::multicast_mac(&sn), src, sn, &mut m);
        None
    }

    /// An IPv6 packet in from WiFi (`cell` false: `src_mac` its sender) or from the cellular call (`cell` true: a raw IP packet, only unicast to the call's address taken; neighbour discovery and router advertisements are WiFi's).
    fn ipv6_in(&mut self, cell: bool, src_mac: [u8; 6], p: &[u8]) {
        if p.len() < 40 || p[0] >> 4 != 6 {
            return;
        }
        let plen = (u16::from_be_bytes([p[4], p[5]]) as usize).min(p.len() - 40);
        let nh = p[6];
        let src: [u8; 16] = p[8..24].try_into().unwrap();
        let dst: [u8; 16] = p[24..40].try_into().unwrap();
        let body = &p[40..40 + plen];
        let multicast = dst[0] == 0xFF;
        if cell {
            if self.cell.flags & 5 != 5 || dst != self.cell.v6 {
                return;
            }
            if nh == 58 && body.len() >= 8 && body[0] == 128 {
                // Echo request → reply up the call, from the address it was sent to.
                let mut r = [0u8; 1452];
                let n = body.len().min(r.len());
                r[..n].copy_from_slice(&body[..n]);
                r[0] = 129;
                r[2] = 0;
                r[3] = 0;
                let c = Self::sum6(&dst, &src, 58, &r[..n]);
                r[2..4].copy_from_slice(&c.to_be_bytes());
                self.out6(true, dst, src, 58, &r[..n], HOP);
            } else if nh == 6 && body.len() >= 20 {
                self.tcp_in(6, src, dst, body);
            } else if nh == 17 && body.len() >= 8 {
                let sport = u16::from_be_bytes([body[0], body[1]]);
                let dport = u16::from_be_bytes([body[2], body[3]]);
                let ulen = (u16::from_be_bytes([body[4], body[5]]) as usize).clamp(8, body.len());
                self.queue(6, src, sport, dport, &body[8..ulen]);
            }
            return;
        }
        if !multicast && !self.is_ours6(&dst) {
            return;
        }
        if nh == 58 && body.len() >= 8 {
            let ty = body[0];
            if ty == 128 && !multicast {
                // Echo request → reply from the address it was sent to, to the sender's MAC.
                let mut r = [0u8; 1452];
                let n = body.len().min(r.len());
                r[..n].copy_from_slice(&body[..n]);
                r[0] = 129;
                self.icmp6_send_hop(src_mac, dst, src, &mut r[..n], HOP);
                self.nd_learn(src, src_mac);
                self.put(b"ping6 from ");
                self.v6_put(src);
                self.put(b"\n");
            } else if ty == 134 && p[7] == 255 && src[0] == 0xFE && src[1] & 0xC0 == 0x80 {
                // WHY the checks (RFC 4861 6.1.2): an advertisement comes from a router's link-local address at hop limit 255, which no packet forwarded from off the link can have; it now names the DNS server too, so one from anywhere else is not believed.
                self.router_adv(src_mac, src, body);
            } else if ty == 135 && body.len() >= 24 {
                let target: [u8; 16] = body[8..24].try_into().unwrap();
                if self.is_ours6(&target) {
                    // Neighbour advertisement: solicited + override, our MAC as the target link-layer address; to the solicitor, or all-nodes if it had no address yet.
                    let unspecified = src == [0; 16];
                    let (to, to_mac) = if unspecified { ([0xFF, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1], [0x33, 0x33, 0, 0, 0, 1]) } else { (src, src_mac) };
                    let mut m = [0u8; 32];
                    m[0] = 136;
                    m[4] = if unspecified { 0x20 } else { 0x60 };
                    m[8..24].copy_from_slice(&target);
                    m[24] = 2;
                    m[25] = 1;
                    m[26..32].copy_from_slice(&self.mac);
                    self.icmp6_send(to_mac, target, to, &mut m);
                    if !unspecified {
                        self.nd_learn(src, src_mac);
                    }
                }
            } else if ty == 136 && body.len() >= 24 {
                let target: [u8; 16] = body[8..24].try_into().unwrap();
                let mut mac = src_mac;
                let mut o = 24;
                while o + 8 <= body.len() {
                    let (t, l) = (body[o], body[o + 1] as usize * 8);
                    if l == 0 {
                        break;
                    }
                    if t == 2 && l >= 8 {
                        mac = body[o + 2..o + 8].try_into().unwrap();
                    }
                    o += l;
                }
                self.nd_learn(target, mac);
            }
        } else if nh == 6 && body.len() >= 20 && !multicast {
            self.tcp_in(6, src, dst, body);
        } else if nh == 17 && body.len() >= 8 && (!multicast || dst[1] & 15 == 2) {
            let sport = u16::from_be_bytes([body[0], body[1]]);
            let dport = u16::from_be_bytes([body[2], body[3]]);
            let ulen = (u16::from_be_bytes([body[4], body[5]]) as usize).clamp(8, body.len());
            self.nd_learn(src, src_mac);
            self.queue(6, src, sport, dport, &body[8..ulen]);
        }
    }

    /// Router advertisement: options 1 (the router's MAC) and 3 (prefix information: length +2, flags +3 with A = G#40, prefix +16). A /64 with A set → the global address = prefix + our interface identifier (SLAAC).
    fn router_adv(&mut self, src_mac: [u8; 6], src: [u8; 16], body: &[u8]) {
        let mut mac = src_mac;
        let mut o = 16;
        let mut prefix = None;
        while o + 8 <= body.len() {
            let (t, l) = (body[o], body[o + 1] as usize * 8);
            if l == 0 || o + l > body.len() {
                break;
            }
            if t == 1 {
                mac = body[o + 2..o + 8].try_into().unwrap();
            } else if t == 25 && l >= 24 {
                // RDNSS (RFC 8106): reserved (2), lifetime (4), then the servers; the first kept, a lifetime of 0 withdrawing it. Without it a network with no DHCPv4 had no DNS server at all, and IPv6 questions went to the call's server from WiFi's address.
                let life = u32::from_be_bytes(body[o + 4..o + 8].try_into().unwrap());
                let s: [u8; 16] = body[o + 8..o + 24].try_into().unwrap();
                if life == 0 {
                    if s == self.dns6 {
                        self.dns6 = [0; 16];
                    }
                } else if s != self.dns6 {
                    self.dns6 = s;
                    self.put(b"IPv6 DNS server from the router\n");
                }
            } else if t == 3 && l >= 32 && body[o + 2] == 64 && body[o + 3] & 0x40 != 0 {
                let p: [u8; 8] = body[o + 16..o + 24].try_into().unwrap();
                if p[0] & 0xE0 == 0x20 {
                    prefix = Some(p);
                }
            }
            o += l;
        }
        self.r6 = src;
        self.r6_mac = mac;
        self.have_r6 = true;
        self.nd_learn(src, mac);
        if let Some(p) = prefix {
            let mut g = [0u8; 16];
            g[..8].copy_from_slice(&p);
            g[8..].copy_from_slice(&self.iid);
            if !self.have_global || g != self.global {
                self.global = g;
                self.have_global = true;
                self.put(b"IPv6 global ");
                self.v6_put(g);
                self.put(b" via router ");
                self.v6_put(src);
                self.put(b"\n");
            }
        }
    }

    // ---- UDP for applications ----

    fn queue(&mut self, fam: u8, addr: [u8; 16], sport: u16, dport: u16, data: &[u8]) {
        // The resolver's port is its own: its answers are read here, never queued for an application.
        if dport == self.res.port {
            self.dns_in(fam, addr, sport, data);
            return;
        }
        if dport == 0 || !self.ports.contains(&dport) {
            return;
        }
        // A free slot, else the oldest datagram's.
        let slot = match self.q.iter().position(|d| !d.used) {
            Some(i) => i,
            None => (0..QN).min_by_key(|&i| self.q[i].seq).unwrap_or(0),
        };
        self.qseq = self.qseq.wrapping_add(1);
        let seq = self.qseq;
        let d = &mut self.q[slot];
        d.seq = seq;
        let n = data.len().min(d.data.len());
        d.used = true;
        d.fam = fam;
        d.addr = addr;
        d.sport = sport;
        d.dport = dport;
        d.len = n;
        d.data[..n].copy_from_slice(&data[..n]);
    }

    /// Keep datagrams for `port` from now on (one of the last eight opened).
    fn open_port(&mut self, port: u16) {
        if !self.ports.contains(&port) {
            self.ports[self.ports_at] = port;
            self.ports_at = (self.ports_at + 1) % self.ports.len();
        }
    }

    fn udp_send(&mut self, fam: u8, addr: [u8; 16], dport: u16, sport: u16, payload: &[u8]) -> u8 {
        let mut u = [0u8; 1480];
        let n = 8 + payload.len();
        if n > 1460 {
            return 3;
        }
        u[0..2].copy_from_slice(&sport.to_be_bytes());
        u[2..4].copy_from_slice(&dport.to_be_bytes());
        u[4..6].copy_from_slice(&(n as u16).to_be_bytes());
        u[8..n].copy_from_slice(payload);
        // A datagram that would not fit the cellular call's MTU is refused (3), not cut: there is no fragmentation here.
        if fam == 4 {
            let dst: [u8; 4] = addr[..4].try_into().unwrap();
            let Some((cell, src)) = self.pick4(&dst) else { return 2 };
            if cell && 20 + n > self.cell.mtu as usize {
                return 3;
            }
            if self.out4(cell, src, dst, 17, &u[..n]) { 0 } else { 1 }
        } else if fam == 6 {
            let Some((cell, src)) = self.pick6(&addr) else { return 2 };
            if cell && 40 + n > self.cell.mtu as usize {
                return 3;
            }
            let c = Self::sum6(&src, &addr, 17, &u[..n]);
            let c = if c == 0 { 0xFFFF } else { c };
            u[6..8].copy_from_slice(&c.to_be_bytes());
            if self.out6(cell, src, addr, 17, &u[..n], HOP) { 0 } else { 1 }
        } else {
            3
        }
    }

    // ---- The resolver (op 10 RESOLVE) ----

    /// For each family, the DNS server of the link it leaves on (`pick4`, `pick6` for a global destination): a server is asked from that link's address (the call's IPv6 server was reported even when IPv6 left on WiFi, a carrier's server asked from an address outside its network, which it need not answer). Zeros for a family with no link, or whose link named none. INFO says these, and the resolver asks them.
    fn dns_servers(&self) -> ([u8; 4], [u8; 16]) {
        let d4 = match self.pick4(&[8, 8, 8, 8]) {
            Some((true, _)) => self.cell.dns4,
            Some((false, _)) => self.dns,
            None => [0; 4],
        };
        let d6 = match self.pick6(&[0x20, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]) {
            Some((true, _)) => self.cell.dns6,
            Some((false, _)) => self.dns6,
            None => [0; 16],
        };
        (d4, d6)
    }

    /// Two random bytes from the entropy grant (a question's id); None without the grant.
    fn rand16(&self) -> Option<u16> {
        let mut b = [0u8; 2];
        (svc(ENTROPY_SYS, self.cap(S_ENTROPY), self.off(b.as_mut_ptr()), 2, 0, 0) == 2).then_some(u16::from_le_bytes(b))
    }

    /// Op 10: [10, family, wait, name…]. The answer, or None when the call is held until the lookup ends (`res_end`) or the wait runs out (`dns_tick`).
    fn resolve(&mut self, m: &[u8], tok: &[u8; 32], out: &mut [u8]) -> Option<usize> {
        let now = now();
        // WHY the checks: the request is the caller's; a family other than 0, 4 or 6, or a name that is not a host name, is a bad request.
        let name = if m.len() >= 4 && (m[1] == 0 || m[1] == 4 || m[1] == 6) { ferros_dns::Name::from_host(&m[3..]) } else { None };
        let Some(name) = name else { return Some(res_put(out, res_answer(RES_BAD, 0, [0; 16], 0))) };
        let (want, wait) = (m[1], m[2]);
        // The types to ask, in order: the family asked for; for either, IPv6 first where it has a route (owner, 2026-10-29: IPv6 preferred), then IPv4 where it has one.
        let mut r = Rq { used: true, tok: *tok, name, until: now + wait as u64 * self.freq, ..RQ_FREE };
        if want == 6 || (want == 0 && self.pick6(&[0x20, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]).is_some()) {
            r.order[r.n] = ferros_dns::AAAA;
            r.n += 1;
        }
        if want == 4 || (want == 0 && self.pick4(&[8, 8, 8, 8]).is_some()) {
            r.order[r.n] = ferros_dns::A;
            r.n += 1;
        }
        if let Some(a) = self.res_run(&mut r, now) {
            return Some(res_put(out, a));
        }
        // A lookup is under way: the caller asked for the cache alone (wait 0: the last good address when there is one, else "still asking"), or it is held.
        if wait == 0 {
            let a = match self.res.cache.look(r.rec, now, self.freq) {
                ferros_dns::Look::Stale { addr } => res_answer(RES_FOUND, fam_of(r.order[r.phase]), addr, 0),
                _ => res_answer(RES_ASKING, 0, [0; 16], 0),
            };
            return Some(res_put(out, a));
        }
        let k = match self.res.rq.iter().position(|q| !q.used) {
            Some(k) => k,
            None => {
                // Every slot taken: the oldest is answered as things stand (its lookup goes on).
                let k = (0..RQN).min_by_key(|&k| self.res.rq[k].seq).unwrap();
                let old = self.res.rq[k].tok;
                self.res_reply(&old, &res_answer(RES_ASKING, 0, [0; 16], 0));
                k
            }
        };
        self.res.rq_seq = self.res.rq_seq.wrapping_add(1);
        r.seq = self.res.rq_seq;
        self.res.rq[k] = r;
        None
    }

    /// A held RESOLVE answered: `a` to the call whose token is `tok`.
    fn res_reply(&self, tok: &[u8; 32], a: &[u8; RES_LEN]) {
        svc(REPLY, self.off(tok.as_ptr()), self.off(a.as_ptr()), RES_LEN as u64, 0, 0);
    }

    /// Request `r` from its current type on: its answer, or None when it waits on a lookup (`r.rec` the record). A fresh address answers; "no such address" moves on to the next type; a stale address answers at once while its name is being asked for again (Mahere's rule, 2026-11-11: the last good address meanwhile), else its name is asked and the request waits; nothing known is asked.
    fn res_run(&mut self, r: &mut Rq, now: u64) -> Option<[u8; RES_LEN]> {
        while r.phase < r.n {
            let qtype = r.order[r.phase];
            let Some(i) = self.res.cache.slot(&r.name, qtype, now) else { return Some(res_answer(RES_BUSY, 0, [0; 16], 0)) };
            match self.res.cache.look(i, now, self.freq) {
                ferros_dns::Look::Fresh { addr, left } => return Some(res_answer(RES_FOUND, fam_of(qtype), addr, left)),
                ferros_dns::Look::NoName => {
                    r.worst = res_worse(r.worst, RES_NO_NAME);
                    r.phase += 1;
                }
                ferros_dns::Look::Stale { addr } => {
                    if self.res.cache.r[i].asking || !self.res_start(i, now) {
                        return Some(res_answer(RES_FOUND, fam_of(qtype), addr, 0));
                    }
                    r.rec = i;
                    return None;
                }
                ferros_dns::Look::Unknown => {
                    if self.res.cache.r[i].asking || self.res_start(i, now) {
                        r.rec = i;
                        return None;
                    }
                    r.worst = res_worse(r.worst, RES_NO_WAY);
                    r.phase += 1;
                }
            }
        }
        Some(res_answer(r.worst, 0, [0; 16], 0))
    }

    /// A lookup for record `i` begun: a fresh id from the entropy grant, the first try sent. False when it cannot be asked: no DNS server of either family, or no entropy (a guessable id is an answer anyone could forge).
    fn res_start(&mut self, i: usize, now: u64) -> bool {
        let (d4, d6) = self.dns_servers();
        if d4 == [0; 4] && d6 == [0; 16] {
            return false;
        }
        let Some(id) = self.rand16() else {
            self.put(b"DNS: no entropy for a question id (needs --entropy)\n");
            return false;
        };
        self.res.asks[i] = Ask { id, t0: now, ..ASK_FREE };
        self.res.cache.r[i].asking = true;
        self.res_send(i, now);
        true
    }

    /// Record `i`'s question, its current try: to the servers of the families with a link in turn (IPv6's first), each try's server noted (`asked`), so an answer counts only from a server asked.
    fn res_send(&mut self, i: usize, now: u64) {
        let (d4, d6) = self.dns_servers();
        let mut s = [(0u8, [0u8; 16]); 2];
        let mut n = 0;
        if d6 != [0; 16] {
            s[n] = (6, d6);
            n += 1;
        }
        if d4 != [0; 4] {
            let mut a = [0u8; 16];
            a[..4].copy_from_slice(&d4);
            s[n] = (4, a);
            n += 1;
        }
        let a = &mut self.res.asks[i];
        if n == 0 {
            // The links went: this try is lost now, and the next one looks again.
            a.hop = false;
            a.due = now;
            return;
        }
        let (fam, srv) = s[a.tries % n];
        a.asked[a.tries] = (fam, srv);
        let id = a.id;
        let mut q = [0u8; ferros_dns::QUERY_MAX];
        let r = &self.res.cache.r[i];
        let len = ferros_dns::query(id, &r.name, r.qtype, &mut q);
        let port = self.res.port;
        let sent = self.udp_send(fam, srv, 53, port, &q[..len]);
        let a = &mut self.res.asks[i];
        // Sent: a second for its answer (`DNS_WAIT_LOG2`). Finding the next hop: the same try again shortly (`DNS_HOP_LOG2`). No address of that family, or refused: this try is lost now.
        a.hop = sent == 1;
        a.due = match sent {
            0 => now + (self.freq << DNS_WAIT_LOG2),
            1 => now + (self.freq >> DNS_HOP_LOG2),
            _ => now,
        };
    }

    /// A datagram to the resolver's port: an answer only from port 53 of a server one of the lookup's tries went to, with the lookup's id, and repeating its question (`ferros_dns::read`); anything else is passed over, as if lost (anyone on the network can send to the port).
    fn dns_in(&mut self, fam: u8, addr: [u8; 16], sport: u16, data: &[u8]) {
        // WHY: the datagram is the network's; under the 12 bytes of a header it is no answer.
        if sport != 53 || data.len() < 12 {
            return;
        }
        let id = u16::from_be_bytes([data[0], data[1]]);
        let now = now();
        for i in 0..RECN {
            let a = &self.res.asks[i];
            if !self.res.cache.r[i].asking || a.id != id || !a.asked[..=a.tries].contains(&(fam, addr)) {
                continue;
            }
            let (name, qtype) = (self.res.cache.r[i].name, self.res.cache.r[i].qtype);
            match ferros_dns::read(data, id, &name, qtype) {
                None => continue,
                Some(ferros_dns::Reply::Failed) => {
                    // The server could not say: the next try at once (to the other server when there are two).
                    let a = &mut self.res.asks[i];
                    a.hop = false;
                    a.due = now;
                }
                Some(reply) => self.res_end(i, reply, now),
            }
            return;
        }
    }

    /// Each pass: a try run out is followed by the next (a lookup whose last try ran out ends with no answer), a held RESOLVE whose wait ran out is answered "still asking" (its lookup goes on), and while nobody is present every held one is answered with nothing, as any request would be.
    fn dns_tick(&mut self) {
        let now = now();
        for i in 0..RECN {
            if !self.res.cache.r[i].asking || self.res.asks[i].due > now {
                continue;
            }
            let a = &mut self.res.asks[i];
            if a.hop && a.hops < DNS_HOPS {
                a.hops += 1;
            } else {
                a.tries += 1;
                a.hops = 0;
                if a.tries == DNS_TRIES {
                    self.res_end(i, ferros_dns::Reply::Failed, now);
                    continue;
                }
            }
            self.res_send(i, now);
        }
        for k in 0..RQN {
            let r = self.res.rq[k];
            if r.used && (!self.person || now > r.until) {
                self.res.rq[k] = RQ_FREE;
                if self.person {
                    self.res_reply(&r.tok, &res_answer(RES_ASKING, 0, [0; 16], 0));
                } else {
                    let none = [0u8; 1];
                    svc(REPLY, self.off(r.tok.as_ptr()), self.off(none.as_ptr()), 0, 0, 0);
                }
            }
        }
    }

    /// Record `i`'s lookup ended with `reply` (`Failed`: no answer after its tries): the cache told, the console a line (the type, the outcome, the time; never the name or the address, the person's), and every RESOLVE held on it answered or moved on to its next type.
    fn res_end(&mut self, i: usize, reply: ferros_dns::Reply, now: u64) {
        self.res.cache.r[i].asking = false;
        let qtype = self.res.cache.r[i].qtype;
        let a = self.res.asks[i];
        // What the held requests get: an address and its TTL, or the status their type ends with.
        let got = match reply {
            ferros_dns::Reply::Found { addr, ttl } => {
                self.res.cache.found(i, addr, ttl, now, self.freq);
                Ok((addr, ttl))
            }
            ferros_dns::Reply::NoName { ttl } => {
                self.res.cache.no_name(i, ttl, now, self.freq);
                Err(RES_NO_NAME)
            }
            ferros_dns::Reply::Failed => self.res.cache.lost(i, now, self.freq).ok_or(RES_NO_ANSWER),
        };
        // `if`s, not a `match` yielding the words (the native payload rule: no pointer tables).
        self.put(if qtype == ferros_dns::AAAA { b"DNS: AAAA ".as_slice() } else { b"DNS: A " });
        if matches!(reply, ferros_dns::Reply::Found { .. }) {
            self.put(b"found");
        } else if matches!(reply, ferros_dns::Reply::NoName { .. }) {
            self.put(b"no such address");
        } else if got.is_ok() {
            self.put(b"no answer, the last good address kept");
        } else {
            self.put(b"no answer");
        }
        self.put(b" in A#");
        self.dec(now.wrapping_sub(a.t0) * 1000 / self.freq);
        self.put(b" ms, A#");
        self.dec(a.tries as u64 + (a.tries < DNS_TRIES) as u64);
        self.put(b" tries\n");
        for k in 0..RQN {
            let mut r = self.res.rq[k];
            if !r.used || r.rec != i {
                continue;
            }
            let ans = match got {
                Ok((addr, ttl)) => Some(res_answer(RES_FOUND, fam_of(qtype), addr, ttl)),
                Err(s) => {
                    r.worst = res_worse(r.worst, s);
                    r.phase += 1;
                    self.res_run(&mut r, now)
                }
            };
            match ans {
                Some(ans) => {
                    self.res.rq[k] = RQ_FREE;
                    self.res_reply(&r.tok, &ans);
                }
                None => self.res.rq[k] = r,
            }
        }
    }

    // ---- TCP (client only) ----

    /// Connection `i`'s receive buffer.
    /// SAFETY: the block is this connection's alone while it holds it (`Blocks::take`), the arc holds its bytes (grown before the block is taken, shrunk only past the highest block still taken), and this process has one thread, so nothing else reaches these bytes while the slice is used.
    fn rx(&self, i: usize) -> &'static mut [u8] {
        let a = self.blocks.addr(self.tcb[i].blk);
        unsafe { core::slice::from_raw_parts_mut(a as *mut u8, RXB) }
    }

    /// Connection `i`'s send buffer, right after its receive buffer (SAFETY as `rx`).
    fn tx(&self, i: usize) -> &'static mut [u8] {
        let a = self.blocks.addr(self.tcb[i].blk) + RXB;
        unsafe { core::slice::from_raw_parts_mut(a as *mut u8, TXB) }
    }

    /// The retransmission timeout's granularity, floor and ceiling in ticks: the main loop's millisecond, 250 ms (Linux's 200 ms, to a power of two; RFC 6298's one second would hold every loss the fast retransmit misses for a second), 8 s (the ceiling this stack always had, so eight expiries fail a connection in about 30 s, as six did before).
    fn rto_bounds(&self) -> (u64, u64, u64) {
        (self.freq >> 10, self.freq >> 2, self.freq << 3)
    }

    /// The SACK option's length on connection `i`'s next segment: blocks while ranges are held out of order and the peer permits them, else none.
    fn opt_len(&self, i: usize) -> usize {
        let c = &self.tcb[i];
        if !c.sack_ok || c.ooo.n == 0 {
            return 0;
        }
        // WHY the min: an ACK carries `SACK_N` blocks at most (`Ooo::sack_blocks`).
        4 + 8 * c.ooo.n.min(ferros_tcp::SACK_N)
    }

    /// A segment of connection `i`: `flags` (FIN, SYN, RST, PSH, ACK), sequence `seq`, `payload` (at most the connection's segment size less `opt_len`). The SYN carries our options (MSS, window scale, SACK permitted) and an unscaled window; every other segment carries the ACK, the receive window (`ferros_tcp::window`, its right edge kept in `rcv_adv`) and, while ranges are held out of order and the peer permits it, their SACK blocks; it pays any ACK owed. Returns whether it went out (false: the next hop is not resolved yet).
    fn tcp_out(&mut self, i: usize, flags: u8, seq: u32, payload: &[u8]) -> bool {
        let mut seg = [0u8; 20 + 40 + MSS_MAX];
        let mut hl = 20;
        let c = &mut self.tcb[i];
        let field;
        if flags & SYN != 0 {
            seg[20..32].copy_from_slice(&ferros_tcp::syn_options(c.rmss as u16, RCV_WS));
            hl = 32;
            // The SYN's window is never scaled (RFC 7323 2.2).
            field = NO_WS_WIN as u16;
        } else {
            if c.sack_ok && c.ooo.n > 0 {
                let mut b = [(0, 0); ferros_tcp::SACK_N];
                let k = c.ooo.sack_blocks(&mut b);
                let mut o = [0u8; 4 + 8 * ferros_tcp::SACK_N];
                let l = ferros_tcp::sack_option(&b[..k], &mut o);
                seg[20..20 + l].copy_from_slice(&o[..l]);
                hl += l;
            }
            let free = c.rcv_total - c.rx_len;
            let left = if ferros_tcp::lt(c.rcv_adv, c.rcv_nxt) { 0 } else { c.rcv_adv.wrapping_sub(c.rcv_nxt) };
            let (f, said) = ferros_tcp::scaled(ferros_tcp::window(free, left, c.rcv_total, c.rmss), left, c.rcv_ws);
            field = f;
            c.rcv_adv = c.rcv_nxt.wrapping_add(said);
            c.owed = false;
            c.unacked = 0;
            c.since_ack = 0;
            c.acks_out += payload.is_empty() as u32;
        }
        let n = hl + payload.len();
        seg[0..2].copy_from_slice(&c.lport.to_be_bytes());
        seg[2..4].copy_from_slice(&c.rport.to_be_bytes());
        seg[4..8].copy_from_slice(&seq.to_be_bytes());
        seg[8..12].copy_from_slice(&(if flags & ACK != 0 { c.rcv_nxt } else { 0 }).to_be_bytes());
        seg[12] = ((hl / 4) as u8) << 4;
        seg[13] = flags;
        seg[14..16].copy_from_slice(&field.to_be_bytes());
        seg[hl..n].copy_from_slice(payload);
        let (fam, cell, src, addr) = (c.fam, c.cell, c.src, c.addr);
        // On the link and from the address the connection opened with.
        if fam == 4 {
            let dst: [u8; 4] = addr[..4].try_into().unwrap();
            let me: [u8; 4] = src[..4].try_into().unwrap();
            let mut ph = [0u8; 12];
            ph[..4].copy_from_slice(&me);
            ph[4..8].copy_from_slice(&dst);
            ph[9] = 6;
            ph[10..12].copy_from_slice(&(n as u16).to_be_bytes());
            let ck = csum(&seg[..n], sum(&ph));
            seg[16..18].copy_from_slice(&ck.to_be_bytes());
            self.out4(cell, me, dst, 6, &seg[..n])
        } else {
            let ck = Self::sum6(&src, &addr, 6, &seg[..n]);
            seg[16..18].copy_from_slice(&ck.to_be_bytes());
            self.out6(cell, src, addr, 6, &seg[..n], HOP)
        }
    }

    /// An ACK alone on connection `i` (the window and any SACK blocks with it).
    fn tcp_ack(&mut self, i: usize) {
        let s = self.tcb[i].snd_nxt;
        self.tcp_out(i, ACK, s, &[]);
    }

    /// The ACKs still owed at the end of a batch of frames (an odd in-order segment's): no delayed-ACK timer, the batch is microseconds, and the loop never sleeps while an ACK waits to go (`busy`).
    fn tcp_acks(&mut self) {
        for i in 0..TCPN {
            if self.tcb[i].state != 0 && self.tcb[i].owed {
                self.tcp_ack(i);
            }
        }
    }

    fn tcp_connect(&mut self, fam: u8, addr: [u8; 16], port: u16, out: &mut [u8]) -> usize {
        // The link and source the connection keeps for its life.
        let picked = match fam {
            4 => self.pick4(&addr[..4].try_into().unwrap()).map(|(c, s)| {
                let mut a = [0u8; 16];
                a[..4].copy_from_slice(&s);
                (c, a)
            }),
            6 => self.pick6(&addr),
            _ => None,
        };
        let Some((cell, src)) = picked else {
            out[0] = 2;
            return 1;
        };
        let Some(i) = self.tcb.iter().position(|c| c.state == 0) else {
            out[0] = 4;
            return 1;
        };
        let Some(blk) = self.blocks.take() else {
            out[0] = 6;
            return 1;
        };
        // A fresh initial sequence number and an ephemeral port from the session's randomness (an LCG stepped off the entropy-seeded value).
        self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.eph = 49152 + (self.eph.wrapping_add(1 + (self.seed >> 20) as u16 % 97) % 16384);
        let iss = self.seed;
        let t = now();
        // Our MSS: what the link it leaves on carries for its family (WiFi's Ethernet MTU, the call's own), less the headers.
        let mtu = if cell { self.cell.mtu } else { 1500 };
        let rmss = ferros_tcp::mss_for(mtu, fam) as u32;
        let one = iss.wrapping_add(1);
        self.tcb[i] = Tcb { state: 1, fam, cell, src, addr, rport: port, lport: self.eph, blk, iss, snd_una: iss, snd_nxt: one, snd_max: one, rmss, smss: rmss, rcv_total: NO_WS_WIN, rtt: ferros_tcp::Rtt::new(self.freq), rto_t: t + self.freq, timing: true, rtt_seq: one, rtt_t: t, touched: t, asked_t: t, ..TCB_FREE };
        self.tcp_out(i, SYN, iss, &[]);
        out[0] = 0;
        out[1] = i as u8;
        2
    }

    /// Connection `i` failed (`why`: reset, or unanswered): its data still unread stays for its app, nothing more is sent.
    fn tcp_fail(&mut self, i: usize, why: u8) {
        let c = &mut self.tcb[i];
        c.state = 4;
        c.why = why;
        c.rto_t = 0;
        // Counted against the address in the resolver's cache: after `ferros_dns::FAILURES` in a row its name is asked for again (Mahere's rule, now every client's).
        let (fam, addr) = (c.fam, c.addr);
        self.res.cache.conn(fam, &addr, false, now());
    }

    /// Connection `i` freed: its report on the console, its block given back.
    fn tcp_free(&mut self, i: usize, why: u8) {
        self.report(i, why);
        let blk = self.tcb[i].blk;
        if blk != NO_BLOCK {
            self.blocks.give(blk);
        }
        self.tcb[i] = TCB_FREE;
    }

    /// A segment in for us: `src` the peer, `dst` our address it came to, `seg` the TCP header and data (20 bytes at least, the callers' check).
    fn tcp_in(&mut self, fam: u8, src: [u8; 16], dst: [u8; 16], seg: &[u8]) {
        // The checksum over the pseudo-header and the whole segment folds to zero when nothing changed on the way (RFC 9293 3.1): a bad one is dropped, as if lost (a corrupted byte would otherwise land in a map cell).
        let ok = if fam == 4 {
            let mut ph = [0u8; 12];
            ph[..4].copy_from_slice(&src[..4]);
            ph[4..8].copy_from_slice(&dst[..4]);
            ph[9] = 6;
            ph[10..12].copy_from_slice(&(seg.len() as u16).to_be_bytes());
            csum(seg, sum(&ph)) == 0
        } else {
            Self::sum6(&src, &dst, 6, seg) == 0
        };
        if !ok {
            return;
        }
        let sport = u16::from_be_bytes([seg[0], seg[1]]);
        let dport = u16::from_be_bytes([seg[2], seg[3]]);
        let Some(i) = self.tcb.iter().position(|c| c.state != 0 && c.fam == fam && c.addr == src && c.rport == sport && c.lport == dport) else { return };
        let seq = u32::from_be_bytes([seg[4], seg[5], seg[6], seg[7]]);
        let ack = u32::from_be_bytes([seg[8], seg[9], seg[10], seg[11]]);
        // WHY the checks: the data offset is the peer's; under 20 it would read the header as options, past the segment it would read what is not there.
        let off = (seg[12] >> 4) as usize * 4;
        if off < 20 || off > seg.len() {
            return;
        }
        let flags = seg[13];
        let win = u16::from_be_bytes([seg[14], seg[15]]) as u32;
        let opts = ferros_tcp::parse(&seg[20..off]);
        let data = &seg[off..];
        let t = now();
        if self.tcb[i].state == 1 {
            self.tcp_synack(i, flags, seq, ack, win, &opts, t);
            return;
        }
        // Failed: nothing more is answered (its app reads what came before).
        if self.tcb[i].state == 4 {
            return;
        }
        let c = &self.tcb[i];
        let free = c.rcv_total - c.rx_len;
        // A reset (RFC 5961 3.2): at exactly the next expected byte it ends the connection; elsewhere in the window it gets a challenge ACK (a blind attacker's guess is answered, not obeyed); outside it, nothing.
        if flags & RST != 0 {
            if seq == c.rcv_nxt {
                self.tcp_fail(i, WHY_RESET);
            } else if seq.wrapping_sub(c.rcv_nxt) < free {
                self.tcp_ack(i);
            }
            return;
        }
        // A SYN on an open connection: a challenge ACK, nothing else (RFC 5961 4.2).
        if flags & SYN != 0 {
            self.tcp_ack(i);
            return;
        }
        if flags & ACK == 0 {
            return;
        }
        // An ACK of what was never sent: answered with ours and dropped (RFC 9293 3.10.7.4).
        if ferros_tcp::lt(c.snd_max, ack) {
            self.tcp_ack(i);
            return;
        }
        let (g, rto_min, rto_max) = self.rto_bounds();
        let c = &mut self.tcb[i];
        // WHY no bound: the field is 16 bits and the shift at most 14 (ferros_tcp::parse), so the window fits 30 bits.
        let wnd = win << c.snd_ws;
        let smss = c.smss;
        let mut resend = false;
        if ferros_tcp::lt(c.snd_una, ack) {
            // New data acknowledged: off the send buffer's front, then our FIN.
            let acked = ack.wrapping_sub(c.snd_una);
            let sent_before = c.snd_nxt.wrapping_sub(c.snd_una);
            // WHY the min: the ACK covers our FIN too once it is out, and the FIN is no byte of the buffer.
            let data_acked = acked.min(c.tx_len);
            c.tx_head = (c.tx_head + data_acked) & (TXB as u32 - 1);
            c.tx_len -= data_acked;
            c.bytes_out += data_acked as u64;
            if c.fin_ever && ack == c.snd_max {
                c.fin_acked = true;
            }
            c.snd_una = ack;
            if ferros_tcp::lt(c.snd_nxt, ack) {
                c.snd_nxt = ack;
            }
            if c.timing && ferros_tcp::le(c.rtt_seq, ack) {
                c.rtt.sample(t.wrapping_sub(c.rtt_t), g, rto_min, rto_max);
                c.timing = false;
            }
            c.tries = 0;
            c.rtt.reset();
            c.rto_t = if c.snd_max == c.snd_una { 0 } else { t + c.rtt.cur };
            let limited = sent_before + smss >= c.cc.cwnd;
            resend = c.cc.on_ack(ack, acked, c.snd_max.wrapping_sub(ack), limited, smss) == ferros_tcp::Act::Resend;
        } else if ack == c.snd_una && data.is_empty() && flags & FIN == 0 && wnd == c.snd_wnd && c.snd_max != c.snd_una {
            // A duplicate ACK (RFC 5681 2): a segment arrived past a hole at the peer.
            let flight = c.snd_max.wrapping_sub(c.snd_una);
            resend = c.cc.on_dup(c.snd_una, c.snd_max, flight, smss) == ferros_tcp::Act::Resend;
            if resend && c.cc.dup == 3 {
                c.fast += 1;
            }
        }
        // The peer's window, from the newest segment only (RFC 9293 3.10.7.4).
        if ferros_tcp::lt(c.wl1, seq) || (c.wl1 == seq && ferros_tcp::le(c.wl2, ack)) {
            c.snd_wnd = wnd;
            c.wl1 = seq;
            c.wl2 = ack;
        }
        // A closed window answered: the peer lives, so its probes going unanswered by data are no failure.
        if c.snd_wnd == 0 {
            c.tries = 0;
        }
        if resend {
            self.tcp_resend(i);
        }
        let rx = self.rx(i);
        let freq = self.freq;
        let c = &mut self.tcb[i];
        let fin = flags & FIN != 0;
        let mut now_ack = false;
        let mut delivered = 0u32;
        if c.state == 2 && (!data.is_empty() || fin) {
            let nxt0 = c.rcv_nxt;
            let free0 = c.rcv_total - c.rx_len;
            let (mut s, mut d) = (seq, data);
            // What came before the next expected byte came before: only its new part is taken. WHY the test: the overlap is the peer's numbers; a skip past the data leaves none.
            if ferros_tcp::lt(s, nxt0) {
                let skip = nxt0.wrapping_sub(s) as usize;
                d = if skip < d.len() { &d[skip..] } else { &[] };
                s = nxt0;
            }
            let at = s.wrapping_sub(nxt0);
            if d.is_empty() {
                // All of it came before (a retransmission: our ACK was lost), or a FIN alone.
                now_ack |= !data.is_empty();
            } else if at >= free0 {
                // Past the window (a probe of a closed one, or a peer that overran it): not kept, ACKed so the peer hears the window.
                now_ack = true;
            } else {
                // WHY the min: the room past `at` is all the window gave; past it the bytes have nowhere to go.
                let take = (d.len() as u32).min(free0 - at);
                ring_put(rx, (c.rx_head + c.rx_len + at) as usize, &d[..take as usize]);
                if at == 0 {
                    c.rx_len += take;
                    c.rcv_nxt = c.rcv_nxt.wrapping_add(take);
                    delivered = take;
                    // The hole before held ranges filled: they join, and the peer hears at once.
                    let filled = c.ooo.advance(c.rcv_nxt);
                    if filled != c.rcv_nxt {
                        let more = filled.wrapping_sub(c.rcv_nxt);
                        c.rx_len += more;
                        c.rcv_nxt = filled;
                        delivered += more;
                        now_ack = true;
                    }
                    // Every second in-order segment ACKed at once (RFC 5681 4.2, RFC 1122 4.2.3.2), one with PSH too; an odd one at the end of its batch (`tcp_acks`, after each exchange with the WiFi driver and each pass), never left owed into a sleep.
                    // From 2026-11-11 to 2026-11-12 the batch was ACKed once (a receiver coalescing its segments), one ACK for up to 88 segments: the Pro's close reports said 143 segments in and 17 ACKs out, a sender starved of ACKs grows its window by them and paces by them (`docs/WIFI.md`, "ACKs").
                    c.unacked += 1;
                    if c.unacked >= 2 || flags & PSH != 0 {
                        now_ack = true;
                    } else {
                        c.owed = true;
                    }
                } else {
                    // Past a hole: kept in place, and a duplicate ACK at once with the SACK blocks, so the peer's fast retransmit fills the hole.
                    c.ooo.latest = s;
                    if c.ooo.insert(s, s.wrapping_add(take)) {
                        c.ooo_segs += 1;
                        let held = c.ooo.bytes();
                        // WHY the max: the high water, for the report.
                        c.ooo_most = c.ooo_most.max(held);
                    }
                    now_ack = true;
                }
            }
            // The peer's FIN at the end of this segment, kept once every byte before it fits the window, and taken when they are all in.
            let fin_at = seq.wrapping_add(data.len() as u32);
            if fin && ferros_tcp::le(nxt0, fin_at) && fin_at.wrapping_sub(nxt0) <= free0 {
                c.fin_in = true;
                c.fin_seq = fin_at;
            }
            // A FIN is ACKed at once, in order or not (RFC 5681 4.2).
            now_ack |= fin;
            if c.fin_in && c.rcv_nxt == c.fin_seq {
                c.rcv_nxt = c.rcv_nxt.wrapping_add(1);
                c.peer_fin = true;
                c.state = 3;
                now_ack = true;
            }
        } else if c.state == 3 && (!data.is_empty() || fin) {
            // After the peer's FIN: a retransmission (our ACK of it was lost).
            now_ack = true;
        }
        if !data.is_empty() {
            c.segs_in += 1;
        }
        if delivered > 0 {
            c.bytes_in += delivered as u64;
            // What the peer sent between two of our ACKs, the most of it: how much it had in flight as we see it.
            c.since_ack += delivered;
            // WHY the max: the high water, for the report.
            c.burst_most = c.burst_most.max(c.since_ack);
            c.last_in_t = t;
            // WHY the max: the high water, for the report.
            c.rx_most = c.rx_most.max(c.rx_len);
            // The rate over quarter seconds, its peak kept.
            let dt = t.wrapping_sub(c.rate_t);
            if dt >= freq >> 2 {
                let r = (c.bytes_in - c.rate_b) * freq / dt;
                if r > c.peak {
                    c.peak = r;
                }
                c.rate_t = t;
                c.rate_b = c.bytes_in;
            }
        }
        if now_ack {
            self.tcp_ack(i);
        }
    }

    /// The answer to our SYN (connection `i` in state 1): a SYN-ACK of it opens the connection with what its options say; a reset of it fails the connection; anything else is not for it.
    fn tcp_synack(&mut self, i: usize, flags: u8, seq: u32, ack: u32, win: u32, opts: &ferros_tcp::Opts, t: u64) {
        let (g, rto_min, rto_max) = self.rto_bounds();
        let c = &mut self.tcb[i];
        let mine = ack == c.iss.wrapping_add(1);
        if flags & RST != 0 {
            // A refusal: only one that acknowledges our SYN counts (RFC 9293 3.10.7.3).
            if flags & ACK != 0 && mine {
                self.tcp_fail(i, WHY_RESET);
            }
            return;
        }
        if flags & (SYN | ACK) != SYN | ACK || !mine {
            return;
        }
        c.rcv_nxt = seq.wrapping_add(1);
        c.rcv_adv = c.rcv_nxt;
        c.snd_una = ack;
        // Window scaling only when both SYNs carry it (RFC 7323 2.2); without it the window is 64 KiB of the buffer.
        if let Some(ws) = opts.ws {
            c.snd_ws = ws;
            c.rcv_ws = RCV_WS;
            c.rcv_total = RXB as u32;
        } else {
            c.snd_ws = 0;
            c.rcv_ws = 0;
            c.rcv_total = NO_WS_WIN;
        }
        c.sack_ok = opts.sack_ok;
        let peer = if opts.mss != 0 { opts.mss } else { ferros_tcp::mss_default(c.fam) } as u32;
        // WHY the floor: the MSS is the peer's; 0 would leave the send loop unable to move a byte and a few bytes a segment is a flood of headers (Linux refuses under 88, TCP_MIN_MSS). The smaller of its MSS and ours, as our link carries no more.
        let peer = peer.max(1 << 6);
        c.smss = peer.min(c.rmss);
        // The SYN-ACK's window is never scaled (RFC 7323 2.2).
        c.snd_wnd = win;
        c.wl1 = seq;
        c.wl2 = ack;
        c.cc = ferros_tcp::Cc::new(c.smss, c.iss);
        // Karn's rule: a SYN sent again is not timed.
        if c.timing && c.tries == 0 {
            c.rtt.sample(t.wrapping_sub(c.rtt_t), g, rto_min, rto_max);
        }
        c.timing = false;
        c.tries = 0;
        c.rtt.reset();
        c.rto_t = 0;
        c.state = 2;
        c.up_t = t;
        c.rate_t = t;
        c.last_in_t = t;
        // The address answers: its failures in the resolver's cache start again.
        let (fam, addr) = (c.fam, c.addr);
        self.res.cache.conn(fam, &addr, true, t);
        self.tcp_ack(i);
    }

    /// The first unacknowledged segment of connection `i` sent again (a fast retransmit, or the next hole in NewReno's recovery): from the send buffer's front, or our FIN when only it is outstanding.
    fn tcp_resend(&mut self, i: usize) {
        let room = self.tcb[i].smss as usize - self.opt_len(i);
        let c = self.tcb[i];
        // WHY the min: a segment's worth, or what the buffer holds.
        let n = (c.tx_len as usize).min(room);
        let mut p = [0u8; MSS_MAX];
        if n > 0 {
            ring_get(self.tx(i), c.tx_head as usize, &mut p[..n]);
            self.tcp_out(i, ACK, c.snd_una, &p[..n]);
        } else if c.fin_ever && !c.fin_acked {
            self.tcp_out(i, ACK | FIN, c.snd_una, &[]);
        } else {
            return;
        }
        let c = &mut self.tcb[i];
        c.resent += 1;
        c.timing = false;
        let end = c.snd_una.wrapping_add(if n > 0 { n as u32 } else { 1 });
        if ferros_tcp::lt(c.snd_nxt, end) {
            c.snd_nxt = end;
        }
    }

    /// Each pass: finished connections freed, retransmission on expiry (back to the oldest unacknowledged byte, the congestion window to one segment), then new data out (`tcp_push`).
    fn tcp_tick(&mut self) {
        let t = now();
        let (_, _, rto_max) = self.rto_bounds();
        for i in 0..TCPN {
            let c = self.tcb[i];
            if c.state == 0 {
                continue;
            }
            if c.fin_acked && c.peer_fin {
                self.tcp_free(i, WHY_CLOSED);
                continue;
            }
            if c.closed_t != 0 && t.wrapping_sub(c.closed_t) > 16 * self.freq {
                self.tcp_free(i, WHY_LINGER);
                continue;
            }
            // Abandoned: a reset to the peer if it was open (it stops sending), then the slot is free.
            if t.wrapping_sub(c.touched) > 32 * self.freq {
                if c.state == 2 || c.state == 3 {
                    self.tcp_out(i, RST | ACK, c.snd_nxt, &[]);
                }
                self.tcp_free(i, WHY_IDLE);
                continue;
            }
            if c.state == 4 {
                continue;
            }
            // The retransmission timer: the one timer TCP needs (a segment lost with nothing after it brings no ACK to say so).
            let flight = c.snd_max.wrapping_sub(c.snd_una);
            let mut expired = false;
            if c.rto_t != 0 && t > c.rto_t && (flight > 0 || c.state == 1) {
                if c.tries + 1 >= TRIES {
                    self.tcp_fail(i, WHY_TIMEOUT);
                    continue;
                }
                let c = &mut self.tcb[i];
                c.tries += 1;
                c.expiries += 1;
                c.rtt.backoff(rto_max);
                c.rto_t = t + c.rtt.cur;
                c.timing = false;
                if c.state == 1 {
                    let iss = c.iss;
                    self.tcp_out(i, SYN, iss, &[]);
                    continue;
                }
                c.cc.on_timeout(flight, c.snd_max, c.smss);
                // Go back to the oldest unacknowledged byte.
                c.snd_nxt = c.snd_una;
                c.fin_sent = false;
                expired = true;
            }
            if self.tcb[i].state >= 2 {
                self.tcp_push(i, t, expired);
            }
        }
    }

    /// Connection `i`'s data out, a segment at a time, within the peer's window and the congestion window; its FIN after the last byte once its app closed. A closed peer window is probed with one byte on the retransmission timer (RFC 9293 3.8.6.1; `expired` this pass's expiry).
    fn tcp_push(&mut self, i: usize, t: u64, expired: bool) {
        loop {
            let room = self.tcb[i].smss - self.opt_len(i) as u32;
            let c = &mut self.tcb[i];
            let out = c.snd_nxt.wrapping_sub(c.snd_una);
            // WHY the min: once our FIN is out it counts in `out` and is no byte of the buffer.
            let sent = out.min(c.tx_len);
            let unsent = c.tx_len - sent;
            // What may be outstanding: the smaller of the two windows (RFC 5681 3), less what is. WHY the test: the peer may shrink its window under what is already in flight.
            let wnd = c.cc.cwnd.min(c.snd_wnd);
            let usable = if wnd > out { wnd - out } else { 0 };
            let full = unsent.min(room);
            let mut n = full.min(usable);
            if n == 0 && unsent > 0 && out == 0 && c.snd_wnd == 0 {
                if c.rto_t == 0 {
                    c.rto_t = t + c.rtt.cur;
                    break;
                }
                if !(expired || t > c.rto_t) {
                    break;
                }
                n = 1;
            }
            if n == 0 {
                break;
            }
            // Sender silly-window avoidance: a segment smaller than a full one and than what is left waits while anything is in flight (its ACK opens the window).
            if n < full && out > 0 {
                break;
            }
            let (seq, at) = (c.snd_nxt, c.tx_head + sent);
            let flags = if n == unsent { ACK | PSH } else { ACK };
            let mut p = [0u8; MSS_MAX];
            ring_get(self.tx(i), at as usize, &mut p[..n as usize]);
            if !self.tcp_out(i, flags, seq, &p[..n as usize]) {
                break;
            }
            let c = &mut self.tcb[i];
            if c.snd_una == c.snd_nxt {
                c.rto_t = t + c.rtt.cur;
            }
            if ferros_tcp::lt(c.snd_nxt, c.snd_max) {
                c.resent += 1;
            } else if !c.timing {
                c.timing = true;
                c.rtt_seq = c.snd_nxt.wrapping_add(n);
                c.rtt_t = t;
            }
            c.snd_nxt = c.snd_nxt.wrapping_add(n);
            if ferros_tcp::lt(c.snd_max, c.snd_nxt) {
                c.snd_max = c.snd_nxt;
            }
        }
        let c = self.tcb[i];
        if c.fin_queued && !c.fin_sent && c.snd_nxt.wrapping_sub(c.snd_una) >= c.tx_len && self.tcp_out(i, ACK | FIN, c.snd_nxt, &[]) {
            let c = &mut self.tcb[i];
            if c.snd_una == c.snd_nxt {
                c.rto_t = t + c.rtt.cur;
            }
            c.snd_nxt = c.snd_nxt.wrapping_add(1);
            if ferros_tcp::lt(c.snd_max, c.snd_nxt) {
                c.snd_max = c.snd_nxt;
            }
            c.fin_sent = true;
            c.fin_ever = true;
        }
    }

    /// `v` tenths as A#whole.tenth.
    fn tenths(&mut self, v: u64) {
        self.put(b"A#");
        self.dec(v / 10);
        self.put(b".");
        self.dec(v % 10);
    }

    /// `v` hundredths as A#whole.hundredths.
    fn hundredths(&mut self, v: u64) {
        self.put(b"A#");
        self.dec(v / 100);
        self.put(if v % 100 < 10 { b".0".as_slice() } else { b"." });
        self.dec(v % 100);
    }

    /// Connection `i`'s measures on the console as it is freed, one line (owner, 2026-11-11: what he reads on the device after a Mahere base cell fetch): how it ended, bytes each way, how long it was open, the peak rate over quarter seconds and the average, the smoothed round trip and its variation and the timeout, segments resent (fast retransmits, timer expiries), segments kept out of order and the most bytes so held, the most bytes waiting for the app of the window's most, the segment sizes, the scales, SACK, and the congestion window at the end.
    fn report(&mut self, i: usize, why: u8) {
        let c = self.tcb[i];
        let f = self.freq;
        // A report is up to about 500 bytes: the log goes out first, so the line fits whole (`put` drops what overflows).
        self.flush();
        self.put(b"tcp A#");
        self.dec(i as u64);
        self.put(b" port A#");
        self.dec(c.rport as u64);
        if why == WHY_CLOSED {
            self.put(b" closed both ways");
        } else if why == WHY_RESET {
            self.put(b" reset by the peer");
        } else if why == WHY_TIMEOUT {
            self.put(b" failed, unanswered");
        } else if why == WHY_IDLE {
            self.put(b" abandoned by its app");
        } else {
            self.put(b" closed by its app, no FIN from the peer");
        }
        self.put(b": in A#");
        self.dec(c.bytes_in);
        self.put(b" bytes, out A#");
        self.dec(c.bytes_out);
        if c.up_t == 0 {
            self.put(b", never opened (asked A#");
            self.dec(now().wrapping_sub(c.asked_t) * 1000 / f);
            self.put(b" ms ago)\n");
            return;
        }
        self.put(b", open A#");
        self.dec(now().wrapping_sub(c.up_t) * 1000 / f);
        self.put(b" ms, peak ");
        self.tenths((c.peak * 10) >> 20);
        self.put(b" MiB/s, average ");
        let span = c.last_in_t.wrapping_sub(c.up_t);
        self.tenths(if span == 0 { 0 } else { ((c.bytes_in * f / span) * 10) >> 20 });
        self.put(b" MiB/s, round trip ");
        self.tenths(c.rtt.srtt * 10_000 / f);
        self.put(b" ms (variation ");
        self.tenths(c.rtt.var * 10_000 / f);
        self.put(b", timeout ");
        self.tenths(c.rtt.rto * 10_000 / f);
        self.put(b"), resent A#");
        self.dec(c.resent as u64);
        self.put(b" segments (fast retransmits A#");
        self.dec(c.fast as u64);
        self.put(b", expiries A#");
        self.dec(c.expiries as u64);
        self.put(b"), out of order A#");
        self.dec(c.ooo_segs as u64);
        self.put(b" segments (most held A#");
        self.dec(c.ooo_most as u64);
        self.put(b" bytes), waiting for the app most A#");
        self.dec(c.rx_most as u64);
        self.put(b" of A#");
        self.dec(c.rcv_total as u64);
        self.put(b", MSS A#");
        self.dec(c.smss as u64);
        self.put(b" out A#");
        self.dec(c.rmss as u64);
        self.put(b" in, scale A#");
        self.dec(c.snd_ws as u64);
        self.put(b" out A#");
        self.dec(c.rcv_ws as u64);
        self.put(if c.sack_ok { b" in, SACK" } else { b" in, no SACK" });
        self.put(b", congestion window A#");
        self.dec(c.cc.cwnd as u64);
        self.put(b", segments in A#");
        self.dec(c.segs_in as u64);
        self.put(b", ACKs out A#");
        self.dec(c.acks_out as u64);
        // WHY the test: a connection that took no data in has no ratio.
        if c.segs_in > 0 {
            self.put(b", ");
            self.hundredths(c.acks_out as u64 * 100 / c.segs_in as u64);
            self.put(b" a segment");
        }
        self.put(b" (most between two A#");
        self.dec(c.burst_most as u64);
        self.put(b" bytes)\n");
        // The WiFi path since the last report (`Path`), on its own line.
        if c.cell {
            return;
        }
        self.flush();
        let p = core::mem::take(&mut self.path);
        p.say(self);
    }

    /// The port's oldest datagram into `out` (family, address, source port, destination port, payload); 0 when it has none.
    fn udp_take(&mut self, port: u16, out: &mut [u8]) -> usize {
        let Some(i) = (0..QN).filter(|&i| self.q[i].used && self.q[i].dport == port).min_by_key(|&i| self.q[i].seq) else { return 0 };
        let d = &mut self.q[i];
        d.used = false;
        out[0] = d.fam;
        out[1..17].copy_from_slice(&d.addr);
        out[17..19].copy_from_slice(&d.sport.to_be_bytes());
        out[19..21].copy_from_slice(&d.dport.to_be_bytes());
        let n = d.len.min(out.len() - 21);
        out[21..21 + n].copy_from_slice(&d.data[..n]);
        21 + n
    }

    /// Connection `i`'s state and what it has taken in, `most` bytes at most, into `out` (op 6's answer). A read that reopens a window shrunk under half says so at once (`ferros_tcp::window_update_due`), or a sender held back by it would wait for its persist timer.
    fn tcp_take(&mut self, i: usize, out: &mut [u8], most: u32) -> usize {
        let c = &self.tcb[i];
        out[0] = match c.state {
            1 => 1,
            2 => 2,
            3 => 3,
            _ => 4,
        };
        // WHY the mins: only what is in order is the app's, `most` is what it asked for, and `out` holds `RECV_MAX` after the state byte.
        let n = (c.rx_len as usize).min(most as usize).min(out.len() - 1);
        if n == 0 {
            return 1;
        }
        ring_get(self.rx(i), c.rx_head as usize, &mut out[1..1 + n]);
        let c = &mut self.tcb[i];
        c.rx_head = (c.rx_head + n as u32) & (RXB as u32 - 1);
        c.rx_len -= n as u32;
        if c.state == 2 {
            let free = c.rcv_total - c.rx_len;
            let left = if ferros_tcp::lt(c.rcv_adv, c.rcv_nxt) { 0 } else { c.rcv_adv.wrapping_sub(c.rcv_nxt) };
            if ferros_tcp::window_update_due(free, left, c.rcv_total) {
                self.tcp_ack(i);
            }
        }
        1 + n
    }

    /// Hold a receive (`tok` its reply token) for `wait` seconds: on UDP `port`, or on connection `handle` when `tcp` (`most` the data its reader takes). A held receive on the same connection is answered first as things stand (one reader a connection), and with every slot taken the oldest is.
    fn hold(&mut self, tcp: bool, port: u16, handle: u8, wait: u8, most: u32, tok: &[u8; 32], out: &mut [u8]) {
        let same = if tcp { (0..HELDN).find(|&k| self.held[k].used && self.held[k].tcp && self.held[k].handle == handle) } else { None };
        let k = match same.or_else(|| (0..HELDN).find(|&k| !self.held[k].used)) {
            Some(k) => k,
            None => (0..HELDN).min_by_key(|&k| self.held[k].seq).unwrap(),
        };
        if self.held[k].used {
            self.answer(k, out);
        }
        self.held_seq = self.held_seq.wrapping_add(1);
        let state = if tcp { self.tcb[handle as usize].state } else { 0 };
        self.held[k] = Held { used: true, tcp, port, handle, state, most, seq: self.held_seq, until: now() + wait as u64 * self.freq, tok: *tok };
    }

    /// Answer hold `k` as things stand (a connection's state and data, or the port's next datagram or nothing), and free it.
    fn answer(&mut self, k: usize, out: &mut [u8]) {
        let h = self.held[k];
        self.held[k] = HELD_FREE;
        let l = if !self.person {
            0
        } else if h.tcp {
            let i = h.handle as usize;
            if self.tcb[i].state == 0 {
                out[0] = 5;
                1
            } else {
                self.tcb[i].touched = now();
                self.tcp_take(i, out, h.most)
            }
        } else {
            self.udp_take(h.port, out)
        };
        svc(REPLY, self.off(h.tok.as_ptr()), self.off(out.as_ptr()), l as u64, 0, 0);
    }

    /// Each pass: every held receive that has something now (a datagram for its port; data on its connection, or its state changed since the hold: opened, closed by the peer, failed, gone), or whose wait ran out, or all of them while nobody is present, answered. A connection waited on counts as asked about. Returns whether any was.
    fn answer_held(&mut self, out: &mut [u8]) -> bool {
        let t = now();
        let mut any = false;
        for k in 0..HELDN {
            let h = self.held[k];
            if !h.used {
                continue;
            }
            let ready = !self.person
                || t > h.until
                || if h.tcp {
                    let c = &self.tcb[h.handle as usize];
                    c.state != h.state || c.state >= 3 || c.rx_len > 0
                } else {
                    (0..QN).any(|i| self.q[i].used && self.q[i].dport == h.port)
                };
            if ready {
                self.answer(k, out);
                any = true;
            } else if h.tcp {
                self.tcb[h.handle as usize].touched = t;
            }
        }
        any
    }

    /// One `udp` request (`tok` its reply token); returns the reply length written to `out`, or None when the request is held (answered later by `answer_held`).
    fn serve(&mut self, m: &[u8], tok: &[u8; 32], out: &mut [u8]) -> Option<usize> {
        // While nobody has attested no application sends, receives, connects or joins: nothing may act on what the radios bring until a person is present (the addresses, op 3, stay readable). The locate responder will be the one exception (AUTH.md "Locate").
        if !self.person && m.first() != Some(&3) {
            return Some(0);
        }
        // The receives that may be held (the wait byte); every other request is answered at once.
        match m.first() {
            Some(2) if m.len() >= 3 => {
                let port = u16::from_be_bytes([m[1], m[2]]);
                self.open_port(port);
                let n = self.udp_take(port, out);
                if n == 0 && m.len() >= 4 && m[3] > 0 {
                    self.hold(false, port, 0, m[3], 0, tok, out);
                    return None;
                }
                return Some(n);
            }
            Some(10) => return self.resolve(m, tok, out),
            Some(6) => {
                // WHY the checks: the request is the caller's; shorter than its 7 bytes, or a handle past the table, is no connection.
                let i = if m.len() >= 7 { m[1] as usize } else { TCPN };
                if i >= TCPN || self.tcb[i].state == 0 {
                    out[0] = 5;
                    return Some(1);
                }
                self.tcb[i].touched = now();
                // WHY the min: the caller's most, held to what one reply carries.
                let most = u32::from_be_bytes([m[3], m[4], m[5], m[6]]).min(RECV_MAX as u32);
                let c = &self.tcb[i];
                if c.rx_len == 0 && (c.state == 1 || c.state == 2) && m[2] > 0 {
                    self.hold(true, 0, i as u8, m[2], most, tok, out);
                    return None;
                }
                return Some(self.tcp_take(i, out, most));
            }
            _ => {}
        }
        Some(self.serve_now(m, out))
    }

    /// A request answered at once; the reply length written to `out`.
    fn serve_now(&mut self, m: &[u8], out: &mut [u8]) -> usize {
        match m.first() {
            Some(1) if m.len() >= 22 => {
                let addr: [u8; 16] = m[2..18].try_into().unwrap();
                let dport = u16::from_be_bytes([m[18], m[19]]);
                let sport = u16::from_be_bytes([m[20], m[21]]);
                // The resolver's port is its own: a question from it in an application's name could pass for one of ip's own.
                if sport == self.res.port {
                    out[0] = 3;
                    return 1;
                }
                self.open_port(sport);
                out[0] = self.udp_send(m[1], addr, dport, sport, &m[22..]);
                1
            }
            Some(3) => {
                let a4 = self.pick4(&[8, 8, 8, 8]);
                let a6 = self.pick6(&[0x20, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
                out[0..6].copy_from_slice(&self.mac);
                out[6..10].copy_from_slice(&a4.map(|a| a.1).unwrap_or([0; 4]));
                out[10..14].copy_from_slice(&self.mask);
                out[14..18].copy_from_slice(&self.router);
                out[18..34].copy_from_slice(&self.ll);
                out[34..50].copy_from_slice(&a6.map(|a| a.1).unwrap_or([0; 16]));
                out[50] = a4.is_some() as u8 | (a6.is_some() as u8) << 1 | (self.cell.flags & 1) << 2 | (self.person as u8) << 3;
                let (d4, d6) = self.dns_servers();
                out[51..55].copy_from_slice(&d4);
                out[55..71].copy_from_slice(&d6);
                71
            }
            Some(9) if m.len() >= 2 && m[1] <= 2 => {
                self.mode = m[1];
                out[0] = 0;
                1
            }
            Some(4) if m.len() >= 20 => {
                let addr: [u8; 16] = m[2..18].try_into().unwrap();
                let port = u16::from_be_bytes([m[18], m[19]]);
                self.tcp_connect(m[1], addr, port, out)
            }
            Some(5) if m.len() >= 2 => {
                let i = m[1] as usize;
                // WHY: the handle is the caller's; one past the table is no connection.
                if i >= TCPN || self.tcb[i].state == 0 || self.tcb[i].state == 4 || self.tcb[i].fin_queued {
                    out[0] = 5;
                    return 1;
                }
                self.tcb[i].touched = now();
                let c = &self.tcb[i];
                // WHY the min: what the send buffer has room for; the rest is the caller's to send again.
                let n = (m.len() - 2).min(TXB - c.tx_len as usize);
                ring_put(self.tx(i), (c.tx_head + c.tx_len) as usize, &m[2..2 + n]);
                self.tcb[i].tx_len += n as u32;
                out[0] = 0;
                out[1..3].copy_from_slice(&(n as u16).to_be_bytes());
                self.tcp_tick();
                3
            }
            Some(8) if m.len() >= 18 && (m[1] == 4 || m[1] == 6) => {
                let g: [u8; 16] = m[2..18].try_into().unwrap();
                if !self.groups[..self.groups_n].contains(&(m[1], g)) && self.groups_n < self.groups.len() {
                    self.groups[self.groups_n] = (m[1], g);
                    self.groups_n += 1;
                }
                self.group_report(m[1], g);
                out[0] = 0;
                1
            }
            Some(7) if m.len() >= 2 => {
                let i = m[1] as usize;
                // WHY: the handle is the caller's; one past the table is no connection (answered as closed).
                if i < TCPN && self.tcb[i].state != 0 {
                    let c = &mut self.tcb[i];
                    c.touched = now();
                    c.fin_queued = true;
                    c.closed_t = now();
                    if c.state == 4 {
                        let why = c.why;
                        self.tcp_free(i, why);
                    }
                }
                out[0] = 0;
                1
            }
            _ => {
                out[0] = 3;
                1
            }
        }
    }
}

/// An IPv4 packet (no options) into `p`; its length, or 0 when the payload does not fit a 1500-byte packet.
fn ip4(p: &mut [u8; 1500], src: [u8; 4], dst: [u8; 4], proto: u8, payload: &[u8], ttl: u8) -> usize {
    let total = 20 + payload.len();
    if total > p.len() {
        return 0;
    }
    p[0] = 0x45;
    p[2..4].copy_from_slice(&(total as u16).to_be_bytes());
    p[8] = ttl;
    p[9] = proto;
    p[12..16].copy_from_slice(&src);
    p[16..20].copy_from_slice(&dst);
    let c = csum(&p[..20], 0);
    p[10..12].copy_from_slice(&c.to_be_bytes());
    p[20..total].copy_from_slice(payload);
    total
}

/// An IPv6 packet into `p`; its length, or 0 when the payload does not fit a 1500-byte packet.
fn ip6(p: &mut [u8; 1500], src: [u8; 16], dst: [u8; 16], nh: u8, payload: &[u8], hop: u8) -> usize {
    let total = 40 + payload.len();
    if total > p.len() {
        return 0;
    }
    p[0] = 0x60;
    p[4..6].copy_from_slice(&(payload.len() as u16).to_be_bytes());
    p[6] = nh;
    p[7] = hop;
    p[8..24].copy_from_slice(&src);
    p[24..40].copy_from_slice(&dst);
    p[40..total].copy_from_slice(payload);
    total
}

/// RFC 1071 ones'-complement sum: 16-bit big-endian words added into a wide accumulator, carries folded once at the end.
fn sum(data: &[u8]) -> u32 {
    let mut s: u64 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        s += u16::from_be_bytes([data[i], data[i + 1]]) as u64;
        i += 2;
    }
    if i < data.len() {
        s += (data[i] as u64) << 8;
    }
    while s > 0xFFFF {
        s = (s & 0xFFFF) + (s >> 16);
    }
    s as u32
}

fn csum(data: &[u8], init: u32) -> u16 {
    let mut s = sum(data) + init;
    while s > 0xFFFF {
        s = (s & 0xFFFF) + (s >> 16);
    }
    !(s as u16)
}

#[unsafe(no_mangle)]
pub extern "C" fn payload_main(grants: u64, _input: u64, _input_len: u64) -> ! {
    let (base, freq) = (ferros_sys::arc_base(), ferros_sys::freq());
    // The queue lives in `.data` (a non-zero initializer keeps it out of `.bss`, which the flat image does not carry), not on the 64 KiB stack.
    const EMPTY: Datagram = Datagram { used: false, fam: 0, addr: [0; 16], sport: 0, dport: 0, len: 0, seq: 0, data: [1; 1472] };
    #[unsafe(link_section = ".data.queue")]
    static mut QUEUE: [Datagram; QN] = [EMPTY; QN];
    // The connections' control state, and the request and reply buffers (a reply up to `RECV_MAX` bytes of data): statics, not the 64 KiB stack (the kernel maps .bss from the image header's tail).
    static mut TCBS: [Tcb; TCPN] = [TCB_FREE; TCPN];
    static mut MSG: [u8; MSG_MAX] = [0; MSG_MAX];
    // The frames batched for the WiFi driver's next exchange (`Ip::txb`): a call carries 4096 bytes.
    static mut TXB: [u8; MSG_MAX] = [0; MSG_MAX];
    static mut OUT: [u8; 1 + RECV_MAX] = [0; 1 + RECV_MAX];
    // The resolver's cache, lookups and held calls (`Dns`).
    static mut DNS: Dns = Dns { cache: ferros_dns::Cache::new(), asks: [ASK_FREE; RECN], rq: [RQ_FREE; RQN], rq_seq: 0, port: 0 };
    // The heap the kernel maps past .bss (`HEAP_START` of it, or the whole `HEAP_CEILING` on a kernel without SYS_ARC): the first blocks, on a cache line.
    unsafe extern "C" {
        static __image_end: u8;
    }
    let heap = ((&raw const __image_end) as usize + 16 + 63) & !63;
    let (top, _) = ferros_sys::arc::size();
    let blocks = if top as u64 == ERR {
        Blocks { heap, start: (HEAP_CEILING - 64) / BLOCK, top0: 0, top: 0, grows: false, used: 0 }
    } else {
        Blocks { heap, start: (HEAP_START - 64) / BLOCK, top0: top, top, grows: true, used: 0 }
    };
    let mut ip = Ip {
        grants,
        base,
        freq,
        log: [0; 1024],
        n: 0,
        mac: [0; 6],
        v4: [0; 4],
        mask: [0; 4],
        router: [0; 4],
        bound: false,
        dhcp_state: 0,
        xid: 0,
        offer: [0; 4],
        server: [0; 4],
        dhcp_t: 0,
        iid: [0; 8],
        ll: [0; 16],
        global: [0; 16],
        have_global: false,
        r6: [0; 16],
        r6_mac: [0; 6],
        have_r6: false,
        rs_t: 0,
        rs_n: 0,
        arp: [([0; 4], [0; 6]); 8],
        arp_n: 0,
        nd: [([0; 16], [0; 6]); 8],
        nd_n: 0,
        q: unsafe { &mut *(&raw mut QUEUE) },
        frames: 0,
        dns: [0; 4],
        dns6: [0; 16],
        ports: [0; 8],
        ports_at: 0,
        qseq: 0,
        tcb: unsafe { &mut *(&raw mut TCBS) },
        blocks,
        eph: 0,
        seed: 0,
        groups: [(0, [0; 16]); 8],
        groups_n: 0,
        groups_t: 0,
        wifi: false,
        wifi_t: 0,
        cell: CELL_DOWN,
        cell_t: 0,
        person: false,
        person_t: 0,
        dark: false,
        backoff: 0,
        mode: 0,
        held: [HELD_FREE; HELDN],
        held_seq: 0,
        batch: false,
        txb: unsafe { &mut *(&raw mut TXB) },
        txn: EXCH_HEAD,
        path: Path::default(),
        dhcp_n: DhcpCount { discovers: 0, requests: 0, offers: 0, acks: 0, naks: 0, others: 0, unanswered: 0, lease_s: 0 },
        ring: false,
        ring_thread: false,
        ring_none: false,
        res: unsafe { &mut *(&raw mut DNS) },
    };
    // Randomness: the DHCP transaction id and the IPv6 interface identifier (random per session).
    let mut r = [0u8; 16];
    if svc(ENTROPY_SYS, ip.cap(S_ENTROPY), ip.off(r.as_mut_ptr()), 16, 0, 0) != 16 {
        ip.put(b"no entropy right: needs --entropy\n");
        ip.flush();
    }
    let r = &r;
    ip.xid = u32::from_le_bytes([r[0], r[1], r[2], r[3]]);
    ip.seed = u32::from_le_bytes([r[4], r[5], r[6], r[7]]);
    ip.eph = u16::from_le_bytes([r[6], r[7]]);
    ip.iid.copy_from_slice(&r[8..16]);
    // The resolver's one port, random for the session in 49152 to 65535 (RFC 6335's dynamic range; RFC 5452 9.2: a forger must guess it as well as the id). Without entropy it stays 0, a port no datagram is delivered to, and every lookup says it has no way to ask (`res_start`).
    if let Some(p) = ip.rand16() {
        ip.res.port = 0xC000 | p;
    }
    ip.ll[0] = 0xFE;
    ip.ll[1] = 0x80;
    ip.ll[8..].copy_from_slice(&ip.iid);
    ip.put(b"ip up, IPv6 link-local ");
    let ll = ip.ll;
    ip.v6_put(ll);
    ip.put(b"\n");
    // Where the connections' buffers come from (`Blocks`).
    ip.put(b"TCP: A#");
    ip.dec(TCPN as u64);
    ip.put(b" connections, each a block of A#");
    ip.dec((BLOCK >> 10) as u64);
    ip.put(b" KiB (receive window A#");
    ip.dec((RXB >> 10) as u64);
    ip.put(b" KiB); A#");
    ip.dec(ip.blocks.start as u64);
    if ip.blocks.grows {
        ip.put(b" in the heap mapped at spawn, the arc grown for more as they open\n");
    } else {
        ip.put(b" in the heap mapped at spawn, no SYS_ARC on this kernel, so no more\n");
    }
    ip.flush();
    // WiFi: the driver's MAC, then DHCP and a router solicitation (`wifi_up`). No `wlan` grant or no driver serving it: cellular only for now, the driver asked again every 4 s (an error at once while it is not serving: CALL_LIVE).
    ip.wifi_up();
    let mut frame = [0u8; 2048];
    let msg = unsafe { &mut *(&raw mut MSG) };
    let out = unsafe { &mut *(&raw mut OUT) };
    let mut tok = [0u8; 32];
    loop {
        // The process watchdog's pet, once a pass (ferros_sys::pet; measuring, `diag pets`).
        ferros_sys::pet();
        let mut busy = false;
        let pass_t = now();
        ip.path.passes += 1;
        // Frames from the WiFi driver: in batches, our own going with each call (`wlan_batch`), or from a driver without batches [2] → the oldest frame, or nothing. With the doorbell (`WIFI_RING`) only when it rang or frames wait to go, re-armed after the exchange: the driver answers a new ask at once while frames still wait (op 9 looks at its queue when asked), so nothing that came during the exchange is missed, and no empty exchange follows each ring.
        if ip.wifi && ip.batch {
            use core::sync::atomic::Ordering::{Acquire, Release};
            let r = if ip.ring { RING.load(Acquire) } else { 1 };
            if ip.ring && r >= 2 {
                // The driver gone (its next exchange fails and `wifi_up` re-arms), or one without the doorbell: asked every pass again.
                ip.ring = false;
                ip.ring_none = r == 3;
                ip.put(if r == 3 { b"WiFi driver has no doorbell: asking every idle millisecond\n".as_slice() } else { b"WiFi doorbell: the driver went away\n" });
            }
            if !ip.ring || r != 0 || ip.txn > EXCH_HEAD {
                busy |= ip.wlan_batch();
                if ip.ring && r == 1 {
                    ip.path.rings += 1;
                    RING.store(0, Release);
                    ferros_sys::wake(&RING, 2);
                }
            }
        }
        for _ in 0..if ip.wifi && !ip.batch { FRAMES_PASS } else { 0 } {
            let req = [2u8];
            let t0 = now();
            let n = svc(CALL, ip.cap(S_WLAN), ip.off(req.as_ptr()), 1, ip.off(frame.as_mut_ptr()), frame.len() as u64 | CALL_LIVE);
            if n == ERR {
                ip.wifi_down();
                break;
            }
            if n == 0 || n < 14 {
                break;
            }
            ip.path_call(t0);
            ip.path.frames_in += 1;
            ip.path.in_most = 1;
            busy = true;
            // WHY the min: the length is the kernel's, the reply it cut at the buffer.
            let n = (n as usize).min(frame.len());
            ip.wlan_in(&frame[..n]);
        }
        // Packets from the cellular call: [2] → the oldest, or nothing.
        for _ in 0..if ip.cell.flags & 1 != 0 { FRAMES_PASS } else { 0 } {
            let req = [2u8];
            let n = svc(CALL, ip.cap(S_RMNET), ip.off(req.as_ptr()), 1, ip.off(frame.as_mut_ptr()), frame.len() as u64 | CALL_LIVE);
            if n == 0 || n == ERR {
                break;
            }
            let n = (n as usize).min(frame.len());
            busy = true;
            let p = &frame[..n];
            if p[0] >> 4 == 4 {
                ip.ipv4_in(true, [0; 6], p);
            } else if p[0] >> 4 == 6 {
                ip.ipv6_in(true, [0; 6], p);
            }
        }
        // The ACKs the pass's segments still owe (an odd in-order one's), now that the batch is in.
        ip.tcp_acks();
        // The call's state from `rmnet` INFO: every second until it is up, then every 16 s (a call that drops and comes back may come back with new addresses).
        let t = now();
        // The WiFi path said at the first pass 64 s or more after its window began, when no connection's report said it sooner: at rest its passes and waits are the stack's wakes (2026-11-13, the deep-sleep measure: ip woke ~260 times a second in the dark).
        if t.wrapping_sub(ip.path.t0) >= 64 * ip.freq {
            if ip.path.t0 != 0 {
                ip.flush();
                let p = core::mem::take(&mut ip.path);
                p.say(&mut ip);
            }
            ip.path.t0 = t;
        }
        if t.wrapping_sub(ip.cell_t) > if ip.cell.flags & 1 != 0 { 16 } else { 1 } * ip.freq {
            ip.cell_t = t;
            ip.cell_info();
        }
        if t.wrapping_sub(ip.person_t) > ip.freq {
            ip.person_t = t;
            ip.person = ferros_sys::facts().0[83] == 2;
            ip.dark = ferros_sys::pref(ferros_sys::PREF_SCREEN_OFF) == 1;
        }
        // Applications: a polling recv on `udp`, the requests waiting taken a few to a pass (sixteen connections' readers and writers share it).
        for _ in 0..REQS_PASS {
            let n = svc(RECV, ip.cap(S_UDP), ip.off(msg.as_mut_ptr()), msg.len() as u64, ip.off(tok.as_mut_ptr()), 1);
            if n == NOTHING || n == ERR {
                break;
            }
            // WHY the min: the length is the kernel's, the request it cut at the buffer.
            let m = &msg[..(n as usize).min(msg.len())];
            if let Some(l) = ip.serve(m, &tok, out) {
                svc(REPLY, ip.off(tok.as_ptr()), ip.off(out.as_ptr()), l as u64, 0, 0);
            }
            busy = true;
        }
        ip.tcp_tick();
        // The resolver's tries run out, and its held calls' waits.
        ip.dns_tick();
        // The held receives that have their answer now (after what came in and what the connections did this pass).
        if ip.answer_held(out) {
            busy = true;
        }
        // Membership refreshed every 60 s (a querier's interval is 125 s; an unsolicited report keeps the snooper's entry alive without one).
        if ip.groups_n > 0 && now().wrapping_sub(ip.groups_t) > 60 * ip.freq {
            ip.groups_t = now();
            ip.group_reports();
        }
        // Timers: DHCP retransmits every 2 s until bound; router solicitation every 4 s, three times, until a global address appears.
        let t = now();
        if !ip.wifi && t.wrapping_sub(ip.wifi_t) > 4 * ip.freq {
            ip.wifi_up();
        }
        if ip.wifi && !ip.bound && t.wrapping_sub(ip.dhcp_t) > 2 * ip.freq {
            ip.dhcp_retry();
        }
        if ip.wifi && !ip.have_global && ip.rs_n < 3 && t.wrapping_sub(ip.rs_t) > 4 * ip.freq {
            ip.router_solicit();
        }
        ip.flush();
        // Frames batched for the WiFi driver this pass (ACKs, a request, a resend): the next pass's exchange takes them at once, no sleep first.
        busy |= ip.wifi && ip.txn > EXCH_HEAD;
        let dt = now().wrapping_sub(pass_t);
        // WHY the max: the high water, for the measure.
        ip.path.pass_most = ip.path.pass_most.max(dt);
        // Nothing came in either way: a millisecond's sleep, not a spinning core (it spun from boot once init started it, 2026-10-26). With the screen off, each idle pass in a row doubles it up to 8 ms, and anything that comes in brings it back to 1 (2026-11-10: the WiFi and cellular drivers were each asked a thousand times a second in the dark, every one a wakeup of the kernel's core).
        if busy {
            ip.backoff = 0;
        } else {
            let t0 = now();
            if ip.ring || !ip.wifi {
                // The doorbell (`WIFI_RING`), or no WiFi link to ask at all: the wait ends when the doorbell rings, when an application calls (`udp`, polled above), when a preference changes, or at the stack's next timer (`idle_until`). Only a WiFi driver without the doorbell is still asked every idle millisecond (below). With no link, the word is whatever the helper left (2, the driver gone), so it is waited on as it is.
                let expect = if ip.ring { 0 } else { RING.load(core::sync::atomic::Ordering::Acquire) };
                match ferros_sys::wait_world_or_call(&RING, expect, ip.idle_until(t0)) {
                    0 | 1 => ip.path.by_ring += 1,
                    3 => ip.path.by_call += 1,
                    _ => ip.path.by_timer += 1,
                }
            } else {
                ferros_sys::sleep_until(t0 + (ip.freq / 1000 << ip.backoff));
            }
            ip.path.sleeps += 1;
            ip.path.slept_t += now().wrapping_sub(t0);
            ip.backoff = match (ip.dark, ip.backoff) {
                (false, _) => 0,
                (true, 3) => 3,
                (true, b) => b + 1,
            };
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}

// The entry and its image header (ferros_sys::entry!: the kernel sizes the arc from the image and its .bss), its heap growing on demand up to `HEAP_CEILING` for the connections' blocks (`Blocks`).
ferros_sys::entry!(payload_main, grows HEAP_CEILING);
