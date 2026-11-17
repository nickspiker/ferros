# WIFI — BCM4398 on PCIe ch1 (husky / zuma): bare-metal bring-up recipe

**Status (2026-10-15): goals 1-3 reached on husky, cold boot, from a driver process.** `tools/husky-wifi.sh` runs `payloads/pcie1` (EL0, hardware grants only) through the recipe below:
- survey: every PCIe ch1 CMU gate already on (G#0020_0000: CG_VAL=1, MANUAL=0, no auto-gating), PMU PHY control G#C0 (isolated), DBI answers G#A5A5_144D;
- PHY: external PLL lock 1 µs, PHY PLL lock 97 µs, CDR lock 29 µs, offset calibration 6.7 ms;
- link: LTSSM Polling.Active → **L0 in 81 µs**, link status G#3011_0000 = **Gen1 ×1** (Gen1 forced), RC id G#ECED_144D;
- config space: **G#14E4 : G#4444** (BCM4398 D11AX); BAR0 → chipcommon ChipID **G#6003_4398 = chip G#4398 rev 3**.
- NCI enumeration (stage 04 + `tools/nci-parse.py`, following nciutils.c nci_scan): EROM pointer G#1800_7118 (EROM1 G#1800_7000, OOBR G#1800_6000). Cores: chipcommon G#800 rev 76 @ G#1800_0000; PCIe2 G#83C rev 130 @ G#1800_1000; **ARM CA7 G#847 rev 13 @ G#1808_0000** (OOBR +G#240, DMP control at +8); **sysmem G#849 rev 14 @ G#1808_8000 with RAM at backplane 0..G#29_FFFF and G#2A_0000 + 4 MiB**; D11 G#812 ×3 @ G#1808_2000/4000/6000; all interfaces Booker. (The download path: hold the CA7 in reset with CPUHALT, write the image, NVRAM vars + random seed, the reset instruction, release — dhd_pcie.c `_dhdpcie_download_firmware` / `__dhdpcie_bus_download_state`; no FW_SIGNATURE on this board.)
- measured before writing (stage 05): BAR0 64 KiB, BAR1 4 MiB (both 64-bit); DAR security status G#0 (no secure boot, no signature file); CA7 core info G#D047, DMP control G#21 (clock on + CPUHALT), AMNI reset control bit 0 clear — after WL_REG_ON the ARM is already out of reset and halted, so dhd's core reset is skipped. OTP CIS (G#1801_115C..): vendor tuple `80 03 81 57 99` → VID 57 99 = module **USI G5SN V17** (`bcmdhd.cal_USI_G5SN_9957_V17`, byte-identical to `cal_ROW`/`JP`/`MMW`; `cal_NA` differs only in FTM ranging calibration). Chip rev 3 = **d0** (dhd_custom_cis.c chip_revs).
- RAM (stage 06): sysmem coreinfo G#0B20_1000 → 32 banks × 128 KiB = **4 MiB at backplane G#2A_0000** (dhd's NCI default: the sysmem's second slave port). BAR1 at G#6040_0000, window = cfg G#84 (4 MiB-aligned backplane base, dhdpcie_setbar1win); write/read round trips pass in both windows.
- **Firmware boots (stage 07-08, 2026-10-15):** `fw_bcmdhd.bin_4398_d0` written at G#2A_0000 and verified word for word (3.9 s through the syscall path), NVRAM vars below the length token at the top of RAM (`process_nvram_vars`, token `(~w << 16) | w`), a 128-byte RNG seed rTLV (type G#FEEDC0DE, no length checksum) drawn from the session wairua through the kernel's entropy capability, reset instruction (the image's first word) at address 0, CPUHALT cleared in the OOBR DMP control. The last RAM word first changes after 36 ms to an early, temporary value; the **real shared area appears at ~430 ms** (G#0061_0000: flags G#7025_0509 = IPC v9, console G#0052_375C, rings_info G#0061_008C, h2d/d2h mailboxes G#0061_0600/0604) — dhd only ever sees the second, because it reads after the boot interrupt. The firmware console (hnd_cons_t log ring) reads: `RTE (PCIE-MSGBUF) 24.100.671.82.2 (g998bb28 d0 release2) on BCM4398 r3 @ 39.960/258.400/1411.0MHz`, wl0 (5G) / wl1 (2G) / wl2 (2G+5G) attached, `Firmware boot took 429457 us`.
- **MSI:** the RC's DesignWare MSI receiver (DBI G#820/G#824 address, G#828 enable, G#82C mask, G#830 status) and the EP's MSI capability (cfg G#58, 64-bit) both point at G#F000 (boot ROM space, so a write that ever escaped interception lands nowhere); the boot-complete MSI arrives as RC MSI status bit 0, and the firmware logs `Single MSI enabled`. **The firmware then runs idle without trapping, waiting for the host's rings.**
- **Trap to avoid:** once booted the firmware disables backplane G#0..G#29_FFFF (reads return all ones); a *host* read there through BAR1 is an AXI slave error on the dongle's s_pcie master and the firmware takes a FIQ trap (`bp_to=2`). The shared area's flags carry it (PCIE_SHARED_TRAP G#400, trap_addr non-zero). Don't touch address 0 after release.
- **First ioctl (stage 09):** the five common rings in the fenced `pcie1-dma` region (dhd's smallest sizes: ctrl submit 64 × 40, rxpost 512 × 32, ctrl complete 64 × 24, tx complete 1024 × 24, rx complete 512 × 40), read/write indices left in TCM (rings_info pointers; no DMA'd index arrays, so every completion is checked by epoch = seq % 253 from 254 and a zero XOR over its six words), host_cap G#0080_1409 (IPC 9, hostready, deep sleep off, no trap on Unsupported Request) at shared +84, an 8-byte scratch at +52/+56, hostready on H2D doorbell 1 (BAR0 + G#2144), 8 ioctl response buffers (8 KiB) and 8 event buffers (2 KiB) posted, then WLC_GET_VAR (262) "ver" with doorbell 0 (BAR0 + G#2140): ack after 5.2 ms, completion 7 µs later — `wl0: Nov 21 2025 14:44:15 version 24.100.671.82.2 (g998bb28 d0 release2) FWID 01-eed23c89`. `cur_etheraddr` answers too (stage 10).
- **Scan (stage 11, first run):** `country` reads `#n` (no regulatory table in the firmware) → `clmload` of `bcmdhd_clm.blob_4398_d0` in 1408-byte `wl_dload_data_t` chunks (flag 1 << 12 | begin 2 / end 4, type 2) → status 0, `country` then reads `US`; `event_msgs_ext` GET/SET with WLC_E_ESCAN_RESULT (69) added; WLC_UP (2) → 0; `scan_ver` = 3 (the v3 escan params: version/length prefix, 72 bytes); `escan` (version 3, action 1, broadcast SSID, BSSID ff…ff, any BSS type, firmware-default times, all channels) → events of type 69 with status 8 (one BSS each) and a final 0. **8 BSSes on 2.4, 5 and 6 GHz in 4.7 s.** Event packets are bcm_event_t (ether type G#886C, big-endian wl_event_msg_t, data at +72; rx_dataoffset from shared +36); wl_bss_info is read only through its version-independent prefix (chanspec +72, RSSI +78, ctl_ch +88, ie_offset +116 / ie_length +120).
- **Join (stage 12, 2026-10-15): associated and keyed in 241 ms**, WPA2-PSK on 5 GHz, the firmware's own supplicant doing the 4-way handshake. Order (dhd wl_cfg80211_connect): receive buffers posted (rxpost ring 1, 64 × 2 KiB) → `sup_wpa` 1, `roam_off` 1, `buf_key_b4_m4` 1, WLC_SET_PM (86) 0, WLC_SET_INFRA (20) 1 → `wsec`/`auth`/`wpa_auth` cleared → `wpa_auth` G#C0, `auth` 0, `wsec` 4 (AES), `mfp` (1 if the AP's RSN caps have MFPC), `wpa_auth` G#80 (WPA2-PSK) → WLC_SET_WSEC_PMK (268): 263-byte wsec_pmk_t, key_len, flags 1 (passphrase), the passphrase → our RSN IE via `wpaie` (version 1, the AP's group cipher, pairwise CCMP, AKM PSK, caps) → `join_ver` (2 here) → `join` (v2: ssid +4, scan params +40, BSSID +64, bssid_cnt +70, chanspec_num +76, one chanspec +80). Events: 87, AUTH 3, 88, LINK 16 (flags 1), ASSOC 7, then **PSK_SUP 46 status 6 (KEYED) reason 0** = authorized. The credential is `keys/wifi.txt` (SSID line, passphrase line), an input file of the driver, never printed or committed.
- Two lessons: **with no receive buffers posted the handshake never starts** (split RX: the AP's EAPOL frames land in host buffers; associated, no PSK_SUP, beacon loss after 15 s). And a first 2.4 GHz join (40 MHz, control 11) lost the link waiting for the first beacon (WLC_E_LINK reason 8, NO_FIRST_BCN_RX) — before rx buffers existed, so not yet re-tested; the driver prefers 5 GHz. Re-tested 2026-11-04, with receive buffers: three 2.4 GHz joins (channel 11, −36 dBm) failed the same way, while Bluetooth advertised the pairing beacon on the shared antenna; the first 5 GHz join after (channel 149, −55 dBm) keyed and leased at once. The driver now takes 5 GHz whenever the network has it, 2.4 GHz only where it has not, until WiFi and Bluetooth share the antenna properly (coexistence).
- **DHCP (stage 13, 2026-10-15): the device leases an address** — OFFER and ACK from the router, a /24 with router, DNS and a ~24 h lease — over a transmit flowring from the EL0 driver. Receive: a frame starts at `data_offset` if non-zero, else at pcie_shared `dma_rxoffset` (8 here), as a plain Ethernet frame (the "wrong EtherType" was reading from offset 0; 802.11 management frames also arrive, flagged G#02). Transmit: flowring 2 (msgbuf ring 5, H2D index slot 2) created on the control ring with MSG_TYPE_FLOW_RING_CREATE (G#03: DA broadcast, SA our MAC, flow_ring_id 2, 512 × 48 B, ring address) → create completion G#04 status 0; each frame is host_txbuf_post v1 (G#0F, 48 B: the 14-byte Ethernet header inline, flags G#01, seg_cnt 1, data address past the header, data_len − 14) + the flowring's TCM write index + doorbell 0 `G#FF000000 | 5 << 16 | wr`; tx completions (G#10) on D2H ring 3 report status 0. DISCOVER/REQUEST are broadcast with the BOOTP broadcast flag and a zero UDP checksum.
- **Photon on ferros needs only ARP + IPv4 + IPv6 + UDP** (decided with the owner 2026-10-15; IPv6 with neighbour discovery and SLAAC, as Photon binds dual-stack): no fgtw.org bootstrap, no seed, no TCP/HTTP/TLS/DNS. Peers come from physical pairing into the fleet (the phonebook is handed over and kept in the vault) and LAN broadcast discovery; addresses are hints, identity is proven by signatures. Next: the WiFi driver as a frame server, an IP process (ARP, IPv4, DHCP, IPv6 NDP/SLAAC, UDP) serving UDP to apps, then a no_std Photon core — each its own process.
- **The network stack runs (2026-10-15, `tools/husky-net.sh`):** three EL0 processes over capabilities — the WiFi driver serving Ethernet frames on `wlan` (stage 14: SEND / RECV / INFO; received frames wait in their DMA buffers until taken; `allmulti` on for IPv6 neighbour discovery), `payloads/ip` (ARP, IPv4 + DHCP + echo, IPv6 NDP + router solicitation + SLAAC + echo, UDP; serves `udp`), `payloads/nettest` (UDP echo on 7777). DHCP lease and a SLAAC global IPv6 address; from another machine: ping 3/3 over IPv4 and IPv6, UDP echo round trips over both. The firmware offers TX checksum offload (flags2 TXPOST_EXT) — not used yet; `ip` sums in software (RFC 1071). Rx frames: length = data_len from the offset (subtracting the offset clipped 8 bytes); completion flags don't separate 802.11 from Ethernet frames, so all go to `ip`.
- **At boot from the vault (2026-10-26):** init starts the WiFi driver and `ip` after the shell; the driver, given no input files, takes its firmware, NVRAM, CLM blob and credential from the vault scope `wifi.` and runs the whole bring-up (keyed about 8 s after its start). Both loops sleep 1 ms when idle instead of spinning a core. A restart of the driver re-joins at once, and the router then refuses a second join for about ten minutes (devbox_network_for_phone_tests): when iterating on `ip`, restart `ip` alone.
- **Client TCP + DNS (2026-10-26, owner: "we probably need the full TCP/UDP/IP"):** `ip` grew a client-only TCP (ops 4-7 on `udp`; two connections, 4 KiB receive window, 2 KiB send buffer, MSS 1200, go-back retransmission 1 s doubling to 8 s; nothing listens) and passes on the DHCP-given DNS server (INFO +51). `payloads/httptest` resolves a name through it and fetches over plain HTTP: Broadcom's LTO file, **189 047 bytes in 1.9 s, FNV-1a 64 equal to the host's fetch**. The window, not the link, bounds that rate (4 KiB per round trip). This widens the 2026-10-15 "UDP only" line for HTTP fetches (orbit data); Photon itself still needs only UDP. No TLS.
- **Client TCP for throughput (2026-11-11, owner: "sounds like it needs re-engineered"):** Mahere's 1.6 MB base cells crawled thru two connections of a 4 KiB window (about 80 KB/s at a 50 ms round trip). Now (payloads/ip header, the protocol's pure parts in `ferros_tcp`, tested on the host):
  - Sixteen connections; an idle one is its control state alone (a static table), an open one holds a block of ip's arc, a 1 MiB receive buffer and a 256 KiB send buffer. The kernel maps 4 MiB of heap at spawn (three blocks); ip's arc grows for more as connections open (SYS_ARC, ARCS.md) and shrinks when the highest blocks are free again, masked or native.
  - The window is the receive buffer's free room, scaled (RFC 7323, shift 5; 64 KiB for a peer without scaling), its edge moved by a segment or half the buffer at least, never back; a read that reopens a window shrunk under half is a window update at once.
  - Segments past a hole are kept in the buffer where they belong (eight ranges) and delivered when it fills; each is ACKed at once with SACK blocks (RFC 2018). Every second in-order segment and one with PSH are ACKed at once (RFC 5681, RFC 1122), an odd one at the end of its batch of frames (no delayed-ACK timer: the batch is microseconds, and ip never sleeps with an ACK owed), at once for FIN, out-of-order data and filled holes ("ACKs" below).
  - Sending: the peer's window and a congestion window (slow start, congestion avoidance, fast retransmit, NewReno; RFC 5681, 6582); the retransmission timeout from measured round trips (RFC 6298, Karn), 250 ms to 8 s, the eighth expiry failing the connection; a closed peer window probed on the same timer. The SACK blocks peers send are read, not yet used.
  - The MSS from the link's MTU (WiFi 1500, the call's own) less the headers; 1200 when the call reports nothing believable. Every incoming segment's checksum is checked; a reset counts only at the next expected byte, a SYN or a reset elsewhere in the window gets a challenge ACK (RFC 5961).
  - Ops (no versions: every caller changed with it, owner: "No existing callers yet, so no need to version it"): CONNECT answers 6 when no block can be had; SEND takes up to 4094 bytes (a call carries 4096); RECV is `[6, handle, wait, most u32]` and answers up to 256 KiB (the kernel's largest reply), a held one also when the connection's state changes; INFO is 71 bytes again. The callers: fgtw's post, the kist stower's fetch (now held receives, its millisecond polls gone), httptest, Mahere.
  - Each connection's measures, one console line when it is freed: how it ended, bytes in and out, time open, peak (quarter seconds) and average MiB/s, round trip and its variation and timeout, segments resent (fast retransmits, timer expiries), segments held out of order and the most bytes so held, the most bytes waiting for the app of the window's most, the MSS and scale each way, SACK, the congestion window at the end, data segments in, ACKs out and the most bytes taken between two of them; then, for a WiFi connection, the WiFi path since the last report (passes, sleeps, calls to the driver and their time, frames each way).
- **The receive path, measured (2026-11-11):** with the TCP above, Mahere's cells still came at about 0.2 MiB/s a connection, 0.6 to 0.8 in all, with no loss, no reordering and the window never near full: the cap was under TCP. Measured on the Pro, read only:
  - `diag sched`, Mahere's threads busy on the four worker cores: ip → pcie1 waited A#9740 µs on average for the driver (A#76 calls in A#877 ms). One frame a call, and the driver, after each reply, at the back of the run queue behind 10 ms slices; a call to it asleep wakes it at the back too. The driver's own time a request: A#808 µs for a full frame, A#200 µs for a short one (copied a byte at a time out of non-cacheable DMA memory).
  - `diag sched` at idle: a pass of the driver's loop took A#460 to A#550 µs before it looked for a request, and with frames queued and no request it spun (10 ms of poll misses while ip waited for a core).
  - `flame` at idle (2 s): pcie1 used A#790 ms of core time, A#550 ms of it in register reads, its three reads a pass of the chip's ring write indices over a PCIe link mostly in L1.2 (the link states line). The kernel takes its one lock for every syscall, so each read held it; spinning on the lock was A#17 % of all five cores' time.
  - The SYN expiry most connections showed (expiries A#1, resent A#0) and the round trips of 500 to 830 ms: frames waiting in the driver's queue behind the other connections' data at a hundred or so a second, past the SYN's one-second timeout.
  - Now: `wlan` op 8 exchanges frames both ways in one call (up to 128 KiB of received frames a reply, a 4096-byte batch of ip's frames a request), so the waits and passes are paid per batch; ip ACKed once a batch (until 2026-11-12, "ACKs" below); the driver copies frames a 64-bit word at a time, sleeps whenever no request waits (a call wakes it), publishes ring indices and doorbells once a batch, keeps 32 tx buffers (128 since 2026-11-12) and reuses one only after the chip completes it, and reads its completion rings from host memory alone (each item's epoch and XOR, dhd's sync mode), never the chip's write indices. Measures on the console: pcie1's `rx path, last 32 s` line with the link states, ip's `WiFi path since the last report` after each WiFi connection's report.
  - The kernel, after (branch kernel-sched, 2026-11-11, not yet measured on a device): a register access thru the syscall takes no lock when its token is presented from its grants-page slot (process.rs `mmio_fast`, the race argument with it); a poller woken by a call runs next on the caller's core, and a server after its reply goes to the front while it runs under an eighth of a core (`enqueue_woken`, the fairness bound with it); a sleep ends early for a call already queued on an endpoint the sleeper polls (returns 1). `diag sched` (and `diag "sched reset"`) leads with the counts that say whether it worked: wake to run per kind (`call` is ip → pcie1's wait for the driver, A#9740 µs before), lock waits per core, register syscalls without and with the lock.
- **ACKs (2026-11-12, owner: "the connectivity or cache or something is stupidly slow"):** with frames in batches ip had ACKed in-order data once a batch, as a receiver coalescing segments does. The senders starved: the Pro's close reports (WiFi, round trip 113 to 127 ms) said `segments in A#143, ACKs out A#17 (most between two A#53040 bytes)` and `segments in A#102, ACKs out A#11`, so a sender that grows its window and paces by ACKs kept a small window, and a batch's one ACK carried up to 88 segments' worth.
  - Now every second in-order segment is ACKed at once again, PSH at once, an odd one at its batch's end; ip's loop never sleeps with an ACK owed (the batch waiting in `txb` keeps the pass busy, and the next exchange carries it). The ACKs of a batch go out together in the next exchange: up to 44 for a reply of 88 full frames, more than the 32 transmit buffers pcie1 kept, and a batch's last ACK refused would leave its sender waiting for its timeout. pcie1 keeps 128 now, above the receive buffers in the DMA region's upper MiB.
  - Each close report says the ACKs a segment (`ACKs out A#n, A#x.yy a segment`). On the 8a (WiFi, round trip 23 to 36 ms, a 421 KB cell by httptest, five each): before, `ACKs out A#29` to `A#35` of `A#311` segments (about A#0.10 a segment, most between two A#28560 to A#84428 bytes), 222 to 271 ms; after, A#0.53 to A#0.62 a segment, most between two A#2720 bytes, none refused by the driver, 199 to 236 ms (the 8a's short round trip hid the starvation; the Pro's shows it).
  - Seen on the Pro at the same time, and not the ACKs: its WiFi chip stopped answering (`pcie1: no ioctl completion` A#247 times in one console read, no transmit completions, every frame refused), and each of pcie1's measure ioctls waited out its timeout, so one exchange with ip took A#21 s (`A#3 calls to the driver (A#21004655 us on average)`) and Mahere's fetches failed "nothing for 32 s" (a later read: A#648 such lines, every connection "never opened"). Since then pcie1 ends after four ioctls in a row go uncompleted while serving, says the firmware's shared flags and console, and init's keeper starts it again, which cycles WL_REG_ON (not yet run on a device).
- **The Pro's firmware lost under transmit load (2026-11-12 to 2026-11-14): two differences from Android's dhd, both fixed.** The firmware console said `PSM ASSERT ... CONS_PEND_PREWDS`, `PHYTX error` with the frame the MAC had shorter than the PSDU it promised (`TDC_Frmlen G#CD4`, `TDC_Psdulen G#D5A`), `TXE shared error ... aqmerrstatus G#80`, a MAC reinit after each, and finally a THREADX data abort, after which no ioctl completes. On 2026-11-14 it did so at light load too (9 reinits over 2600 s before the abort that dropped Photon's alias join), and under load at once. The reproducer: 64 pings of 1400 bytes to the Pro's IPv6 address, each every 200 ms (320 a second; the device replies to each, so it transmits as much).
  - **Frames as 802.3 + LLC/SNAP.** bcmdhd for the 4398 builds with HOST_SFH_LLC (Kbuild) and, when the firmware's `cap` names `host_sfhllc` (this one does, as does the 8a's BCM4383), `dhd_prot_txdata` rewrites every frame into 802.3 + LLC/SNAP in host memory (`dhd_ether_to_8023_hdr`) and posts the whole of it with flag G#10 (BCMPCIE_TXPOST_FLAGS_HOST_SFH_LLC), the Ethernet header still in the item. pcie1 posted Ethernet II after the header, so the firmware inserted the LLC itself; the 4398's firmware fetches frames from host memory for its MAC ("alfrag": `txq_process_alfrag`, `tx_dma_alfrag`, strings the 4383's firmware has none of), and that path broke. Measured: the original died in about 16 s at 320 a second, twice (64 a second, the earlier agent's rate, took 40 to 250 s); with Ethernet frames and the console streamed, PSM asserts came at the firmware's 14 s and 29 s with 158 frames sent, then the PHYTX errors and the abort at 97 s; with 802.3 + LLC/SNAP, none in 57088 frames.
  - **Request ids under G#FF00.** pcie1 named a frame G#2000 + its number; dhd keeps transmit ids under DHD_MAX_PKTID_16BITS (PKTID_MAX_MAP_SZ_TXFLOWRING, G#FF00; the ids above it are its reserved ones, G#FFFE an ioctl). The firmware took a data abort (dfar 0, G#FF00 on its stack) when id G#FF00 came: completions stopped at exactly A#57088 frames. Now the id names the buffer (`TX_ID` + k), and the buffer is freed by the completion naming it: the chip completes out of order (12 of 225 with Ethernet frames, 21 to 134 a run of 130 to 270 thousand with 802.3), so counting completions could free a buffer still in flight.
  - **Result (the fixed pcie1 stored and sealed on the Pro):** 320 a second for 720 s: every watch ping answered (719 of 719, no gap), 229070 of 229585 load pings answered, 271389 frames posted and all completed, nothing in the firmware console. Two runs before it: 8 minutes clean, then the device rebooted (below); 9 minutes, then 9 s with no ioctl completed and a LINK down for beacon loss (event 16 reason 1, no assert, no trap), the keeper's restart serving again 33 s later, clean for the remaining 145 s.
  - Also in pcie1 since: the firmware's console said as it grows with the 32 s lines, the transmit totals (`tx: posted, completed, out of order, not sent`) with them, and a trapped firmware (shared flags G#600) ends the driver as a silent one does: with the screen off no ioctl is asked, and the original served on with the firmware dead.
  - **Trap for a flood test:** ip logs every echo (`ping6 from`), and the kernel keeps undrained console text in a growing buffer; `diag "console kept"` does not drain it. At 320 a second for 8 minutes the kernel heap refused A#16374136 bytes and the Pro rebooted (boot 869 to 870, "previous boot's kernel heap refused"). Drain with `diag console` every few seconds during a flood. Fixed 2026-11-14: the console is a fixed 256 KiB ring that overwrites its oldest text and counts the bytes a drain missed per writer (KERNEL.md "The console"), and ip counts echoes in its 64 s WiFi path line instead of a line each (`FERROS_IP_PINGS=1` builds say each one).
- **The chip's interrupt (`FERROS_WIFI_IRQ=1` builds of pcie1 and ip, 2026-11-11; the default since 2026-11-13, below):**
  - Proven from source: channel 1's MSIs reach the GIC on SPI 439 (INTID 471, `IRQ_PCIE_GEN3A_1_HSI2`, level high; zuma-pcie.dtsi pcie_1 has no `separated-msi`), thru ELBI PCIE_IRQ2 bit 17 (IRQ_MSI_RISING_ASSERT), which PCIE_IRQ2_EN bit 17 enables (pcie-exynos-rc.c `exynos_pcie_rc_irq_handler`, `exynos_pcie_rc_msi_init`). The kernel grants the line (`pcie1`) to whoever holds `pcie1-elbi`, as the slot after its last (slot 19 for init's pcie1; `bridge spawn` gives it with `--irq pcie1` there).
  - Not proven without the device: that the BCM4398 sends an MSI for its completions once its mailbox mask is set. The driver uses brcmfmac's register set for PCIe cores of rev 64 and up (this core is rev 130): MAILBOXMASK at core G#C34 = G#FFFF (the D2H doorbells), MAILBOXINT at G#C30, write back to clear (BAR0 + G#2000 + offset). dhd's own layout for the 4398 may differ.
  - The driver: idle, it waits on the line (backstop 2^3 of ~1 ms) instead of sleeping 1 ms; on a wake by the line it clears ELBI G#8 (write back), DBI G#830 (write 1 to clear), the chip's MAILBOXINT, acks the line, then reads its rings. The IP stack: a second thread holds a `wlan` op 9 call ("tell me when frames wait"), answered once frames wait; the main thread's idle wait is on that doorbell, an application's call or its next timer (the cellular call's millisecond, a retransmit timer, a held receive's deadline, else 2^6 of ~1 ms).
  - What to read on the device: pcie1's `rx path, last 32 s` line ends `irq A#n wakes, A#n with new frames; new frames with no irq A#n` (the MSI works when the first two climb with traffic and the last stays near zero; if `irq: chip mailbox mask` at serve start reads G#0 or G#FFFFFFFF, the register is not where brcmfmac puts it); ip's `WiFi path` line ends with the doorbell's rings and how its idle waits ended. `bridge diag threads` should show pcie1 in `irq_wait on INTID A#471`.
- **No timers while serving (2026-11-13, the 8a; owner, 2026-11-10: "No polling! No timers unless PROVEN necessary!"):** the 8a dark drew 782-859 mW with the radio drivers waking on millisecond timers (deep-sleep measure: pcie1 ~900, ip ~260 wakes a second in the dark).
  - pcie1: the interrupt is the default (`FERROS_WIFI_IRQ=0` keeps the sleep). An interrupt thread waits on the line and rings the serving loop, leaving the line masked until the loop has read its rings (one interrupt an idle spell, as NAPI). The loop's wait ends on that, a call, or a preference changing; its one deadline is the corners' measures every 2 s while the screen is on (none in the dark: five ioctls every 2 s for nobody). An ioctl waits on the MSI with its 3 s timeout instead of spinning. A self-test at serve start (an ioctl's completion must raise the line within ~16 ms) falls back to the old sleep and says so. With no network to join and no scan, it waits for a call, the quarter-second look for the credential, or the retry.
  - ip: the doorbell is the default, the idle wait has no backstop (it ended every 64 ms whatever ran): it ends on a doorbell, a call, a preference, or the earliest protocol timer running (retransmits, held receives, the resolver, DHCPv4's 2 s while unbound, the router solicitation's 4 s, the membership reports' 60 s, the WiFi driver looked for every 4 s while there is none). The cellular call has a doorbell too (`rmnet` op 9 WATCH, payloads/modem), so neither RECV every idle millisecond nor INFO every second while a modem driver answers it.
  - Measured on the 8a (BCM4383): `irq: the chip's completion raised SPI 439` at serve start. Screen on: pcie1 from 64,000 to 99,000 passes per 32 s (32,000 to 48,000 of them sleeps) to 918 to 1,008 (270 to 300 waits; A#219 interrupts, A#166 with new frames, A#1 frame found with no interrupt); ip's waits by its own timer A#23 in 16 s. Ping from the dev box 5.0 ms on average before, 5.5 ms after; httptest's 178 KB LTO file 115 ms (round trip 32 ms) before, 127 ms (25.6 ms) after, one each.
  - The dark: the kernel switches the workers off (CPU_OFF), and the line had been routed to the core that made the wait, a worker: it went silent for whole 32 s windows (`irq A#0 wakes`, frames found only when ip called) and pings averaged 785 ms (1.86 s most), against 65 ms for the old loop and 61 ms with the workers kept on (`diag "smp dark off"`). Fixed in the kernel (killswitch.rs `enable_spi`: a granted line goes to the kernel's core); until a kernel with it is flashed the stored pcie1 stays the millisecond loop (the 8a's was put back).
  - **The Pro's interrupt went silent, and every measure took another ioctl's answer (2026-11-17, b913):** the owner's WiFi circle showed terabytes a second and 415357 dB.
    The console: `irq A#0 wakes ... armed A#0` in every 32 s line, A#458 `no ioctl completion` in one read, SPI 439 stuck at A#271060 unmasks across two `diag irqs` minutes apart (it had worked earlier in the boot).
    With no interrupt each ioctl waited its 3 s and ended without reading the ring again, so the completion that came meanwhile was taken by the next ioctl: `ioctl` returned the first completion in the ring whatever ioctl it answered, so RSSI, noise, chanspec, rate and the packet counters each got another's words (the SNR most likely a cumulative packet count less a noise floor, the rate another ioctl's word times 500 kbit/s).
    Fixed in pcie1: the answer is the completion carrying the ioctl's own trans_id (+14 of ioctl_comp_resp_msg_t, as dhd checks it), a late one is dropped and counted on the console, and the ring is read once more at the deadline.
    The id is wrapped at 16 bits: it shares a word with input_buf_len, and the 65,536th ioctl would have ORed its bit 16 into the length.
    Why the line went silent, measured by the `irq silent:` line pcie1 now says (b917, the new kernel, 21 minutes in): ELBI G#32180C00 (bit 17, the MSI's edge, clear), the MSI status (DBI G#830) G#1, the chip's mailbox status 0 with its mask G#FFFF, the line armed.
    ELBI latches only the rising edge of the MSI status, so a status still 1 after the edge was cleared never raises the line again: `irq_clear` read the status once and wrote it back once, and an MSI landing on that write left it set (the chip's mailbox theory was wrong: it read 0).
    With the screen off the loop has no deadline and ip waits on the doorbell, so nothing made a pass: A#2 passes in 42 s, one ip call A#21 s, until the access point dropped the link (event 16) and the keeper restarted the driver.
    Now `irq_clear` clears the MSI status until it reads 0; not yet run.
  - What still wakes ip at rest is its clients: Photon's sockets called it ~1,000 times a second and fgtw ~150 (`diag sched`: Photon.7 → ip), each asking with nothing to wait for; held receives (op 2's wait byte) would end that.
- **UDP ports and multicast (2026-10-26):** datagrams are kept only for ports a process has opened (a RECV on it or a SEND from it, the last eight), the oldest giving way when the 8-slot queue is full — before, eight stray LAN broadcasts to unread ports filled the queue for good. IPv4 multicast (224/4) and IPv6 link-scope multicast (ff02::) are delivered: `payloads/fgtw` hears the stock app's `pt_disc` LAN beacons (239.104.199.144 / ff02::68c7:9014, port 4384), parsed by photon_core on the device.
- **Names while locked, and each family's DNS server (2026-11-12):** Mahere on the Pro overnight with nobody attested, and on the 8a spawned at the attest screen, logged "DNS gave no address" for every lookup. Measured on the 8a: the clients' questions never left. `ip` answers an application nothing while nobody is present (AUTH.md "Radios while locked"), so a SEND sent nothing, the held RECV came back empty at once, and every client read that as DNS finding no address. With the person check lifted in a test build (never stored), the same httptest resolved over WiFi and fetched (AAAA from the router's server, a 1.6 MB cell, 200 OK). Changes:
  - INFO's flags bit 3 says whether a person is present; httptest and Mahere ask it first and say "nobody present" instead of sending questions nobody will carry.
  - INFO's IPv6 DNS server is the one of the link IPv6 leaves on: WiFi's from the router advertisement's RDNSS (RFC 8106, now read; the advertisement believed only from a link-local source at hop limit 255, RFC 4861), the call's only when IPv6 leaves on the call. Before, it was always the call's, asked from WiFi's address (a carrier's server need not answer an address outside its network; not measured, the 8a has no call), and a network naming its server only by RDNSS had none. The IPv4 server already followed its link; a family with no link now reports zeros, and both of WiFi's are forgotten when WiFi goes.
  - The kist stower's DNS port is its own (53056): it shared fgtw's 53054, and ip hands a port's datagrams to whoever asks first.
  - httptest, the stower, fgtw and Mahere take an answer only from the server asked, from port 53; httptest's answers come by a held receive, its 2 ms polls gone.
- **One resolver for the device (2026-11-12):** the four copies (httptest, the kist stower, fgtw, Mahere's names.rs), each its own port, parser, retries and cache and each a little different, gave way to `ip` op 10 RESOLVE (the op and its statuses in payloads/ip's header; its pure parts in `ferros_dns`, tested on the host).
  - The request names a family (0 either: AAAA where IPv6 has a route, then A; or 4, or 6), a wait and the host; the answer is a status, the family, the address and the TTL left, held until the lookup ends.
  - One cache (32 records, a name and a type each, searched in order): an address for its TTL, then the last good one (answered at once while it is asked for again, kept another TTL when that lookup brings no answer); no such address for the SOA's negative TTL, at most 256 s (RFC 2308). A TCP connection that opens clears its address's failures; four failures in a row send its name to the server again (Mahere's rules, now every app's).
  - One lookup a name and type however many ask; questions from one port random for the session, a random id from the entropy grant each, to each family's server in turn, four tries a second apart; an answer counts only from port 53 of a server asked, with the id and the question repeated, every length and offset checked (compression pointers only back, CNAME chains followed to eight).
  - The stower and fgtw ask for IPv4 (their connections are IPv4, as before); httptest and Mahere for either.
  - On the 8a over WiFi (the person check lifted in a test ip spawned by hand, never stored; the vault's ip restarted after): gllto2.glpals.com AAAA from the router's server in 52 ms on the first try, the 181623-byte LTO file fetched, 200 OK, the same fnv64 as before; Mahere's bucket host AAAA in 56 ms; example.com, IPv4 only, A in 38 ms, then 0 ms from the cache with no question sent; a name under .invalid "no such name" (AAAA 54 ms, then A 3 ms), asked again from the negative cache with no question sent.
- **Speed (2026-11-14, the 8a; owner: "the map seems to be loading quite slow. FPS is good, cells come in quite a bit slower from cloudflare than on android. have we speed tested the cellular and wifi?"):** measured with httptest (now `host:port`, IPv4 literals, `@` to start several together, the time to the first byte, 256 KiB receives) on the bucket Mahere uses, brobdingnagian.holdmyoscilloscope.com over plain HTTP, its largest file the 17775924-byte font pack `fonts/noto-sans-sc.vsf` (served from the origin, `cf-cache-status: DYNAMIC`).
  - One TCP stream: A#1524 to A#1697 ms, A#10.0 to A#11.1 MiB/s on average, A#14.9 at peak, round trip A#30 ms, nothing resent (one run of A#3749 ms, A#4.5 MiB/s, just after pcie1 was restarted). Four at once: A#71.1 MB in A#4.9 s, A#13.8 MiB/s together, A#3.4 to A#3.9 each, nothing resent.
  - The dev box on the same WiFi, same file: one stream 10.7 to 21 MB/s, four together 13.4 MB/s. The device matches it: the access point and the WAN bound the rate (about 14 MB/s with several streams), not the 8a.
  - What bounds the device once the WAN is faster: pcie1 copies each received frame out of its non-cacheable DMA buffers at A#53 to A#55 µs a 1440-byte frame (its `rx path` line), about 27 MB/s for a whole core; at 14 MB/s that is half of one. Cacheable receive buffers with an invalidate before the copy would lift it (not done).
  - Under four streams pcie1 refused A#6918 of ip's A#31206 frames out (A#3924 of A#31257 in an earlier run; A#0 to A#154 with one stream), nearly all ACKs: nothing was resent and the rate matched the dev box, but a sender loses ACKs it paces by. Next to look at: the transmit buffers, held until the chip completes them while the receive side is busy.
  - Not measured: the LAN alone (device to the dev box). The dev box's firewall drops inbound TCP from the LAN as it drops UDP (devbox_network_for_phone_tests): the SYN to a server on its WiFi address went unanswered, eight tries in 47 s. Opening a port needs the owner (root).
  - Mahere's pattern, before (the owner's session, 285 cells and 130 checks): one request a connection (`Connection: close`), four connections, twenty loaders queued for them; a name lookup (ip's cache, 0 ms) and a TCP handshake a fetch; a 2284-byte cell 811 ms in Mahere, its connection open 263 ms, the same cell 78 ms by httptest; in the busy spells A#7.4 fetches a second, A#1.04 MB/s.
  - After (Mahere branch ferros-netspeed, plain_http's pool): eight connections, each kept 16 s for the next request (the ip server frees one its app leaves unasked for 32 s); each fetch line says its wait, dial or kept connection, first byte and body. A cache check (304) A#139 ms on a new connection, A#54 on a kept one; zooming in on land, A#98 cells (A#30.6 MB, cells of 400 KB on average) at A#9.7 fetches and A#3.0 MB/s in the busy spells, the eight connections busy at once, no failures.
  - What bounds a fresh cell now is the bucket: its first byte took A#300 to A#560 ms on a kept connection (Cloudflare fetching it from the origin: the same cells from the dev box seconds later were `HIT`, first byte 60 to 137 ms). Android's build keeps a connection for each of its twenty loaders; ferros's eight are half of ip's sixteen (`TCPN`), so more in flight needs ip's table grown first.
  - At start the 8a checks about 140 cached cells: A#9.8 s before, A#11.3 s after, though each check took less than half the time and no fetch waited for a connection: fewer than one check in flight on average, and the loaders' time summed A#1.47 s a base cell against A#54 ms for its check. The start looks bound by decoding (mahere-engine's two decodes at once, half of the four cores ferros's std reports), not the network; not yet measured directly.
- **Cellular speed: the plan for the Pro (not yet run; the owner's device, run only with his go, cellular switched on by him).** The 8a has no SIM. The same tests over LTE/NR, the WiFi numbers above as the reference:
  - Before: the Pro on USB (`FERROS_PORT` of its port), attested (ip serves apps only while someone is present), cellular on, the modem registered and its data call up (the modem's console: registration, the call's addresses and MTU). Note the RAT (LTE or NR), the band and the signal from the modem's lines at each run. Data used: a 17.8 MB pack a stream, so the plan below is about 200 MB; a smaller file (a 2 MB base cell) where the owner's data is short.
  - Round trip: httptest of a small cell (`!brobdingnagian.holdmyoscilloscope.com/mahere/cells/base/dTMFd202HEQAAAAAAAA.vsf?e=8`, `!` = ip's LINK 1, cellular only, put back after): connect and first byte times, the close report's round trip. Three runs, IPv6 (the default) and IPv4 (`!~`).
  - One stream: `!brobdingnagian.holdmyoscilloscope.com/fonts/noto-sans-sc.vsf`, three runs, then `!~` for IPv4. Read each close report: average and peak MiB/s, round trip and its variation, resent (fast retransmits, expiries), out of order, the MSS (from the call's MTU), the most waiting for the app.
  - Four together: `/mnt/Harbor/tmp/netspeed/par.sh`-style spawns of `!@` plus the spec (each named httptest0 to 3), so they start on the same 8 s boundary; the sum of their bytes over the longest.
  - Mahere over cellular: ip's LINK 1 (an epcall of `09 01` on `udp`), Mahere relaunched, the same zoom-in (`bridge input pinch` at one point, 20 s apart), its `the network, last` summaries and fetch lines; then LINK 0 (`09 00`).
  - What would bound it, to read off: the round trip (a cell's first byte), resends (loss on the radio), the modem driver's own path (its per-frame cost against pcie1's), and ip's single thread with WiFi idle. Compare with Android on the same spot and time if the owner wants the reference.
  - After: LINK 0, cellular back to how the owner keeps it (switched off), nothing stored.
Then: msgbuf rings, CLM blob, 802.11 association, an IP stack — each as processes.

**Recipe source:** The bring-up is written as a driver *process* holding hardware grants (`payloads/pcie1`, ferros_kernel process.rs `--hw`), not as kernel code. Goal 1: PCIe link up (Gen1 or better). Goal 2: the endpoint's vendor/device ID over config space. Goal 3: dhd's first BAR0 chip-id read. Firmware download, msgbuf rings, 802.11 and an IP stack come after.

Numbers use the project's `G#` notation. **[AMBIGUOUS]** / **[INFERENCE]** mark what the sources don't settle; the open questions are collected in §13.

Source tags (all under `/mnt/Harbor/husky-kernel/`):

| Tag | Path |
|---|---|
| **RC** | `private/google-modules/soc/gs/drivers/pci/controller/dwc/pcie-exynos-rc.c` |
| **RCH** | `.../dwc/pcie-exynos-rc.h` |
| **CMNH** | `.../dwc/pcie-exynos-common.h` |
| **CAL** | `.../dwc/pcie-exynos-zuma-rc-cal.c` |
| **DWH** | `aosp/drivers/pci/controller/dwc/pcie-designware-host.c` |
| **DW** | `aosp/drivers/pci/controller/dwc/pcie-designware.c` |
| **DWHDR** | `aosp/drivers/pci/controller/dwc/pcie-designware.h` |
| **ZDT** | `private/devices/google/zuma/dts/` |
| **SFR** | `private/google-modules/soc/gs/drivers/soc/google/cal-if/zuma/cmucal-sfr.c` |
| **DHD** | `private/google-modules/wlan/bcm4398/` |

## 0. Address map (ch1 = HSI2 "GEN3A_1", not HSI1)

Source: ZDT/zuma-pcie.dtsi:75-122, ZDT/zuma-board-pcie.dtsi:31-43.

| Block | Base | Size | Notes |
|---|---|---|---|
| ELBI | G#1312_0000 | G#2000 | app/ctrl regs |
| SOC | G#1313_0000 | G#5000 | soc_ctrl; clock select at +G#4000 / +G#4004 |
| PCS | G#1314_0000 | G#5000 | |
| UDBG | G#1315_0000 | G#D000 | external PLL at +G#C700 |
| PHY (PMA) | G#1316_0000 | G#3000 | |
| SYSREG_HSI2 | G#1302_0000 | G#2000 | not written by the ch1 zuma path |
| DBI | G#1340_0000 | G#38_1000 | iATU (unrolled) at DBI + G#30_0000 = **G#1370_0000** |
| CONFIG window | **G#60FF_E000** | G#2000 | the `config` reg (zuma-pcie.dtsi:85) |
| PCIe MEM | G#6000_0000 | G#FF_0000 | identity mapped (zuma-pcie.dtsi:104) |
| PMU | G#1546_0000 | | ch1 PHY control **G#1546_3EC8** (`pmu-offset`); ch2 +G#3ECC. **EL3 SMC only** |
| CMU_HSI2 | G#1300_0000 | G#8000 | SFR:37 |
| CMU_TOP | G#2604_0000 | G#8000 | SFR:11 |
| GPIO_HSI2 (gph3) | G#1304_0000 | | gph3 at offset 0 (`pinctrl-gs.c:283`) |
| GPIO_FAR_ALIVE (gpa5) | G#154E_0000 | | gpa5 at offset 0 (`pinctrl-gs.c:246`) |

Board DT for ch1: `num-lanes=1`, `max-link-speed=3`, `ep-device-type=EP_BCM_WIFI (1)`, `perst-delay-us=15000`, `use-msi="true"`, `use-l1ss="true"`, `pcie,wlan-gpio=<gph3 4>` (zuma-board-pcie.dtsi:31-43). `use-sysmmu = "disabled"` is an invalid string the driver treats as false (RC:2202-2215). `pcie-clk-num = phy-clk-num = 0` (zuma-pcie.dtsi:108-109): Linux never touches a CMU clock for PCIe.

## 1. Power domain

The `pcie_1` node has no `power-domains`. zuma-pm-domains.dtsi defines only `pd_hsi0` (G#1546_2A80) and `pd_hsi1` (G#1546_2B00, disabled); there is no pd_hsi2 (ZDT/zuma-pm-domains.dtsi:209-229). `pm_runtime_get_sync()` (RC:5658-5659) is a power no-op: **no ACPM/PMU power-domain call for ch1**. HSI2 also hosts UFS, which ferros already drives, so it is up.

## 2. Clocks (CMU)

Linux relies on the bootloader state plus hardware auto clock gating: the GEN3A_1 Q-channels are `VCLK_GATE` without `VCLK_QCH_DIS` (`clk-zuma.c:465+`), and `cal_vclk_enable`/`disable` return immediately for them (`clk/gs/composite.c:1247, 1282`). **Read these first, force them on only if gated.**

Gate register bits (SFR:8745-8936): bit 20 MANUAL, bit 21 CG_VAL, bit 28 ENABLE_AUTOMATIC_CLKGATING. "Clock on" = MANUAL=1 + CG_VAL=1, or MANUAL=0 + bit 28 = 0 (`cal-if/ra.c:412-433`). Q-channel bits (SFR:14149-14169): bit 0 ENABLE, bit 1 CLOCK_REQ, bit 2 IGNORE_FORCE_PM_EN.

| Register | Address | Purpose |
|---|---|---|
| GAT PHY_REFCLK_IN (GEN3A_1) | G#1300_201C | parent `MUX_CLKCMU_HSI2_PCIE_USER` |
| GAT DBI_ACLK / MSTR_ACLK / SLV_ACLK | G#1300_2088 / 208C / 2090 | |
| GAT SUB_CTRL APB / PHY_UDBG APB / PIPE_PAL APB | G#1300_2094 / 2098 / 209C | |
| GAT SF_PCIEPHY QCH_TM APB | G#1300_2020 | |
| GAT UASC_PCIE_GEN3A_DBI_1 ACLK/PCLK | G#1300_20EC / 20F0 | NOC→DBI path |
| GAT UASC_PCIE_GEN3A_SLV_1 ACLK/PCLK | G#1300_20F4 / 20F8 | NOC→config/MEM windows |
| GAT QE_PCIE_GEN3A ACLK/PCLK | G#1300_20BC / 20C0 | |
| QCH APB / AXI / DBG / PCS_APB / PMA_APB / UDBG | G#1300_3058 / 3060 / 3068 / 306C / 3070 / 3074 | |
| DMYQCH REF | G#1300_3000 | |
| QCH UASC DBI_1 / SLV_1, QE_PCIE_GEN3A | G#1300_30B4 / 30B8 / 3088 | |
| PLL_CON0_MUX_CLKCMU_HSI2_PCIE_USER | G#1300_0620 | MUX_SEL bit 4: 0 OSCCLK_HSI2, 1 CLKCMU_HSI2_PCIE (SFR:4834; cmucal-node.c:1161-1164). Preserve |
| CMU_TOP MUX / DIV / GATE CLKCMU_HSI2_PCIE | G#2604_10B4 / 18AC / 20DC | parents OSCCLK_CMU / PLL_SHARED2_D2 (cmucal-node.c:385-388). Preserve |

**[INFERENCE]** The PHY refclk comes from the UDBG "External PLL" fed from 38.4 MHz (CAL:333-338, 382-385), not this CMU path; the PHY_REFCLK_IN gate still feeds the sub-controller. **[AMBIGUOUS]** reset/ABL values of these gates — read and log them before the first DBI access; a gated block can hang the bus.

## 3. GPIO / pinmux

gph3 = `bank_type_6` (`pinctrl-gs.c:59-62`): CON +0 (4 bits/pin), DAT +4, PUD +8 (4 bits), DRV +C (4 bits), CONPDN +10 (2 bits), PUDPDN +14 (4 bits). gpa5 = `bank_type_7` (`pinctrl-gs.c:67-70`). PULL_NONE 0, PULL_DOWN 1, PULL_UP 3, PDN_PREV 3; DRV_X1 0, DRV_X4 3.

| Pin | Use | CON G#1304_0000 | PUD G#1304_0008 | DRV G#1304_000C | Source |
|---|---|---|---|---|---|
| gph3-0 | PERST# (output) | [3:0] = 1 | — | [3:0] = 3 | zuma-pinctrl.dtsi:941-946 |
| gph3-1 | CLKREQ# (func 2) | [7:4] = 2 | [7:4] = 3 | [7:4] = 3 | zuma-pinctrl.dtsi:932-939 |
| gph3-4 | WL_REG_ON (output) | [19:16] = 1 | [19:16] = 0 | [19:16] = 0 | zuma-board-wlan.dtsi:24-31 |
| gph3-5 | wl_dev_wake (input, pull-down) | [23:20] = 0 | [23:20] = 1 | 0 | zuma-board-wlan.dtsi:33-40 |
| gpa5-1 | wl_host_wake (EINT, pull-down) | G#154E_0000 [7:4] = G#F | G#154E_0008 [7:4] = 1 | 0 | zuma-board-wlan.dtsi:12-21 |

PERST and WL_REG_ON are driven raw (the DT active-low flag is ignored): 1 = high = PERST# deasserted / WL_REG_ON on (RC:3584, 3694; DHD/dhd_custom_google.c:579, 623). PERST = DAT G#1304_0004 bit 0; WL_REG_ON = DAT G#1304_0004 bit 4.

## 4. Linux order of operations

1. RC probe (RC:5572-5805): WL_REG_ON low (RC:2317-2323), PERST output-low (RC:2353-2358), `resumed_phydown` (RC:5714 → 3133-3158: IRQs off, PMU isolation release, PHY power-down, ch2 PHY off), DBI RC setup with the PHY down.
2. dhd init: WL_REG_ON high, then `msleep(WIFI_TURNON_DELAY = 200 ms)` (DHD/dhd_custom_google.c:563-588; DHD/dhd.h:3472-3474).
3. dhd enumerate: `set_carddetect(1)` → `exynos_pcie_pm_resume(1)` → `exynos_pcie_rc_poweron(1)` (DHD/dhd_linux_platdev.c:731 → dhd_custom_google.c:678 → RC:4144-4155 → RC:3809).
4. poweron: pinctrl active (RC:3876), soc+4000 = G#15 (RC:3879), PHY power-down clear (RC:3882), `establish_link` (RC:3896 → 3551).
5. PCI rescan → dhd probe → chip-id read (§11).

Net timing: WL_REG_ON↑ → ≥200 ms → PHY bring-up → PERST↑ → **15–17 ms** (RC:3589-3590) → LTSSM enable → L0 within ~48–58 ms. If you toggle WL_REG_ON, hold it low ≥20 ms (DHD/wb_regon_coordinator.h:89).

## 5. Step A: ch1 PHY into a known state

| # | Action | Address | Value | Source |
|---|---|---|---|---|
| A1 | ELBI IRQ0/1/2_EN | G#1312_0010 / 0014 / 0018 | 0 | RC:2552-2554 |
| A2 | **PMU PHY isolation release (EL3 SMC)** | G#1546_3EC8 | RMW mask G#1, val G#1 (bit 0 = `PCIE_PHY_BYPASS`) | RC:393-407, CMNH:52-53, RCH:121-122 |
| A3 | phy+204 &= ~G#C | G#1316_0204 | RMW | CAL:50-51 |
| A4 | phy+1044/1048/104C/1050/185C | | G#2A, G#AA, G#A8, G#80, G#0A; 1 µs | CAL:57-62 |
| A5 | phy+208 | G#1316_0208 | G#FF; 1 µs | CAL:64-65 |
| A6 | phy+580, phy+928 | | G#0A, G#AA | CAL:67-68 |
| A7 | phy+00C | G#1316_000C | G#0A; 50 µs | CAL:71-73 |
| A8 | UDBG C700 &= ~bit 0 | G#1315_C700 | RMW; 10 µs | CAL:75-77 |

SMC ABI (`exynos-pd_el3.c:66-77`; `exynos-smc.h:45`; `exynos-el3_mon.h:21-23`): x0 = G#8200_0504, x1 = address, x2 = 2 (RMW), x3 = mask, x4 = value; x0 = 0 on success.

## 6. Step B: WL_REG_ON and pinmux

B1 WL_REG_ON low, PERST low (RC:2322, 2357) → B2 WL_REG_ON high (DAT bit 4) → B3 **200 ms** → B4 §3 pinmux → B5 SOC+4000 = **G#15** (G#1313_4000, RC:3879).

## 7. Step C: PHY power-down clear (`phy_all_pwrdn_clear`, ch1)

C1 G#1315_C700 |= bit 0, 100 µs (CAL:164-166) → C2 G#1316_000C = 0 (CAL:167) → C3 G#1316_0928 = G#55, G#1316_0580 = G#02 (CAL:169-170) → C4 G#1316_0208 = 0 (CAL:172) → C5 G#1316_1044/1048/104C/1050/185C = 0, 10 µs (CAL:174-183).

## 8. Step D: zuma PHY configuration, ch1 (CAL:291-541, from `assert_phy_reset`, RC:3071-3086, 3565)

`customized-de-emphasis` absent (CAL:303-318 skipped); one lane (CAL:415-416).

| # | Address | Value / op | Delay / poll | CAL |
|---|---|---|---|---|
| D1 | G#1316_032C | G#3 | | 320 |
| D2 | G#1315_C700 | \|= bit 0 | | 322-323 |
| D3 | G#1315_C700 | &= ~bit 1 | | 325-326 |
| D4 | poll G#1315_C734 bit 2 (EXT_PLL_LOCK) | | 10 µs × ≤10000 (Linux only logs) | 329-330, 244-274 |
| D5 | G#1313_4004 | &= ~bit 2 (external PLL) | | 334-335 |
| D6 | G#1313_4004 | \|= G#30 | | 337-338 |
| D7 | G#1312_0080 | G#4 (RC) | | 341 |
| D8 | G#1312_03A4 | G#F, then G#D | 10 µs | 344-346 |
| D9 | G#1312_03A4 | G#F | 10 µs | 347-348 |
| D10 | G#1312_1404, 1408, 1400 | 1,1,1 then 0,0,0 | | 351-356 |
| D11 | G#1312_03D8 | 1 | | 359 |
| D12 | PHY +0000=G#88, +001C=G#66, +01F4=0, +0514=G#59, +051C=G#11, +062C=G#0E, +0644=G#22, +0688=G#03, +06D4=G#28, +0788/078C=G#64, +0790/0794=G#50, +0944/0948/094C/0950=G#05 | | | 364-380 |
| D13 | +0590=G#02, +07F8=G#B0, +0730=G#08 | | | 383-385 |
| D14 | +0344=G#C0 | | | 388 |
| D15 | +0040=G#04, +0204=G#03, +02D4=G#1F, +0358=G#10 | | | 391-394 |
| D16 | +0018=G#01 | | | 397 |
| D17 | +0514=G#5B, +0608=G#0C, +060C/0610/0614/0618=G#0F, +0510=G#80, +0688=G#03, +0644=G#23, +0624=G#11 | | | 400-409 |
| D18 | +0630=G#0F, +06D0=G#53 | | | 412-413 |
| D19 | lane: +1140/1144/1148=G#04, +114C=G#02, +1150..115C=0, +12CC=G#1C, +12DC=G#6C, +130C=G#29, +13B4=G#2F, +1A64/1A68/1A84/1A88=G#05, +1A98/1A9C=0, +1AA8=G#07, +1AB8/1ABC=0, +1AF8=G#90, +1B34/1BB0/1BB4=G#03, +1BC0/1BC4=G#06, +1BE8=G#01, +1BF8=G#04, +1C98=0, +1CA4=G#81 | | | 418-448 |
| D20 | BCM: +1B20=G#02, +1340=G#23, +002C=G#3C | | | 451-455 |
| D21 | +1350 \|= 1 | | | 465-467 |
| D22 | PCS +190=G#100B_0604, +154=G#0007_00D5, +100=G#1640_0000, +104=G#0860_0000, +114=G#1850_0000, +124=G#6070_0000, +174=G#7, +178=G#100 | | | 470-480 |
| D23 | BCM: PCS +17C=G#700 | | | 482-483 |
| D24 | BCM: PCS +110=G#0160_0202 | | | 510-511 |
| D25 | G#1312_1404 = 1 | | 10 µs | 515-516 |
| D26 | G#1312_1408 = 1, G#1312_1400 = 1 | | | 517-518 |
| D27 | poll G#1316_0A80 bit 0 (PLL lock) | | ≤100 ms, logged | 521-522 |
| D28 | poll G#1316_15C0 bit 4 (CDR lock) | | | 524-525 |
| D29 | poll G#1316_140C bit 7 (offset cal done) | | | 530-531 |
| D30 | G#1316_032C = 0 | | | 537 |
| D31 | G#1312_1078 = 1 | | | 540 |

## 9. Step E: RC setup, LTSSM, link-up (`establish_link`, RC:3551-3807)

| # | Address | Value | Source |
|---|---|---|---|
| E1 | PMU G#1546_3EC8 (EL3) | RMW mask G#400 val G#400 (bit 10, undocumented) | RC:3567-3568 |
| E2 | G#1315_C800 | G#421 (ch1) | RC:3570-3576 |
| E3 | G#1316_032C | 0 | RC:3578 |
| E4 | DBI G#1340_0890 | G#1_2000 (EQ off) | RC:3581; DWHDR:100-103 |
| E5 | PERST# high (G#1304_0004 bit 0) | | RC:3584 |
| E6 | wait 15–17 ms | | RC:3589-3590 |
| E7 | G#1312_03BC \|= G#11 | | RC:3592-3595 |
| E8 | G#1312_03A0 = G#2 | | RC:3596 |
| E9 | G#1312_0074 = 1 | | RC:3597 |
| E10 | G#1312_03A8 &= ~G#FFF | | RC:3600-3607 |
| E11 | G#1312_0474 = 1 | | RC:3610 |
| E12 | G#1312_1078 = 1 | | RC:3615 |
| E13 | `dw_pcie_setup_rc` (below) | | RC:3620 |
| E14 | `exynos_pcie_setup_rc` (below) | | RC:3621, 3160-3230 |
| E15 | G#1312_0C00 &= ~bit 2; 0C04 = 0C08 = G#FFFF_FFFF; 0C00 \|= 3 | | RC:3637-3647 |
| E17 | **LTSSM enable** G#1312_0054 = 1 | | RC:3669 |
| E18 | **poll** G#1312_02C8 & G#3F == **G#11 (L0)** | 10–12 µs × ≤4800 | RC:3670-3681 |

E13 (DBI offsets, DWH:712-787, DW:620-706): G#08BC |= 1; LNKCTL2 G#00A0 [3:0] = 3; LNKCAP G#007C [3:0] = 3; G#0710 clear bit 7, set bit 5, [21:16] = 1; G#080C [12:8] = 1; BAR0 G#0010 = 4, BAR1 G#0014 = 0; G#003C [15:8] = 1; G#0018 = (old & G#FF00_0000) | G#FF_0100; G#0004 low 16 = G#107; G#0010 = 0; G#000A (16-bit) = G#0604; G#080C |= bit 17; G#08BC &= ~1. (The DWC iATU setup is skipped; exynos programs it, §10.)

E14 (RC:3160-3230): G#08BC = 1 (stays); 16-bit G#0000 = G#144D, G#0002 = G#ECED; LNKCAP clear [17:15],[3:0], set G#3_8000 | 3; G#0B40 = G#1A; G#0B44 = G#EA; G#0044 = 0; LNKCTL2 [3:0] = 3; DEVCTL2 G#0098 [3:0] = 5.

LTSSM (CMNH:71-105): DETECT_QUIET 0 → DETECT_ACT 1 → POLL_ACTIVE 2 → POLL_CONFIG 4 → CFG G#7–G#B → CFG_IDLE G#C → **L0 G#11**; recovery G#D–G#10 during the speed change. On L0: wait ~3 ms, DBI G#1340_0080 speed [19:16] / width [25:20]. Linux retries up to 10 times unless Gen3 — **skip that check for Gen1-or-better**. **[INFERENCE]** Force Gen1: LNKCAP SLS = 1, LNKCTL2 TLS = 1, no G#080C bit 17. Then clear ELBI G#1312_0000/0004/0008 by write-back, program the iATU (§10), G#1312_0074 = 0.

## 10. Step F: iATU and config read of 01:00.0

Unrolled iATU at G#1370_0000 + region × G#200 (RCH:299-321).

Outbound 0, CFG0 (RC:1644-1659): +08 LOWER_BASE = G#60FF_E000, +0C = 0, +10 LIMIT = G#60FF_FFFF, +14 LOWER_TARGET = **G#0100_0000** (bus 1 << 24 | dev << 19 | fn << 16), +18 = 0, +00 CR1 = G#4 (CFG0), +04 CR2 = G#8000_0000.

Outbound 1, MEM (RC:1661-1681): +200 CR1 = 0, +208 = G#6000_0000, +20C = 0, +210 LIMIT = G#601F_FFFF (Linux: 2 MB; **[INFERENCE]** use G#60FE_FFFF to cover the window), +214 = G#6000_0000, +218 = 0, +204 CR2 = G#8000_0000.

Config read (RC:1798-1854): G#1312_0074 = 1; reprogram outbound 0 target; `readl(G#60FF_E000 + where)`; G#1312_0074 = 0. Expected: vendor **G#14E4**, device **G#4444** (`BCM4398_D11AX_ID`) or G#4398 (DHD/include/bcmdevs.h:29, 109; dhd_pcie.c:17741-17742). The RC's own DBI G#1340_0000 should read **G#ECED_144D** after E14.

## 11. Step G: BARs and dhd's first BAR0 read

Assign EP BARs inside G#6000_0000..G#60FE_FFFF: BAR0 (64-bit, cfg G#10/G#14; dhd maps 32 KB), BAR "1" (resource 2, cfg G#18/G#1C; TCM, ≥4 MB), set the RC type-1 memory base/limit at DBI G#0020, then EP cfg G#04 |= G#6 (DHD/dhd_pcie_linux.c:2030-2060). First BAR0 access (dhd_pcie.c:1391 → 1323 → 4437-4452): EP **config write G#80 = G#1800_0000** (`PCI_BAR0_WIN` = `SI_ENUM_BASE_DEFAULT`), then **read32(BAR0 + 0)** = chipcommon ChipID: chip = value & G#FFFF → **G#4398**, revision = [19:16].

## 12. S2MPU / SysMMU

SysMMU `sysmmu_hsi2` G#131C_0000 sits in front of PCIe ch1 only (port PCIe_CH1) and is disabled (DT `status = "disabled"`, `pcie_1` `use-sysmmu = "disabled"`); Linux never programs it. S2MPU `s2mpu_s0_hsi2` G#131F_0000 (v9, always-on) gates every HSI2 master (UFS, PCIe ch1). Register model, from pkvm-s2mpu kvm_s2mpu.h / hyp s2mpu.c: CTRL_PROT_EN_PER_VID_SET G#50 / CLR G#54 (cold_ufs used to write CLR = G#FF: protection off for all 8 VIDs), DECERR-on-deny G#70, contexts G#100/G#104 (context i → VID i, before any L1 write), L1 per VID per GiB at G#4000 + vid·G#200 + gb·8 (table address = PA >> 4) / +4 (attr: bit 0 table, bits 2:1 R/W for a 1 GiB block), 4 KiB tables of 4 bits per page (bit 2 R, bit 3 W, 128 KiB per GiB, aligned to their size), GiB 4..33 fixed no-access, invalidation G#1000 + sync G#102C/G#1030, faults G#2000 (bit per VID) and per VID PA/info (VID [26:24])/info2 (stream ID [23:0]).

**The fence (ferros_kernel/src/s2mpu.rs, 2026-10-15):** every VID may reach only the cold window G#8000_0000..G#8240_0000 (kernel buffers for UFS, the DMA pool for the WiFi driver); every other GiB is no-access. Verified: UFS link, vault open/verified and the devinfo write work, the firmware boots, no faults. **Measured (stage 10):** an ioctl whose request buffer sits outside the fence latches a VID 0 permission fault (FAULT_INFO G#004F_0002, stream 4) — **PCIe ch1 is VID 0, and so is UFS** (it keeps working with VIDs 1..7 denied). So the fence is: VID 0 may reach UFS's own DMA pages (`.bss.dma.ufs`, 8 KiB) and `pcie1-dma` (2 MiB), nothing else — not the kernel, not the USB/PT buffers, not the S2MPU table (kept outside `.bss.dma`); VIDs 1..7 nothing. Verified: vault open/verified, devinfo write, ioctls, and a probe read of the kernel image denied (PA G#8008_0000). Left shared: the WiFi chip can reach UFS's 8 KiB; separating them needs the PCIe-only SysMMU (sysmmu_hsi2) or the HSI2 SSMT (address unknown) to move stream 4 to another VID. The dongle's PCIe core also has host-memory protection windows (HMAP, pcie_core.h `hmapwindow*` G#580.., violations at G#600..), a second, device-side layer.

## 13. What silently breaks the link / open questions

1. PMU G#1546_3EC8 bit 0 left 0 keeps the PHY isolated; bit 10 (E1) is set before PERST, meaning undocumented.
2. External PLL not locked (G#1315_C734 bit 2): Linux continues after 100 ms; check it yourself.
3. SOC+4004 bits [2],[5:4] wrong → TCXO instead of the external PLL; SOC+4000 = G#15 undocumented.
4. UDBG G#1315_C800 = G#421 undocumented, ch1-specific.
5. PMA reset (ELBI G#1400/1404/1408) and soft_pwr_rst (ELBI G#3A4) ordering and 10 µs gaps.
6. Gated CMU gates/Q-channels on the DBI/SLV/UASC/QE path → bus hang or SError.
7. PERST timing: WL_REG_ON high ≥200 ms before PERST↑, then 15–17 ms before LTSSM.
8. Refclk: CLKREQ pulled up on function 2; **[INFERENCE]** the SoC PHY drives the EP refclk ("ERIO on", D16).
9. Linux's Gen3 insistence — skip.
10. Lock polls D27–D29: log, don't abort.
11. INT bus ≥200 MHz while linked (`pcie-pm-qos-int`) — likely not needed for link-up.
12. wl_dev_wake is an input; bcm4398 never drives it.
13. BAR placement vs the 2 MB MEM iATU limit (§10).
14. SYSREG_HSI2: the ch1 path writes nothing there.
15. **ferros-specific:** on husky, reading an unpowered block makes the next EL3 call hang (BOOT.md EL3 poison rule) — read the CMU gates (§2) before touching ELBI/DBI.
16. **The PHY powered down with its isolation released stalls ELBI and DBI** (2026-11-15): Android's suspended link leaves exactly that (PMU G#4C1, the stage 1 A3-A8 values, UDBG G#C700 bit 0 clear), and the first jumps' WiFi and modem drivers hung the bus on it. With the PHY isolated (a cold boot's G#C0, or after `off`) both answer, at their reset values. Every bring-up therefore begins with `off` (stage G#F0's power-down: Bluetooth off, PERST low, the PHY powered down and isolated, WL_REG_ON low, no ELBI or DBI), and the survey reads ELBI and DBI only when the PHY is isolated or powered. HV.md "The radios after a jump" has the whole state Android leaves.
17. **Bluetooth's off must drive its pins** (2026-11-16): with Bluetooth switched off (or never on this boot) BT_REG_ON (gpn8-0) is still an input, and it reads high (con 0, dat 1, pull-down on); the old `bt::power(false)` wrote only the data bits, so pcie1's "Bluetooth off before the REG_ON cycle" did nothing, and a cycle of a WiFi chip that had been running wedged it. "A BAR access that never completes" below.

## The keeper: the driver started again after a failed join (2026-10-29)

When a join fails, the driver exits (`join failed (see events)`). Until tonight nothing started it again before the next boot, so a device that missed its first join had no WiFi for the session. On 2026-10-29 the home access point refused every join for an evening: dozens of reboots and reloads each rejoined, and it throttles repeated joins.

Now the kernel's service loop keeps it. Once the shell is up, a `pcie1` that is gone is started again from the vault (`diag restart pcie1`'s path, a full fresh bring-up). The first restart comes 64 s after it went, and the wait doubles with each restart up to 1024 s, since a refused join only adds to a throttle. The wait goes back to 64 s once the driver has stayed up that long. `diag kill pcie1` stops the keeper for the session.

Meanwhile the IP process stays on cellular. Its `wlan` calls carry CALL_LIVE (KERNEL.md), so they fail at once while no driver runs, and while a restarted one is still bringing the chip up. It asks again every 4 s and takes WiFi back the moment the driver serves.

## The MAC: a random one, not the calibration file's placeholder (2026-10-31)

Ferros never set a MAC, so the chip joined as its calibration file's `macaddr=00:90:4c:50:10:01`, Broadcom's placeholder (the same on every unit; Android sets the board's own address at every start).

From the evening of 2026-10-29, pangolin refused that address: the key exchange never started, or association failed with status 2. Nothing showed as blocked in the router's settings. The credential matched throughout.

A random address joined at once: WPA2 keyed, DHCP, SLAAC, fgtw.org over IPv6 and IPv4, ping replies at TTL 63.

**Now one address per network (owner, 2026-10-31).** The driver sets `cur_etheraddr` while the radio is still down. The value is the first six bytes of BLAKE3, keyed by the device's ira-derived key (`wifi mac per network v1`, the Ira right as pcie1's slot 17, home slot 49), over the network's SSID, made locally administered unicast.
- It's the same on a network every time and different everywhere else, and can't be linked across networks without the ira.
- It's keyed on the SSID, not the access point's MAC: the SSID is known before the radio comes up, and one network answers from a different MAC on each band and each mesh node.
- Without the Ira right or an SSID, it falls back to a random address.

**Worn by the person present (2026-11-09, 2026-11-14).** Since 2026-11-09 the key is the kernel's person-bound ira key (`ira_person_key`, "wifi mac per network v2": `derive_key("ferros 2026-11-09 person ira", the identity's domain base ‖ derive_key("ferros process ira v1 wifi mac per network v2", ira))`), owner: "We mix the ira in with our identity. can't have same address devices on network". So the address is per person, per device, per network: the same on every join and restart of that network by that person on this device, and different for another person on this device or this person on another device. It exists only while the person's identity is unlocked.
- **The bug (measured 2026-11-14 on the 8a):** the driver used that key only for a credential from `wifi.cred`. The 8a's network was kept as `wifi.net` (joined at the attest screen, before anyone attested; the move into the person's on attest is still to build), so every start after attest still joined under a fresh random address: "cur_etheraddr set (random ...)" 4 s after "identity unlocked", with the person key held. The Pro did the same (.217, .227, .247, .248, .228 on restarts). Every restart took a new DHCP lease (a day each), and after dozens the router stopped answering the 8a (48 DISCOVERs, no OFFER).
- **Now** the person's address whenever someone is attested, whichever entry the credential came from.
- **While nobody is present** (AUTH.md rule 2: no stable identifier on the air while locked) the address is this boot's: BLAKE3 keyed by the kernel's per-power-session key (`ferros_sys::session_key`, SYS_IRA x4 bit 2: `derive_key("ferros 2026-11-14 process session key", derive_key("ferros 2026-11-14 process session root", wairua) ‖ the process's name)`) over `wifi mac this boot v1\0` and the SSID. The wairua is fresh each power session and never stored, so it is the same on every restart within a boot and unlinkable to any other boot. A kernel from before 2026-11-14 refuses that call (no domain is passed), and the driver falls back to a random address then, never to a lasting one. A kernel from before 2026-11-09 answers the person flag with the device's key alone; the driver takes an answer equal to `ira_key` as nobody present.
- **The log never prints an address:** `joining as MAC #525c` is the first two bytes of the address's BLAKE3 hash, enough to see that two joins wore the same one.
- **On the 8a, 2026-11-14** (its network kept as `wifi.net`, the owner's wife's identity attested):
  - On the running kernel, two restarts of the driver both set `#525c` ("worn by the person present"). The first joined, bound DHCP at once (A#192.168.1.160, 3 OFFERs, a day's lease) and got its IPv6 SLAAC address. The second's join was refused (WLC_E_ASSOC status 2: a killed driver leaves the access point holding the protected association of the same address, see below), and the retry 64 s later joined and was given the same lease, A#192.168.1.160.
  - After a RAM reload of the new kernel, locked: `#8f9c` ("this network's for this boot: nobody present"), joined; a restart while still locked set `#8f9c` again. After attest, a restart set `#525c` again (the person's address held across the boot), joined at once and was given the same lease, A#192.168.1.160 (1 OFFER), with an IPv6 SLAAC address.
- A restart is a kill, with no disassociation. With a stable address the access point refuses the next join until the retry (it still holds the old protected association; with random addresses that never showed). A clean stop that disassociates first would spare restarts that minute.

**The router throttles rejoins.** A join a few seconds after the previous session was cut off (a driver killed, so no disassociation) is refused. The keeper's retry 64 s later joined. That's probably what happened to the placeholder on 2026-10-29, after dozens of reload rejoins under one address.

## A BAR access that never completes: the cause and the guard (2026-11-16, the Pro, b908-b911)

The kernel-lock work found two cores stuck in register accesses after RAM reloads (b902, b904): core 4 reading G#6000_0040 (pcie1-mem), core 5 writing G#4006_0000 (pcie0-mem), interrupts masked, and minutes later the first big spawn hung in its broadcast cache and TLB maintenance with the kernel lock held. Reproduced, found and fixed here.

**Reproduced (b908: the forge kernel reloaded into the cold session b907, WiFi and cellular on, Bluetooth switched off by the radios preference, nobody attested).** Within a second of init starting the drivers, NOTE_1 and NOTE_3 read G#94 (inside SYS_MMIO_READ on the lock-free path): core 6 on G#6000_0040, core 4 on G#40FF_E000 (the modem's first config read of the CP). The path: pcie1's stage 6, which sizes the BCM4398's RAM through BAR0's window on the sysmem core (G#1808_8000): the coreinfo read at BAR0 + 0 answered, the bank index write at + G#10 went, the bank info read at + G#40 never completed (its console stops after `otp.cis`, stage 5's last line, and `sysmem.coreinfo` never prints). The modem was in `endpoint`, reading the CP's vendor id; in b904 it had got one step further, to the ROM boot's doorbell write. Every other core ran on, and nothing touched the kernel lock, so the device lived until something broadcast a TLB or cache maintenance.

**Measured on the stuck session, by hand (`bridge read`/`poke`, nothing the drivers do):**
- Both links in L0 by their PCS (G#1314_0188 and G#1212_0188 read 0), both PHYs powered (ch1 UDBG G#C700 = G#10141, ch0 = 0), PMU isolation released on both (G#401), PERST high on both, WL_REG_ON high. Not a link that is down, and not the PHY-powered-down state of section 13 item 16.
- The controller's completion timeout (DEVCTL2 range B, 28 to 44 ms in this controller) ended neither access in 130 s.
- **PERST taken low on WiFi's link alone (gph3 dat G#11 -> G#10) ended both accesses at once.** pcie1's read came back (all ones), it went on and ended ("BAR1 does not decode"); the modem's config read came back with the right answer (A5A5144D) and its whole bring-up then ran to INIT_END and a data call. So WiFi's endpoint held the root access, and the modem's access, on another controller in another domain, waited behind it somewhere the two share. Only a link-down ends such an access.
- The keeper's restart that followed, and a second one by hand, each ran `off` (PERST low, the PHY powered down and isolated, WL_REG_ON low 20 ms) and the cold path: the link trained to L0 and the chip's vendor id read 0 ("not Broadcom", as on b900). The chip was not resetting.
- Bluetooth's REG_ON (gpn8-0, G#1506_0100): con 0 (an input, its reset mode), pull-down on, and it read 1. Bluetooth had been switched off all session, so `bt::power(true)` had never made it an output, and `bt::power(false)`, which pcie1 calls before every WL_REG_ON cycle (the REG_ON rule), wrote only the data bit and pulsed DBO: it drove nothing. The rule's own failure mode (BT.md: a WL_REG_ON cycle under a powered Bluetooth core wedges the chip) was happening on every restart of a WiFi chip that had been running.
- gpn8 made an output, driven low and latched with a DBO pulse, then pcie1 restarted: vendor G#14E4, the sysmem read, the firmware, the join. The same session, the same chip, one change.

Why only after a reload: on a cold boot the WiFi chip is powered for the first time, WL_REG_ON going high from a long low (no cycle of a running chip); on a reload the chip was running the last session's firmware, and `off` cycles it with Bluetooth's core held on by the undriven pin. The keeper's restarts on a cold boot meet the same thing whenever Bluetooth is switched off, which may be what the 2026-10-31 resets were.

**The fix (built and run on the Pro):**
- `bt::power(false)` drives its pins: DEV_WAKE, REG_ON and DBO's data low, then their modes as outputs, then the DBO pulse (bt.rs).
- pcie1 checks the switch's answer before WL_REG_ON moves (`off` and stage 1): a refusal stops the bring-up with WL_REG_ON as it was.
- A reload or a jump crowbars the radios before init starts any driver (pcie_guard.rs `crowbar`, from service.rs): Bluetooth off as above, PERST low on both links (GPIO words only, never ELBI or DBI; the modem's only with HSI1 powered). The flash log says what it found: `radios crowbarred (a reload): Bluetooth's REG_ON found con G#0 dat G#1, now driven low; PERST low on ...`.
- The kernel's guard (pcie_guard.rs): every register access to a link's window (G#4000_0000 to G#4100_0000 for ch0, G#6000_0000 to G#6100_0000 for ch1: memory and config) notes its core, address and start, on the lock-free path and the locked one. The service loop's every pass and the workers' idle loop look for the oldest such access older than 2^-3 s (three times the completion timeout) and take that link's PERST low, the one thing measured to end it, then write `pcie guard: core N in an access to G#... for A#... ms with no answer: PERST of ... taken low` to the flash log. One release a look, the oldest first, since an access on the other link may only be waiting behind it.
- pcie1: a read of the chip's BAR or config space that comes back all ones looks at the link as Linux does (`exynos_pcie_rc_link_up`: the LTSSM, ELBI G#2C8, from Recovery.Lock G#D to L1.Idle G#14, read only while the PHY is powered); a link found down marks the driver dead, no BAR or config access follows, the stages stop and the serving loop ends, and the keeper starts it again.
- The modem: every doorbell goes through `ring_doorbell`, which rings only when the link is not suspended (`LINK_DOWN`) and the LTSSM is from Recovery.Lock to L1.Idle (Linux's s51xx_pcie_send_doorbell_int asks the same before every doorbell); refusals are counted in `diag modem state`'s lines.

**Measured with the fix (the Pro):**
- b909, b910, b911: three reloads of the fixed kernel, each with gpn8 put back to an input reading high first; b911's with WiFi joined and serving, the modem online with its link suspended (ch0 UDBG G#C700 = 2, PMU G#4C1). Each logged the crowbar (found con G#0 dat G#1), and pcie1 read the vendor id and the RAM size past stage 6; on b910 and b911 it downloaded the firmware with no bad words and joined, and the modem came up to its data call; no core stuck.
- The first guard build (b909) took a link down three times in a healthy firmware download: it read the counter once before its scan and computed `now - start` with wrapping, so an access that began after that read had a huge age. Fixed (the counter read per candidate, compared as `now > start + late`); b910 and b911 downloaded clean with no guard line.
- Bluetooth driven on by hand, then WL_REG_ON cycled under it while pcie1 served unjoined (b911): the chip went silent (the next scan's ioctl had no completion, its result all ones), but no access stalled. A restart then took Bluetooth off through the switch (gpn8 read as an output, low) and the chip came up.
- The guard's release itself has not fired on a real stall: with Bluetooth's off fixed, nothing here reproduces a wedged chip that holds a link in L0. Its mechanism (PERST low ends the stuck access, and the one behind it on the other link) is the hand measurement above.

**Inferred, not measured:** that an outstanding device access blocks the completion of a broadcast TLB or cache maintenance (the kernel-lock agent's b904 spawn hang); and where the two links' accesses share a path.

## A BAR access that never completes stalls a core, and after a RAM reload the next big spawn hangs the device (2026-11-16, kernlock)

Found while measuring the kernel lock (KERNEL.md "Order" item 7); for whoever takes the PCIe fix. Two RAM-reloaded sessions on the Pro (b902, kernel #3; b904, kernel #4) died the same way; cold boots of the flashed kernel (b901, b903, b905) did not.
- **What the post-mortem read (b905, after b904; the report page survives the cluster watchdog's warm reset).** Cores 4 and 5 stopped counting slices within b904's first minute (the heartbeat's C4/C5 at 138 and 167 from 64 s to 384 s, C6 to C8 counting on). Their last crumbs (PREV_NOTE_1, PREV_NOTE_2) are G#94 and G#95: entering a register read and a register write (SYS_MMIO_READ/WRITE on the lock-free path, process.rs `mmio_fast`) that never returned. Their last register notes (platform `note_mmio`, the previous boot's at G#FD60_01C8): core 4 a read of G#6000_0040 (`pcie1-mem`, the WiFi chip's BAR), core 5 a write to G#4006_0000 (`pcie0-mem`, the modem's BAR). An outstanding PCIe memory access whose completion never comes (the link down or not in L0, the endpoint not answering) holds the core in that load or store: interrupts are masked on the syscall path, so not even the killswitch or the stall catcher's SGI reaches it.
- **The hang that follows.** Minutes later the person attested and init spawned fgtw and the map; the spawn hung on core 7 with the kernel lock held and interrupts masked (stall catcher: core 7 silent 500 ms; PREV_EXIT G#0900_009A, SYS_SPAWN at `spawn_step` 9: the code copied, `build_from` not done), then the cluster watchdog (DSS G#CBEA). `build_from` does broadcast maintenance: cachemap `sync_code` (data and instruction cache maintenance to the point of unification) and `set_arc_wx` (`tlbi vae1is`, `dsb ish`). **Inference, not measured:** the broadcast's completion waits on the two stalled cores, so the first spawn after the stalls hangs. It fits both hangs, the step, and kernel #2's session (b900, never attested: no such spawn came; its pcie1 exited "not Broadcom" twice without stalling a core).
- **Why only after a reload, as far as read:** a RAM reload restarts the kernel and every driver while the PCIe controllers, the PHYs, the WiFi chip and the modem keep whatever state the previous session left (links up, in L1.2, or half torn down), where a cold boot finds them from power-on. The drivers then touch a BAR before (or without) the link answering. On b905 (cold, attested, minutes up) every core counts and no BAR access is outstanding.
- **What a fix needs (for the owner of pcie1 and the modem):** never touch a BAR unless the link is in L0 (the controller's LTSSM state read first, CAL/RC sections 9-11 above have the registers), and bring a link left by an earlier session down and up again on the reload path; possibly the kernel refusing `pcie*-mem` accesses while its controller's link is down. The kernel's other guard, `mmio_quiesce` at a process's end, waits for exactly such an access too and would hang the same way.
