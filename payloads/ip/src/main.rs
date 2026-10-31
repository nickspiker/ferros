//! The IP process: ARP + IPv4 (DHCP, echo) and IPv6 (neighbour discovery, router solicitation, SLAAC, echo), with UDP for applications and a small client-only TCP (owner, 2026-10-26: fetches over plain HTTP, like Broadcom's LTO file; nothing listens). A client of two links — the WiFi driver's `wlan` endpoint (Ethernet frames) and the cellular data call's `rmnet` (raw IP packets: no link layer, the addresses and DNS servers given by the call, no DHCP or SLAAC) — and the server of `udp` (datagrams and connections for apps). Photon itself needs only UDP (WIFI.md).
//!
//! Both links stay up; a packet leaves on WiFi when WiFi holds an address of its family, else on the cellular call (owner, 2026-10-29: both up, prefer WiFi; IPv6 preferred, which the apps choose by asking for family 6). Link-scope and multicast traffic (Photon's LAN beacons) stays on WiFi. A TCP connection keeps the link and source it opened on. Everything it originates goes out with TTL and hop limit 63, both families on both links (`HOP`; owner, 2026-10-19 and 2026-10-31).
//!
//! Spawn after the WiFi driver serves `wlan` (tools/husky-wifi.sh 14): ferros-bridge spawn ip.bin --name ip --use wlan --serve udp --entropy --use rmnet
//! Grant slots: 0 console, 1 wlan (use), 2 udp (serve), 3 entropy, 4 rmnet (use; served by the modem driver, payloads/modem: its calls carry CALL_LIVE, so no driver means no cellular link, not a hang).
//!
//! `udp` requests (first byte):
//! - 1 SEND: family (4 or 6), address (16 bytes; IPv4 in the first 4), destination port (u16 BE), source port (u16 BE), payload → [0] sent, [1] resolving the next hop (retry), [2] no address yet, [3] bad request.
//! - 2 RECV: port (u16 BE) → family, address (16), source port, destination port, payload — or nothing (length 0). A RECV on a port, or a SEND from it, opens it: datagrams are kept only for the last eight ports asked for (a broadcast to a port nobody reads once filled the queue for good), the oldest giving way when the queue is full. IPv4 multicast and IPv6 link-scope multicast (ff02::; Photon's LAN beacons go to 239.104.199.144 and ff02::68c7:9014, port 4384) are delivered like unicast.
//! - 3 INFO: → MAC (6), IPv4 address, mask, router (4 each), IPv6 link-local (16), IPv6 global (16), flags (bit 0 IPv4 bound, bit 1 IPv6 global, bit 2 the cellular call is up), the DNS server (4), then the cellular call's IPv6 DNS server (16) — 71 bytes. The addresses and DNS are the ones traffic of each family leaves with: WiFi's when it has them, else the call's.
//! - 4 TCP CONNECT: family, address (16), port (u16 BE) → [0, handle] connecting, [2] no address yet, [4] no free connection.
//! - 5 TCP SEND: handle, data → [0, accepted u16 BE] (what the send buffer took; send the rest later), [5] not a connection.
//! - 6 TCP RECV: handle → [state, data…]: state 1 connecting, 2 open, 3 the peer closed (no more data once this is empty), 4 failed (reset or timed out), 5 not a connection.
//! - 7 TCP CLOSE: handle → [0]: a FIN after the data already sent; the connection is freed once both sides have closed (or 16 s on).
//! - 8 JOIN: family, group address (16) → [0]: join a multicast group — an IGMPv2 membership report (IPv4) or an MLDv1 listener report (IPv6) now and every 60 s, so a snooping access point forwards the group to us (2026-10-26: the desktop's Photon beacons to 239.104.199.144 never reached the phone until it reported itself a member; broadcast did). The firmware already passes all multicast; the switch was the filter. Up to eight groups.
//! - 9 LINK: mode → [0]: 0 both links, WiFi preferred (the default); 1 cellular only; 2 WiFi only. A test can take the cellular path with WiFi up; existing connections keep their link.
//!
//! TCP here: two connections, a 4 KiB receive window each (what the client has not yet taken; the window is the flow control), a 2 KiB send buffer, MSS 1200, go-back retransmission from the oldest unacknowledged byte (1 s, doubling to 8 s, six tries), no options but MSS on the SYN.
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
    dns: [u8; 4],
    ports: [u16; 8],
    ports_at: usize,
    qseq: u32,
    tcp: [Tcp; TCPN],
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
    /// LINK mode (request 9): 0 both, WiFi preferred; 1 cellular only; 2 WiFi only.
    mode: u8,
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


