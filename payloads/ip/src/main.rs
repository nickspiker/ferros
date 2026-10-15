//! The IP process: ARP + IPv4 (DHCP, echo) and IPv6 (neighbour discovery, router solicitation, SLAAC, echo), with UDP for applications. A client of the WiFi driver's `wlan` endpoint (Ethernet frames) and the server of `udp` (datagrams for apps). Photon on ferros needs exactly this (WIFI.md; no TCP, no TLS).
//!
//! Spawn after the WiFi driver serves `wlan` (tools/husky-wifi.sh 14): ferros-bridge spawn ip.bin --name ip --use wlan --serve udp --entropy
//! Grant slots: 0 console, 1 wlan (use), 2 udp (serve), 3 entropy.
//!
//! `udp` requests (first byte):
//! - 1 SEND: family (4 or 6), address (16 bytes; IPv4 in the first 4), destination port (u16 BE), source port (u16 BE), payload → [0] sent, [1] resolving the next hop (retry), [2] no address yet, [3] bad request.
//! - 2 RECV: port (u16 BE) → family, address (16), source port, destination port, payload — or nothing (length 0).
//! - 3 INFO: → MAC (6), IPv4 address, mask, router (4 each), IPv6 link-local (16), IPv6 global (16), flags (bit 0 IPv4 bound, bit 1 IPv6 global).
//!
//! Checksums are ones'-complement sums (RFC 1071: wide adds, one fold at the end). IPv4 UDP goes out with checksum 0 (allowed); IPv6 UDP and every ICMP carry one. The IPv6 interface identifier is random per session (from the entropy right) until a persistent device secret is granted (then RFC 7217 stable-privacy). No duplicate address detection yet.
//!
//! Native payload rule (KERNEL.md): unrelocated at its arc base — no `match` yielding string slices (pointer tables).

#![no_std]
#![no_main]

use core::arch::asm;

const CALL: u64 = 1;
const RECV: u64 = 2;
const REPLY: u64 = 3;
const ENTROPY_SYS: u64 = 6;
const NOTHING: u64 = u64::MAX - 1;
const ERR: u64 = u64::MAX;

const S_CONSOLE: u64 = 0;
const S_WLAN: u64 = 1;
const S_UDP: u64 = 2;
const S_ENTROPY: u64 = 3;

fn svc(n: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    let r: u64;
    unsafe { asm!("svc #0", inlateout("x0") a0 => r, in("x1") a1, in("x2") a2, in("x3") a3, in("x4") a4, in("x8") n, options(nostack)) };
    r
}

fn now() -> u64 {
    let t: u64;
    unsafe { asm!("isb", "mrs {}, cntvct_el0", out(reg) t) };
    t
}

const QN: usize = 8;