const TCPN: usize = 2;
const RXW: usize = 4096;
const TXB: usize = 2048;
const MSS: usize = 1200;

/// One client connection (see the header): state 0 free, 1 SYN sent, 2 open, 3 the peer closed, 4 failed. It keeps the link and source address it opened with (`cell`, `src`).
#[derive(Clone, Copy)]
struct Tcp {
    state: u8,
    fam: u8,
    cell: bool,
    src: [u8; 16],
    addr: [u8; 16],
    rport: u16,
    lport: u16,
    iss: u32,
    snd_una: u32,
    snd_nxt: u32,
    snd_max: u32,
    rcv_nxt: u32,
    peer_win: u32,
    rx: [u8; RXW],
    rx_head: usize,
    rx_len: usize,
    tx: [u8; TXB],
    tx_len: usize,
    fin_queued: bool,
    fin_sent: bool,
    fin_ever: bool,
    fin_acked: bool,
    peer_fin: bool,
    rto: u64,
    rto_t: u64,
    tries: u32,
    closed_t: u64,
}
const TCP_FREE: Tcp = Tcp { state: 0, fam: 0, cell: false, src: [0; 16], addr: [0; 16], rport: 0, lport: 0, iss: 0, snd_una: 0, snd_nxt: 0, snd_max: 0, rcv_nxt: 0, peer_win: 0, rx: [0; RXW], rx_head: 0, rx_len: 0, tx: [0; TXB], tx_len: 0, fin_queued: false, fin_sent: false, fin_ever: false, fin_acked: false, peer_fin: false, rto: 0, rto_t: 0, tries: 0, closed_t: 0 };

impl Ip {
    fn cap(&self, slot: u64) -> u64 {
        self.grants + 4 + 32 * slot
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
            self.dec_raw(*b as u32);
        }
    }
    fn dec_raw(&mut self, mut v: u32) {
        let mut b = [0u8; 10];
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

    /// A frame to the driver: [1][Ethernet frame].
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
        let mut r = [0u8; 16];
        svc(CALL, self.cap(S_WLAN), self.off(f.as_ptr()), n as u64, self.off(r.as_mut_ptr()), 16 | CALL_LIVE);
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

    /// Ask the WiFi driver for its MAC (CALL_LIVE: an error at once when no driver serves `wlan`); answered, WiFi is up and DHCP and a router solicitation start.
    fn wifi_up(&mut self) {
        self.wifi_t = now();
        let mut info = [0u8; 16];
        let q = [3u8];
        if svc(CALL, self.cap(S_WLAN), self.off(q.as_ptr()), 1, self.off(info.as_mut_ptr()), 16 | CALL_LIVE) == ERR {
            return;
        }
        self.mac.copy_from_slice(&info[..6]);
        self.wifi = true;
        self.put(b"WiFi driver answers\n");
        self.dhcp_state = 1;
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
        self.arp_n = 0;
        self.nd_n = 0;
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
                self.dec_raw(c.mtu as u32);
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

    /// The MAC for an IPv4 destination: broadcast, a cached neighbour (on-link) or the router's; None after sending an ARP request (the caller retries).
    fn next_hop4(&mut self, dst: [u8; 4]) -> Option<[u8; 6]> {
        if dst == [255; 4] {
            return Some([0xFF; 6]);
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
        if total < ihl {
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
                let mut a = [0u8; 16];
                a[..4].copy_from_slice(&src);
                self.tcp_in(4, a, body);
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
    }

    fn dhcp_in(&mut self, b: &[u8]) {
        if b.len() < 240 || b[0] != 2 || u32::from_be_bytes([b[4], b[5], b[6], b[7]]) != self.xid {
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
                }
            }
            if code == 53 && l >= 1 {
                ty = v[0];
            }
            o += 2 + l;
        }
        if ty == 2 && self.dhcp_state == 1 {
            self.offer = yiaddr;
            self.server = server;
            self.dhcp_state = 2;
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
        } else if ty == 6 {
            self.dhcp_state = 1;
            self.dhcp_send(1);
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
                self.tcp_in(6, src, body);
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
            } else if ty == 134 {
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
            self.tcp_in(6, src, body);
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

    // ---- TCP (client only) ----

    /// A segment of connection `i`: `flags` (FIN 1, SYN 2, RST 4, PSH 8, ACK 16), sequence `seq`, `payload`; the window is the receive buffer's room. Returns whether it went out (false: the next hop is not resolved yet).
    fn tcp_out(&mut self, i: usize, flags: u8, seq: u32, payload: &[u8]) -> bool {
        let c = self.tcp[i];
        let mut seg = [0u8; 20 + 4 + MSS];
        let hl = if flags & 2 != 0 { 24 } else { 20 };
        let n = hl + payload.len();
        seg[0..2].copy_from_slice(&c.lport.to_be_bytes());
        seg[2..4].copy_from_slice(&c.rport.to_be_bytes());
        seg[4..8].copy_from_slice(&seq.to_be_bytes());
        seg[8..12].copy_from_slice(&(if flags & 16 != 0 { c.rcv_nxt } else { 0 }).to_be_bytes());
        seg[12] = ((hl / 4) as u8) << 4;
        seg[13] = flags;
        seg[14..16].copy_from_slice(&((RXW - c.rx_len) as u16).to_be_bytes());
        if hl == 24 {
            seg[20..24].copy_from_slice(&[2, 4, (MSS >> 8) as u8, MSS as u8]);
        }
        seg[hl..n].copy_from_slice(payload);
        // On the link and from the address the connection opened with.
        if c.fam == 4 {
            let dst: [u8; 4] = c.addr[..4].try_into().unwrap();
            let me: [u8; 4] = c.src[..4].try_into().unwrap();
            let mut ph = [0u8; 12];
            ph[..4].copy_from_slice(&me);
            ph[4..8].copy_from_slice(&dst);
            ph[9] = 6;
            ph[10..12].copy_from_slice(&(n as u16).to_be_bytes());
            let ck = csum(&seg[..n], sum(&ph));
            seg[16..18].copy_from_slice(&ck.to_be_bytes());
            self.out4(c.cell, me, dst, 6, &seg[..n])
        } else {
            let ck = Self::sum6(&c.src, &c.addr, 6, &seg[..n]);
            seg[16..18].copy_from_slice(&ck.to_be_bytes());
            self.out6(c.cell, c.src, c.addr, 6, &seg[..n], HOP)
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
        let Some(i) = self.tcp.iter().position(|c| c.state == 0) else {
            out[0] = 4;
            return 1;
        };
        // A fresh initial sequence number and an ephemeral port from the session's randomness (an LCG stepped off the entropy-seeded value).
        self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.eph = 49152 + (self.eph.wrapping_add(1 + (self.seed >> 20) as u16 % 97) % 16384);
        let iss = self.seed;
        let t = now();
        self.tcp[i] = Tcp { state: 1, fam, cell, src, addr, rport: port, lport: self.eph, iss, snd_una: iss, snd_nxt: iss.wrapping_add(1), snd_max: iss.wrapping_add(1), rto: self.freq, rto_t: t + self.freq, ..TCP_FREE };
        self.tcp_out(i, 2, iss, &[]);
        out[0] = 0;
        out[1] = i as u8;
        2
    }

    fn tcp_in(&mut self, fam: u8, src: [u8; 16], seg: &[u8]) {
        let sport = u16::from_be_bytes([seg[0], seg[1]]);
        let dport = u16::from_be_bytes([seg[2], seg[3]]);
        let Some(i) = self.tcp.iter().position(|c| c.state != 0 && c.fam == fam && c.addr == src && c.rport == sport && c.lport == dport) else { return };
        let seq = u32::from_be_bytes([seg[4], seg[5], seg[6], seg[7]]);
        let ack = u32::from_be_bytes([seg[8], seg[9], seg[10], seg[11]]);
        let off = ((seg[12] >> 4) as usize * 4).clamp(20, seg.len());
        let flags = seg[13];
        let win = u16::from_be_bytes([seg[14], seg[15]]) as u32;
        let data = &seg[off..];
        let c = &mut self.tcp[i];
        if flags & 4 != 0 {
            c.state = 4;
            return;
        }
        if c.state == 1 {
            if flags & 18 == 18 && ack == c.iss.wrapping_add(1) {
                c.rcv_nxt = seq.wrapping_add(1);
                c.snd_una = ack;
                c.peer_win = win;
                c.state = 2;
                c.tries = 0;
                self.tcp_out(i, 16, ack, &[]);
            }
            return;
        }
        c.peer_win = win;
        if flags & 16 != 0 {
            // What the ack covers: data from the send buffer's front, then our FIN. Up to the most ever sent: after a go-back the ack can run ahead of what has been resent.
            let acked = ack.wrapping_sub(c.snd_una);
            let in_flight = c.snd_max.wrapping_sub(c.snd_una);
            if acked > 0 && acked <= in_flight {
                let data_acked = (acked as usize).min(c.tx_len);
                c.tx.copy_within(data_acked..c.tx_len, 0);
                c.tx_len -= data_acked;
                if c.fin_ever && ack == c.snd_max {
                    c.fin_acked = true;
                }
                c.snd_una = ack;
                if ack.wrapping_sub(c.snd_nxt) as i32 > 0 {
                    c.snd_nxt = ack;
                }
                c.tries = 0;
                c.rto = self.freq;
                c.rto_t = now() + c.rto;
            }
        }
        let mut reply = false;
        if !data.is_empty() {
            if seq == c.rcv_nxt {
                let take = data.len().min(RXW - c.rx_len);
                for (k, &b) in data[..take].iter().enumerate() {
                    c.rx[(c.rx_head + c.rx_len + k) % RXW] = b;
                }
                c.rx_len += take;
                c.rcv_nxt = c.rcv_nxt.wrapping_add(take as u32);
            }
            reply = true;
        }
        if flags & 1 != 0 && seq.wrapping_add(data.len() as u32) == c.rcv_nxt && !c.peer_fin {
            c.rcv_nxt = c.rcv_nxt.wrapping_add(1);
            c.peer_fin = true;
            if c.state == 2 {
                c.state = 3;
            }
            reply = true;
        }
        if reply {
            let s = c.snd_nxt;
            self.tcp_out(i, 16, s, &[]);
        }
    }

    /// Each pass: new data out (within the peer's window), our FIN once the data is out, retransmission from the oldest unacknowledged byte on timeout, and freeing finished connections.
    fn tcp_tick(&mut self) {
        let t = now();
        for i in 0..TCPN {
            let c = self.tcp[i];
            if c.state == 0 {
                continue;
            }
            if (c.fin_acked && c.peer_fin) || (c.closed_t != 0 && t.wrapping_sub(c.closed_t) > 16 * self.freq) {
                self.tcp[i] = TCP_FREE;
                continue;
            }
            if c.state == 4 {
                continue;
            }
            let in_flight = c.snd_max.wrapping_sub(c.snd_una);
            if in_flight > 0 && t > c.rto_t {
                if c.tries >= 6 {
                    self.tcp[i].state = 4;
                    continue;
                }
                let c = &mut self.tcp[i];
                c.tries += 1;
                c.rto = (c.rto * 2).min(8 * self.freq);
                c.rto_t = t + c.rto;
                if c.state == 1 {
                    let iss = c.iss;
                    self.tcp_out(i, 2, iss, &[]);
                    continue;
                }
                // Go back to the oldest unacknowledged byte.
                c.snd_nxt = c.snd_una;
                c.fin_sent = false;
            }
            if self.tcp[i].state < 2 {
                continue;
            }
            // New data, MSS at a time, within the peer's window.
            loop {
                let c = self.tcp[i];
                let sent = c.snd_nxt.wrapping_sub(c.snd_una) as usize;
                let unsent = c.tx_len.saturating_sub(sent);
                let room = (c.peer_win as usize).saturating_sub(sent);
                let n = unsent.min(room).min(MSS);
                if n == 0 {
                    break;
                }
                let mut p = [0u8; MSS];
                p[..n].copy_from_slice(&c.tx[sent..sent + n]);
                if !self.tcp_out(i, 16 | 8, c.snd_nxt, &p[..n]) {
                    break;
                }
                let c = &mut self.tcp[i];
                if c.snd_una == c.snd_nxt {
                    c.rto_t = t + c.rto.max(self.freq);
                }
                c.snd_nxt = c.snd_nxt.wrapping_add(n as u32);
                if c.snd_nxt.wrapping_sub(c.snd_max) as i32 > 0 {
                    c.snd_max = c.snd_nxt;
                }
            }
            let c = self.tcp[i];
            if c.fin_queued && !c.fin_sent && c.snd_nxt.wrapping_sub(c.snd_una) as usize >= c.tx_len {
                if self.tcp_out(i, 16 | 1, c.snd_nxt, &[]) {
                    let c = &mut self.tcp[i];
                    if c.snd_una == c.snd_nxt {
                        c.rto_t = t + c.rto.max(self.freq);
                    }
                    c.snd_nxt = c.snd_nxt.wrapping_add(1);
                    if c.snd_nxt.wrapping_sub(c.snd_max) as i32 > 0 {
                        c.snd_max = c.snd_nxt;
                    }
                    c.fin_sent = true;
                    c.fin_ever = true;
                }
            }
        }
    }

    /// One `udp` request; returns the reply length written to `out`.
    fn serve(&mut self, m: &[u8], out: &mut [u8]) -> usize {
        match m.first() {
            Some(1) if m.len() >= 22 => {
                let addr: [u8; 16] = m[2..18].try_into().unwrap();
                let dport = u16::from_be_bytes([m[18], m[19]]);
                let sport = u16::from_be_bytes([m[20], m[21]]);
                self.open_port(sport);
                out[0] = self.udp_send(m[1], addr, dport, sport, &m[22..]);
                1
            }
            Some(2) if m.len() >= 3 => {
                let port = u16::from_be_bytes([m[1], m[2]]);
                self.open_port(port);
                // The port's oldest datagram.
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
            Some(3) => {
                // The addresses traffic leaves with (`pick4`, `pick6` for a global destination), and the DNS server of the link IPv4 leaves on.
                let a4 = self.pick4(&[8, 8, 8, 8]);
                let a6 = self.pick6(&[0x20, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
                out[0..6].copy_from_slice(&self.mac);
                out[6..10].copy_from_slice(&a4.map(|a| a.1).unwrap_or([0; 4]));
                out[10..14].copy_from_slice(&self.mask);
                out[14..18].copy_from_slice(&self.router);
                out[18..34].copy_from_slice(&self.ll);
                out[34..50].copy_from_slice(&a6.map(|a| a.1).unwrap_or([0; 16]));
                out[50] = a4.is_some() as u8 | (a6.is_some() as u8) << 1 | (self.cell.flags & 1) << 2;
                out[51..55].copy_from_slice(if matches!(a4, Some((true, _))) { &self.cell.dns4 } else { &self.dns });
                out[55..71].copy_from_slice(&self.cell.dns6);
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
                if i >= TCPN || self.tcp[i].state == 0 || self.tcp[i].state == 4 || self.tcp[i].fin_queued {
                    out[0] = 5;
                    return 1;
                }
                let c = &mut self.tcp[i];
                let n = (m.len() - 2).min(TXB - c.tx_len);
                let at = c.tx_len;
                c.tx[at..at + n].copy_from_slice(&m[2..2 + n]);
                c.tx_len += n;
                out[0] = 0;
                out[1..3].copy_from_slice(&(n as u16).to_be_bytes());
                self.tcp_tick();
                3
            }
            Some(6) if m.len() >= 2 => {
                let i = m[1] as usize;
                if i >= TCPN || self.tcp[i].state == 0 {
                    out[0] = 5;
                    return 1;
                }
                let was_low = RXW - self.tcp[i].rx_len < RXW / 2;
                let c = &mut self.tcp[i];
                out[0] = match c.state {
                    1 => 1,
                    2 => 2,
                    3 => 3,
                    _ => 4,
                };
                let n = c.rx_len.min(out.len() - 1);
                for k in 0..n {
                    out[1 + k] = c.rx[(c.rx_head + k) % RXW];
                }
                c.rx_head = (c.rx_head + n) % RXW;
                c.rx_len -= n;
                // A window that had shrunk below half has grown: say so (a window update), or a sender avoiding silly windows waits for its persist timer.
                if was_low && n > 0 && c.state >= 2 {
                    let s = c.snd_nxt;
                    self.tcp_out(i, 16, s, &[]);
                }
                1 + n
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
                if i < TCPN && self.tcp[i].state != 0 {
                    let c = &mut self.tcp[i];
                    c.fin_queued = true;
                    c.closed_t = now();
                    if c.state == 4 {
                        self.tcp[i] = TCP_FREE;
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
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, _input: u64, _input_len: u64) -> ! {
    let (base, freq) = (ferros_sys::arc_base(), ferros_sys::freq());
    // The queue lives in `.data` (a non-zero initializer keeps it out of `.bss`, which the flat image does not carry), not on the 64 KiB stack.
    const EMPTY: Datagram = Datagram { used: false, fam: 0, addr: [0; 16], sport: 0, dport: 0, len: 0, seq: 0, data: [1; 1472] };
    #[unsafe(link_section = ".data.queue")]
    static mut QUEUE: [Datagram; QN] = [EMPTY; QN];
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
        ports: [0; 8],
        ports_at: 0,
        qseq: 0,
        tcp: [TCP_FREE; TCPN],
        eph: 0,
        seed: 0,
        groups: [(0, [0; 16]); 8],
        groups_n: 0,
        groups_t: 0,
        wifi: false,
        wifi_t: 0,
        cell: CELL_DOWN,
        cell_t: 0,
        mode: 0,
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
    ip.ll[0] = 0xFE;
    ip.ll[1] = 0x80;
    ip.ll[8..].copy_from_slice(&ip.iid);
    ip.put(b"ip up, IPv6 link-local ");
    let ll = ip.ll;
    ip.v6_put(ll);
    ip.put(b"\n");
    ip.flush();
    // WiFi: the driver's MAC, then DHCP and a router solicitation (`wifi_up`). No `wlan` grant or no driver serving it: cellular only for now, the driver asked again every 4 s (an error at once while it is not serving: CALL_LIVE).
    ip.wifi_up();
    let mut frame = [0u8; 2048];
    let mut msg = [0u8; 2048];
    let mut out = [0u8; 2048];
    let mut tok = [0u8; 32];
    loop {
        let mut busy = false;
        // Frames from the driver: [2] → the oldest frame, or nothing.
        for _ in 0..if ip.wifi { 16 } else { 0 } {
            let req = [2u8];
            let n = svc(CALL, ip.cap(S_WLAN), ip.off(req.as_ptr()), 1, ip.off(frame.as_mut_ptr()), frame.len() as u64 | CALL_LIVE);
            if n == ERR {
                ip.wifi_down();
                break;
            }
            if n == 0 || n < 14 {
                break;
            }
            let f = &frame;
            let n = (n as usize).min(f.len());
            ip.frames += 1;
            busy = true;
            let src_mac: [u8; 6] = f[6..12].try_into().unwrap();
            let ty = u16::from_be_bytes([f[12], f[13]]);
            if ty == 0x0806 {
                ip.arp_in(&f[14..n]);
            } else if ty == 0x0800 {
                ip.ipv4_in(false, src_mac, &f[14..n]);
            } else if ty == 0x86DD {
                ip.ipv6_in(false, src_mac, &f[14..n]);
            }
        }
        // Packets from the cellular call: [2] → the oldest, or nothing.
        for _ in 0..if ip.cell.flags & 1 != 0 { 16 } else { 0 } {
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
        // The call's state from `rmnet` INFO: every second until it is up, then every 16 s (a call that drops and comes back may come back with new addresses).
        let t = now();
        if t.wrapping_sub(ip.cell_t) > if ip.cell.flags & 1 != 0 { 16 } else { 1 } * ip.freq {
            ip.cell_t = t;
            ip.cell_info();
        }
        // Applications: a polling recv on `udp`.
        let n = svc(RECV, ip.cap(S_UDP), ip.off(msg.as_mut_ptr()), msg.len() as u64, ip.off(tok.as_mut_ptr()), 1);
        if n != NOTHING && n != ERR {
            let m = &msg;
            let l = ip.serve(&m[..(n as usize).min(m.len())], &mut out);
            svc(REPLY, ip.off(tok.as_ptr()), ip.off(out.as_ptr()), l as u64, 0, 0);
            busy = true;
        }
        ip.tcp_tick();
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
            let ty = if ip.dhcp_state == 2 { 3 } else { 1 };
            ip.dhcp_send(ty);
        }
        if ip.wifi && !ip.have_global && ip.rs_n < 3 && t.wrapping_sub(ip.rs_t) > 4 * ip.freq {
            ip.router_solicit();
        }
        ip.flush();
        // Nothing came in either way: a millisecond's sleep, not a spinning core (it spun from boot once init started it, 2026-10-26).
        if !busy {
            ferros_sys::sleep_until(now() + ip.freq / 1000);
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