struct Datagram {
    used: bool,
    fam: u8,
    addr: [u8; 16],
    sport: u16,
    dport: u16,
    len: usize,
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
    q: [Datagram; QN],
    frames: u32,
}

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
        svc(CALL, self.cap(S_WLAN), self.off(f.as_ptr()), n as u64, self.off(r.as_mut_ptr()), 16);
    }

    // ---- IPv4 ----

    fn ipv4_send(&mut self, dst_mac: [u8; 6], src: [u8; 4], dst: [u8; 4], proto: u8, payload: &[u8]) {
        let mut p = [0u8; 1500];
        let total = 20 + payload.len();
        if total > p.len() {
            return;
        }
        p[0] = 0x45;
        p[2..4].copy_from_slice(&(total as u16).to_be_bytes());
        p[8] = 64;
        p[9] = proto;
        p[12..16].copy_from_slice(&src);
        p[16..20].copy_from_slice(&dst);
        let c = csum(&p[..20], 0);
        p[10..12].copy_from_slice(&c.to_be_bytes());
        p[20..total].copy_from_slice(payload);
        self.eth_send(dst_mac, 0x0800, &p[..total]);
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

    fn ipv4_in(&mut self, src_mac: [u8; 6], p: &[u8]) {
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
        let ours = (self.bound && dst == self.v4) || dst == [255; 4] || (self.bound && (0..4).all(|k| dst[k] == self.v4[k] | !self.mask[k]));
        let body = &p[ihl..total];
        match p[9] {
            // ICMP echo request → reply to the sender's MAC (no resolution needed).
            1 if self.bound && dst == self.v4 && body.len() >= 8 && body[0] == 8 => {
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
            17 if body.len() >= 8 => {
                let sport = u16::from_be_bytes([body[0], body[1]]);
                let dport = u16::from_be_bytes([body[2], body[3]]);
                let ulen = (u16::from_be_bytes([body[4], body[5]]) as usize).clamp(8, body.len());
                if dport == 68 {
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
        let total = 40 + payload.len();
        if total > p.len() {
            return;
        }
        p[0] = 0x60;
        p[4..6].copy_from_slice(&(payload.len() as u16).to_be_bytes());
        p[6] = nh;
        p[7] = hop;
        p[8..24].copy_from_slice(&src);
        p[24..40].copy_from_slice(&dst);
        p[40..total].copy_from_slice(payload);
        self.eth_send(dst_mac, 0x86DD, &p[..total]);
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

    fn icmp6_send(&mut self, dst_mac: [u8; 6], src: [u8; 16], dst: [u8; 16], msg: &mut [u8]) {
        msg[2] = 0;
        msg[3] = 0;
        let c = Self::sum6(&src, &dst, 58, msg);
        msg[2..4].copy_from_slice(&c.to_be_bytes());
        self.ipv6_send(dst_mac, src, dst, 58, msg, 255);
    }

    fn multicast_mac(dst: &[u8; 16]) -> [u8; 6] {
        [0x33, 0x33, dst[12], dst[13], dst[14], dst[15]]
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

    fn ipv6_in(&mut self, src_mac: [u8; 6], p: &[u8]) {
        if p.len() < 40 || p[0] >> 4 != 6 {
            return;
        }
        let plen = (u16::from_be_bytes([p[4], p[5]]) as usize).min(p.len() - 40);
        let nh = p[6];
        let src: [u8; 16] = p[8..24].try_into().unwrap();
        let dst: [u8; 16] = p[24..40].try_into().unwrap();
        let body = &p[40..40 + plen];
        let multicast = dst[0] == 0xFF;
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
                self.icmp6_send(src_mac, dst, src, &mut r[..n]);
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
        } else if nh == 17 && body.len() >= 8 && !multicast {
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
        let slot = match self.q.iter().position(|d| !d.used) {
            Some(i) => i,
            None => return,
        };
        let d = &mut self.q[slot];
        let n = data.len().min(d.data.len());
        d.used = true;
        d.fam = fam;
        d.addr = addr;
        d.sport = sport;
        d.dport = dport;
        d.len = n;
        d.data[..n].copy_from_slice(&data[..n]);
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
        if fam == 4 {
            if !self.bound {
                return 2;
            }
            let dst: [u8; 4] = addr[..4].try_into().unwrap();
            let Some(mac) = self.next_hop4(dst) else { return 1 };
            let me = self.v4;
            self.ipv4_send(mac, me, dst, 17, &u[..n]);
            0
        } else if fam == 6 {
            let link_local = addr[0] == 0xFE && addr[1] & 0xC0 == 0x80;
            if !link_local && !self.have_global {
                return 2;
            }
            let src = if link_local { self.ll } else { self.global };
            let Some(mac) = self.next_hop6(addr) else { return 1 };
            let c = Self::sum6(&src, &addr, 17, &u[..n]);
            let c = if c == 0 { 0xFFFF } else { c };
            u[6..8].copy_from_slice(&c.to_be_bytes());
            self.ipv6_send(mac, src, addr, 17, &u[..n], 64);
            0
        } else {
            3
        }
    }

    /// One `udp` request; returns the reply length written to `out`.
    fn serve(&mut self, m: &[u8], out: &mut [u8]) -> usize {
        match m.first() {
            Some(1) if m.len() >= 22 => {
                let addr: [u8; 16] = m[2..18].try_into().unwrap();
                let dport = u16::from_be_bytes([m[18], m[19]]);
                let sport = u16::from_be_bytes([m[20], m[21]]);
                out[0] = self.udp_send(m[1], addr, dport, sport, &m[22..]);
                1
            }
            Some(2) if m.len() >= 3 => {
                let port = u16::from_be_bytes([m[1], m[2]]);
                let Some(i) = self.q.iter().position(|d| d.used && d.dport == port) else { return 0 };
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
                out[0..6].copy_from_slice(&self.mac);
                out[6..10].copy_from_slice(&self.v4);
                out[10..14].copy_from_slice(&self.mask);
                out[14..18].copy_from_slice(&self.router);
                out[18..34].copy_from_slice(&self.ll);
                out[34..50].copy_from_slice(&self.global);
                out[50] = self.bound as u8 | (self.have_global as u8) << 1;
                51
            }
            _ => {
                out[0] = 3;
                1
            }
        }
    }
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
    let base: u64;
    let freq: u64;
    unsafe {
        asm!("mov {}, x28", out(reg) base);
        asm!("mrs {}, cntfrq_el0", out(reg) freq);
    }
    const EMPTY: Datagram = Datagram { used: false, fam: 0, addr: [0; 16], sport: 0, dport: 0, len: 0, data: [0; 1472] };
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
        q: [EMPTY; QN],
        frames: 0,
    };
    // Who we are: the driver's MAC.
    let mut info = [0u8; 16];
    let q = [3u8];
    svc(CALL, ip.cap(S_WLAN), ip.off(q.as_ptr()), 1, ip.off(info.as_mut_ptr()), 16);
    let info = unsafe { core::ptr::read_volatile(&info) };
    ip.mac.copy_from_slice(&info[..6]);
    // Randomness: the DHCP transaction id and the IPv6 interface identifier (random per session).
    let mut r = [0u8; 16];
    if svc(ENTROPY_SYS, ip.cap(S_ENTROPY), ip.off(r.as_mut_ptr()), 16, 0, 0) != 16 {
        ip.put(b"no entropy right: needs --entropy\n");
        ip.flush();
    }
    let r = unsafe { core::ptr::read_volatile(&r) };
    ip.xid = u32::from_le_bytes([r[0], r[1], r[2], r[3]]);
    ip.iid.copy_from_slice(&r[8..16]);
    ip.ll[0] = 0xFE;
    ip.ll[1] = 0x80;
    ip.ll[8..].copy_from_slice(&ip.iid);
    ip.put(b"ip up, IPv6 link-local ");
    let ll = ip.ll;
    ip.v6_put(ll);
    ip.put(b"\n");
    ip.flush();
    ip.dhcp_state = 1;
    ip.dhcp_send(1);
    ip.router_solicit();
    let mut frame = [0u8; 2048];
    let mut msg = [0u8; 2048];
    let mut out = [0u8; 2048];
    let mut tok = [0u8; 32];
    loop {
        // Frames from the driver: [2] → the oldest frame, or nothing.
        for _ in 0..16 {
            let req = [2u8];
            let n = svc(CALL, ip.cap(S_WLAN), ip.off(req.as_ptr()), 1, ip.off(frame.as_mut_ptr()), frame.len() as u64);
            if n == 0 || n == ERR || n < 14 {
                break;
            }
            let f = unsafe { core::ptr::read_volatile(&frame) };
            let n = (n as usize).min(f.len());
            ip.frames += 1;
            let src_mac: [u8; 6] = f[6..12].try_into().unwrap();
            let ty = u16::from_be_bytes([f[12], f[13]]);
            if ty == 0x0806 {
                ip.arp_in(&f[14..n]);
            } else if ty == 0x0800 {
                ip.ipv4_in(src_mac, &f[14..n]);
            } else if ty == 0x86DD {
                ip.ipv6_in(src_mac, &f[14..n]);
            }
        }
        // Applications: a polling recv on `udp`.
        let n = svc(RECV, ip.cap(S_UDP), ip.off(msg.as_mut_ptr()), msg.len() as u64, ip.off(tok.as_mut_ptr()), 1);
        if n != NOTHING && n != ERR {
            let m = unsafe { core::ptr::read_volatile(&msg) };
            let l = ip.serve(&m[..(n as usize).min(m.len())], &mut out);
            svc(REPLY, ip.off(tok.as_ptr()), ip.off(out.as_ptr()), l as u64, 0, 0);
        }
        // Timers: DHCP retransmits every 2 s until bound; router solicitation every 4 s, three times, until a global address appears.
        let t = now();
        if !ip.bound && t.wrapping_sub(ip.dhcp_t) > 2 * ip.freq {
            let ty = if ip.dhcp_state == 2 { 3 } else { 1 };
            ip.dhcp_send(ty);
        }
        if !ip.have_global && ip.rs_n < 3 && t.wrapping_sub(ip.rs_t) > 4 * ip.freq {
            ip.router_solicit();
        }
        ip.flush();
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe { asm!("svc #0", in("x0") 0xDEADu64, in("x8") 0u64, options(noreturn, nostack)) }
}
