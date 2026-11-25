# Bluetooth on husky

The BCM4398's Bluetooth core is on the APM's USI1 UART: serial_18, G#155D_0000, Android's `/dev/ttySAC18`. It speaks H4 with RTS/CTS.

## Split

- **Kernel** (`ferros_kernel/src/bt.rs`, power switch `bt`):
  - USI1's mode word: SYSREG_APM G#1542_04E4, 1 = UART.
  - The power lines, in nitrous.c's order:
    - BT_REG_ON gpn8-0;
    - DEV_WAKE gpn9-0, held high: no low-power protocol yet;
    - HOST_WAKE gpn6-0, an input;
    - BLE_DBO gpp24-3, a flip-flop clock pulsed at every REG_ON change.
  - It powers Bluetooth whatever WiFi's WL_REG_ON (gph3-4, gpio-hsi2) is. Until 2026-11-09 it refused while WL_REG_ON was low (Android's order, WiFi first, made a rule, never measured): a switched-off WiFi silently took Bluetooth with it. Measured on the Pro that day with WL_REG_ON low: the chip id, the patch (3104 records, 2.9 s) and the firmware up. The rule that stands is the cycle rule below.
- **Process** (`payloads/bt`, endpoint `bt`):
  - the UART through its grant;
  - the patch from vendor_a `/firmware/brcm/BCM.hcd` (Android's file for this module, G5SN V17);
  - entropy for the addresses and the client sessions' ids;
  - a heap that grows (ferros_heap, NO-CEILINGS.md): links, channels, queued SDUs and the clients' events, as many as are open.
  - Code: `main.rs` (the UART, bring-up, the beacon, the loop), `link.rs` (ACL and L2CAP), `api.rs` (the client ops).
- **Dev tool:** `tools/husky-radio.sh bt [16-byte hex] | call bt <hex> | kill bt`.

## UART

- **Clocks:** running from boot. CMU_APM mux 1, divider 2, gates automatic (G#1540_1000, G#1540_1818, G#1540_20F4/20F8/2148). The clock is 200 MHz (Google's tables). The divisor is clock/baud: UBRDIV = div/16 − 1, UFRACVAL the remaining sixteenths.
- **Pads:** no pin modes to set.
- **USI init:** USI_CON 1 then 0, USI_OPTION CLKREQ_ON.
- **Port:** UMCON RTS trigger 1 with automatic flow control, interrupts masked at bring-up, FIFOs reset then on, UCON G#3C5 (receive timeout on), 8N1.
- **Interrupt (2026-11-13):** with the kernel's `bt-uart` line (SPI 71, granted with the UART) the serving loop opens the receive and error interrupts (UINTM G#C), clears UINTP and reads the FIFO once more before each wait, and waits on the line, a call, or the 8 s look at the chip (its one deadline). Since 2026-11-24 a second thread waits on the line and rings the loop's word (`RING`), so the loop waits on one word for the line, its callers and a client's end (the kernel's end watch sets bit 63 of the same word); its deadlines are the 8 s look and an unanswered channel open's RTX. Without the line it waits on a call or the look, reading the UART every 4 ms only while scanning. Until then the loop read the UART every 4 ms whatever it did (~250 wakes a second on the 8a). Measured on the 8a with no line (the current kernel): A#9 passes in 64 s, against ~240-280 wakes a second before; the chip up and the patch loaded as before.

## Bring-up

Android's HAL sequence, captured on 2026-10-31 by kprobing `uart_write` / `n_tty_receive_buf2` with the HAL restarted:

1. At 115200: HCI_Reset, then G#FC79 (chip id).
2. G#FC18 to 4 Mbaud, and the host follows.
3. G#FE72 `00 01` (fast download), then G#FC2E Download_Minidriver.
4. Every Write_RAM record of the .hcd (3104), with no wait for events.
5. Launch_RAM, waited for.
6. The chip restarts at 115200: HCI_Reset, then 4 Mbaud again.

Measured: the patch takes 1.4-4 s. Read_Local_Name then says `BCM4398D0 G5SN_V17_HK3 REL BR:0169 FW:ec1b3c47bc`.

Then what the host side needs (since 2026-11-24):

7. Set_Event_Mask: the default (bits 0 to 44) and LE Meta (bit 61), which the default leaves out (2026-11-08: the scan heard nothing without it); LE_Set_Event_Mask: Connection Complete, Advertising Report, Connection Update Complete, Read Remote Features, LTK Request, Data Length Change, PHY Update Complete. The remote parameter request stays masked, so the controller answers a peer's request itself.
8. LE_Read_Buffer_Size: on the Pro A#15 ACL buffers of A#251 bytes (Read_Buffer_Size when LE shares BR/EDR's).
9. LE_Write_Suggested_Default_Data_Length: 251 octets, 2120 us, so a link carries a whole 247-byte K-frame in one LE PDU once the peer agrees (the dev box's BlueZ does).
10. LE_Set_Random_Address: the static random address (below).

### Commands this driver sends

The FCC limits (owner, 2026-11-24): Google's BCM.hcd loaded byte for byte; no vendor command that touches the radio (TX power, channel maps, calibration); no test mode (LE Transmitter Test, DUT mode); WiFi's regulatory blob and country code untouched.

- **Vendor, bring-up only:** G#FC79 (chip id, read), G#FC18 (UART baud rate), G#FE72 (fast download), G#FC2E (Download_Minidriver), the patch's own Write_RAM records and Launch_RAM (G#FC4E), as the file has them.
- **Standard HCI, everything else:** HCI_Reset G#0C03, Set_Event_Mask G#0C01, Read_Local_Name G#0C14, Read_Local_Version_Information G#1001 (the 8 s look), Read_Buffer_Size G#1005, Disconnect G#0406; LE: Set_Event_Mask G#2001, Read_Buffer_Size G#2002, Set_Random_Address G#2005, the legacy advertising parameters, data and enable G#2006, G#2008 and G#200A, Read_Advertising_Physical_Channel_Tx_Power G#2007 (a read, for the corners' circle), scan parameters and enable G#200B and G#200C, Create_Connection G#200D and its cancel G#200E, Connection_Update G#2013, Write_Suggested_Default_Data_Length G#2024, Read_PHY G#2030.
- Never: Write_BD_ADDR (G#FC01), any LE test command (Receiver Test, Transmitter Test and Test End, every version), Enable_Device_Under_Test_Mode (G#1803), any vendor TX power or channel command. LE_Set_PHY and LE_Set_Default_PHY are not sent either: the PHY is the controller's choice (measured 1M).

### Addresses

- The public address (G#FC01) is never written: a radio address belongs to an identity (memory: radio addresses belong to the identity).
- **The beacon:** a fresh non-resolvable private address each time it starts (the pairing beacon's privacy).
- **Scans and connections (2026-11-24):** a static random address (the two top bits 11), drawn from the entropy right once when the driver starts, the same across chip restarts until the driver ends. A passive scan never sends it; an active scan's requests and a connection's CONNECT_IND carry it.
- The controller has one random address register for legacy advertising, scanning and initiating, and it changes only while none runs: the beacon's start pauses the scan around its change, and while the beacon is advertising, scans and connections go out from the beacon's address (the beacon runs only while the device is unbound, AUTH.md). A connection asked while the beacon starts is refused.
- **Not yet the identity's** (AUTH.md "Pairings": the address the person's devices know is derived from the identity key, never stored). A question for the owner, below.

## Photon's beacon

The joiner's proximity half of pairing v2 (photon `PhotonBeacon.kt`, `pairing_beacon.rs`) is 16 opaque bytes, `fgtw::pair::beacon_id`. It goes out as one complete list of 128-bit service UUIDs (AD `11 07`, the UUID little-endian on air, byte 0 the most significant) in a legacy ADV_NONCONN_IND every 100 ms. Each start uses a fresh non-resolvable private address.

On 2026-10-31 the dev box's BlueZ heard `f0e1d2c3-b4a5-9687-7869-5a4b3c2d1e0f` from ferros, exactly the 16 test bytes in order. Android's scanner reads the same AD.

The `bt` endpoint, the driver's own ops (the identity service's; a kist is refused ops 1 to 5 with [1], told apart by SYS_CALLER's CALLER_KIST):

| Op | Request | Reply |
|---|---|---|
| 0 STATE | — | text: the firmware, the beacon, links, channels, sessions, ACL buffers, chip restarts, the L2CAP counts |
| 1 BEACON | 16 bytes | [0], or [1] refused (nobody attested, or the chip) |
| 2 STOP | — | [0] / [1] |
| 3 SCAN | — | [0], or [1]: the pairing scan, passive (nothing on the air) |
| 4 SCAN STOP | — | [0] / [1] |
| 5 HEARD | — | [count][count × 16 bytes]: the 128-bit service UUIDs heard in the last 30 s, newest first, at most 16 |

Ops 6 to 15 are the client ops, below.

## The client ops (stage 0, 2026-11-24)

An LE central for clients: Lumis drives its accessories with it (lumis ACCESSORIES.md: the Spin mount first, raw bytes over an L2CAP credit-based channel, PSM G#81, no GATT).
A client holds a send right on `bt`: a system process granted it, or a kist granted the Bluetooth right (KISTS.md; the kernel hands it as `use bt`, ferros_kist `BLUETOOTH`, code 14).
Nothing goes on the air before a person has attested: OPEN is refused until then.

**Sessions.** A client opens a session and gets its id: eight random bytes, so another holder of the endpoint cannot act in it.
Every op after OPEN starts [op][session u64]; an unknown session is answered empty.
A session's end, any way, takes all of it down at once: its links ended, its channels' held sends refused, its pending connection cancelled, its scan stopped, its held calls answered empty.
It ends by op 15, by its process ending (the kernel's end watch rings the driver: `ferros_sys::caller_watch`; the kernel keeps one watch a watcher, so the newest session's process is watched, and an older one is found gone when a reply to it fails or at the 8 s look), or by a reply that fails.
Measured on the Pro: bttest ended with a link up; the driver took its session down in the same millisecond as the exit, and the link was down 7 ms later (G#16).

| Op | Request after [op][session] | Reply |
|---|---|---|
| 6 OPEN | (just [6]) | [0][session u64], or [1] nobody attested |
| 7 EVENTS | [room u32] | held until the session has an event; then as many whole events as fit `room` (one at least). Empty: the session is gone |
| 8 SCAN | [active u8] | [0] / [1]: scan until op 9 or the session's end; active sends scan requests (scan responses come too) |
| 9 SCAN STOP | — | [0] / [1] |
| 10 CONNECT | [address type u8][address 6][interval min u16, max u16, latency u16, timeout u16]? | [0] asked (EV_CONNECTED follows), [1] refused (the controller's status on the console), [2] another connection is being made |
| 11 DISCONNECT | [link u16] (G#FFFF: cancel the pending connection) | [0] / [1]; the link's channels close at once (why 1), EV_DISCONNECTED follows |
| 12 COC OPEN | [link u16][PSM u16][MTU u16][MPS u16]? | [0][channel id u16] asked (EV_COC_OPENED follows), [1] refused |
| 13 SEND | [link u16][channel u16][the SDU] | held until every K-frame of the SDU is in the transport, then [0]; [1] the channel is not open, the SDU is past the peer's MTU (refused whole, never cut), or the channel closed first |
| 14 COC CLOSE | [link u16][channel u16] | [0] / [1]; closed at once from the client's side (EV_COC_CLOSED why 1) |
| 15 CLOSE | — | [0]: the session ended |

- **Addresses** go as HCI carries them: type 0 public, 1 random (a static random address, as the accessories have, is type 1), the six bytes least significant first. An accessory is named by its address and type exactly (ACCESSORIES.md "Decisions").
- **Connection parameters** in 1.25 ms, connection events and 10 ms units; without them the driver asks 24 to 40 (30 to 50 ms), latency 0, timeout 512 (5.12 s). The accessories ask 6 to 12 (7.5 to 15 ms), latency 0, timeout 100 (1 s); what the controller and peer grant comes in EV_CONNECTED and EV_UPDATED. A peer's request to change them (the L2CAP parameter update) is accepted only when the client named none.
- **A channel** receives SDUs up to its MTU in K-frames up to its MPS (247 when the client names none: one LE PDU at the longest data length less L2CAP's header). Both at least 23, the spec's least.
- **Many threads:** EVENTS calls held at once are answered oldest first, events in order across the replies; a client that wants SDUs in order without its own sequencing keeps one thread receiving. SENDs on a channel go in the order they arrived.
- **What a client sees is whole:** one SDU is one event, never split or joined with another (ACCESSORIES.md); the bytes are opaque to the driver.

Events, each [kind u8][length u16][body]:

| Kind | Body |
|---|---|
| 0 ADV | [event type][address type][address 6][RSSI i8][the advertising data, whole]: event type 0 ADV_IND, 1 direct, 2 scannable, 3 non-connectable, 4 scan response. One report per advertiser and event type waits at most: a newer replaces one the client has not taken |
| 1 CONNECTED | [status][link u16][address type][address 6][interval u16][latency u16][timeout u16]; status G#FF: the chip restarted before the link came up |
| 2 DISCONNECTED | [link u16][why][reason]: why 0 the link ended (reason the HCI code), 1 the chip restarted, 2 the driver ended it (reason the channel `WHY_*` below) |
| 3 COC OPENED | [link u16][channel u16][result u16][the peer's MTU u16, MPS u16, credits u16][ours: MTU u16, MPS u16]; result 0 open, else the peer's L2CAP result, G#FFFF rejected, G#FFFE accepted with numbers outside the spec (closed again) |
| 4 SDU | [link u16][channel u16][the SDU, whole] |
| 5 COC CLOSED | [link u16][channel u16][why] |
| 6 UPDATED | [link u16][status][interval u16][latency u16][timeout u16] |
| 7 LENGTH | [link u16][TX octets u16][RX octets u16] |
| 8 PHY | [link u16][TX PHY][RX PHY]: 1 1M, 2 2M, 3 coded |

The `why` of a closed channel (link.rs `WHY_*`): 0 the peer closed it, 1 the client, 2 the link ended, 3 a K-frame past its credits, 4 an SDU past the MTU, 5 a K-frame past the MPS, 6 more bytes than the SDU's length, 7 a first K-frame without its length, 8 the peer's credits past 65535, 9 an ACL fragment out of place, 10 the UART lost its framing, 11 the chip restarted, 12 the peer's answer out of range, 13 no answer to the open.

### The right's scope (decided by the owner, 2026-11-24)

The owner: "a simple bluetooth permission yes/no and when yes, it lets the app bluetooth whenever it wants. we may add more granularity later."
- **One right, yes or no**, asked once (permissions_asked_once), as built (0f99b62c, in the kernel since ebfc18ee).
- **Everything near:** a kist with the right scans every advertiser in range and sees what it hears; no filtering by what its capsule declares.
- **Anything:** it connects to any device, as an LE central, and opens channels on it.
- **Whenever it wants:** it keeps its scans, links and channels while it is not in front (Lumis holding the mount's channel while it records).
- **Later, maybe:** finer grains (its own accessories only, scans filtered by the capsule, a console mark for a live link) are the owner's to add when wanted; none is built or planned now.
- **Still open, apart from the scope:** whose address the driver wears (the person's, derived from the identity, or per kist; AUTH.md "Pairings"), today a static random address drawn when the driver starts.

## Links and channels

**ACL on H4.** Packets from the controller are joined per link by their packet boundary flags into L2CAP PDUs; a first fragment while a PDU is still being joined, or a continuation with none, means a PDU was lost: the link is ended (why 9).
To the controller each PDU goes in fragments of at most the LE buffer size (A#251), one per free buffer; Number_Of_Completed_Packets frees them; the links take turns, and a PDU's fragments never mix with another's on its link.
The receive side reads the FIFO into a buffer that grows to whatever arrives; a byte that starts no H4 packet means the framing is lost (H4 cannot resync): it is counted and every link ends (why 10).

**L2CAP on LE.** The signalling channel (CID 5): LE credit-based connection request and response, flow control credits, disconnection both ways, command reject, the peer's parameter update (answered; applied with LE_Connection_Update when accepted). A peer opening a channel to us is told LE_PSM not supported (no server role yet); every other request, enhanced credit-based ones among them, is rejected not understood.
ATT (CID 4) is answered so a peer's GATT client does not wait 30 s and drop the link: MTU exchange with 23, the discovery reads Attribute Not Found, any other request Request Not Supported, indications confirmed.
SMP (CID 6): Pairing Request and Security Request are answered Pairing Failed, Pairing Not Supported.

**Credit-based channels.** Our open gives the peer four whole SDUs' worth of K-frames (for MTU 2048, MPS 247: 4 × 9 = 36 credits). The credits an SDU spent go back to the peer in one FLOW_CONTROL_CREDIT_IND when the client takes it from an EVENTS reply, so a channel's memory is the SDUs the client has not read, and a slow client slows the peer instead of losing anything.
Sending, each K-frame spends one of the peer's credits; the first carries the SDU's length; with no credits the SEND stays held until the peer gives more.

**Nothing is lost or reordered silently** (ACCESSORIES.md: a drop or reorder ends the session): a K-frame past its credits, past the MPS, an SDU past the MTU or past its own length, a first K-frame without its length, credits past 65535 close the channel (the peer told with a disconnection request, the client with EV_COC_CLOSED and why); a fragment out of place or lost framing ends the link. An SDU reaches the client whole or not at all. The bytes are the client's: the driver never reads into them.

**The one timer: RTX.** An open the peer has not answered is sent again with the same identifier after 1 s, the timeout doubling each time (the spec's duplicate request, Core Vol 3 Part A 6.2.1, 1 to 60 s); past 32 s it is given up (why 13). Measured 2026-11-24: the dev box's Linux (7.0.14) dropped our first request in every run, logging "ACL packet for unknown connection handle": the request reached its controller before its host had taken the connection's completion. Linux rate-limits that message, and the first duplicate (1 s) went unanswered too: the first link's open was answered at the second duplicate, 3.1 s after it was asked, in every run; on a second link in the same run the first request was answered (39 ms).

## Testing (2026-11-24, the Pro, b1170)

- **The peer:** the dev box's BlueZ (Intel, 5.86, Linux 7.0.14) as the Spin mount's stand-in: a small bluer program, run locally and not committed, advertising connectable with manufacturer data company G#FFFF and 24 bytes (G#A0 ^ i), and echoing every SDU on an LE credit-based channel at PSM G#81, MTU 2048. BlueZ advertises it from the box's public address (its static address cannot be set without root): the connection path is the same for a random address, type 1 instead of 0.
- **The client:** `payloads/bttest` (development only): `ferros-bridge spawn bttest.bin [input] --name bttest --use bt`. It scans actively until it hears the test tag, connects, opens the channel, echoes SDUs one at a time (8, 200, 1024 and 2048 bytes) timing each round trip, echoes a run of 2 KiB SDUs back to back, disconnects, connects again and echoes again; or ends with the link up (input flag 1) to test the teardown.
- **Results:**
  - Found 13 ms to 2.1 s after the scan started (the peer advertises every 100 ms to 1.28 s as BlueZ chooses). Another device in the room sends G#FFFF data too (9 bytes, in a scan response): match the whole tag, never the company alone.
  - Connected 1.28 s after asking at first (36 ms when the peer was just disconnected); asked 6 to 12, latency 0, timeout 100: granted 12 (15 ms), 0, 100; asked 6 to 6: granted 6 (7.5 ms). Data length 251 both ways at once, PHY 1M. The peer asks 24 to 40 every time; refused, the client having chosen.
  - The channel: the peer's MTU 2048, MPS 247 and 9 credits (one SDU's worth); ours 2048, 247, 36.
  - Round trips at 15 ms: 8 and 200 bytes 30 ms (two connection events), 1 KiB 60 ms, 2 KiB 105 ms (median of 16). At 7.5 ms: 15 ms, 15 ms, 60 ms, 120 ms.
  - Back to back: 256 SDUs of 2 KiB (9 K-frames each) echoed in 21.7 s, 24 kB/s each way at once (20 kB/s at 7.5 ms). The peer's window bounds it: 9 credits, one SDU in flight, its credits back only after its program read the SDU; its kernel also logged "Unable to allocate ident: -28" 65 times in the run (Linux running out of signalling identifiers for its credit packets). Not this driver's limit, and not measured apart from it.
  - 2,624 K-frames in and as many out, 342 credit packets: no SDU bad, none out of order, none lost; every ACL buffer back.
  - Disconnect 4 to 6 ms; a second connection and channel in the same run as above.
  - The driver's own ops unchanged: the beacon heard by the dev box (a UUID filter: BlueZ's discovery leaves out advertisers without the discoverable flag), the pairing scan's HEARD answering.

## WiFi and Bluetooth share the chip

The rule, following Android's coordinator: Bluetooth is off before WL_REG_ON is cycled. Cycling it under a powered Bluetooth core wedged the chip and, through WiFi's PCIe, the device. On 2026-10-31 there were two watchdog resets, each within a minute of the WiFi keeper restarting pcie1.

- **WiFi driver:** pcie1 holds the `bt` power switch (its grant slot 18) and turns Bluetooth off just before its REG_ON cycle. A refused switch stops its bring-up with WL_REG_ON untouched (2026-11-16).
- **The switch's off drives its pins (2026-11-16).** Until then off wrote only the data bits of DEV_WAKE and REG_ON, which drive nothing on a pin still in its reset mode (an input): with Bluetooth switched off, or never on since boot, BT_REG_ON read high (gpn8 con 0, dat 1, pull-down on), so pcie1's "Bluetooth off" did nothing and its cycle of a running WiFi chip wedged it (a RAM reload on the Pro, b908: the chip's next BAR read never completed; WIFI.md "A BAR access that never completes"). Off now sets the data bits low, makes the pins outputs, then pulses DBO.
- **Bluetooth driver:**
  - At boot it gives WiFi 16 s first.
  - Every 8 s it asks Read_Local_Version. Silence means the core was reset; it then waits 16 s and comes back up (patch, 4 Mbaud) with the beacon it had, from a fresh address. Repeated failures back off from 4 s to 1024 s.
  - Since 2026-11-24 a reset ends every link (EV_DISCONNECTED why 1, channels why 11) and fails a pending connection; scans come back with the chip. STATE counts the chip restarts and says when the last was.

Verified: two pcie1 restarts with a beacon out. The device stayed up, and the beacon came back and was heard by the dev box.

### Coexistence: measured, and what Android does (2026-11-24, for the owner; nothing changed)

**Measured on the Pro.**
- The flash log's process ends (`ferros-bridge deaths 256 --blocks 16384 --exits`, boots b1138 to b1170, 2026-11-22 21:52 to 2026-11-24 07:40): no pcie1 end at all. The WiFi keeper restarted nothing in those two days, so WiFi took Bluetooth down only at boot (where `bt` waits 16 s for WiFi first) and at a WiFi switch-off (`radio_down`), neither counted there. The 1-2 minute restarts of 2026-10-31 (the memory's) are gone with the joins that failed then.
- This driver's own count (STATE, "chip restarts"): A#0 over the 2026-11-24 session (b1170, WiFi joined at 38 s and up since).
- Not measured: whether either radio's traffic suffers while both run (WiFi throughput with a link at 7.5 ms, and the reverse); the chip arbitrates that itself (below).

**What Android does** (the Pixel kernel tree at /mnt/Harbor/husky-kernel/private: `W` = google-modules/wlan/bcm4398, nitrous = google-modules/bluetooth/broadcom/nitrous.c):
- **Two pins, two drivers, one coordinator.** nitrous drives BT_REG_ON (gpn8-0) with no knowledge of WiFi (nitrous.c 580-649); bcmdhd drives WL_REG_ON (gph3-4, `dhd_wlan_power`, W/dhd_custom_google.c 616-648). The coordinator is bcmdhd's WBRC (W/Kbuild 327-328: `-DWBRC -DWBRC_WLAN_ON_FIRST_ALWAYS`, W/wb_regon_coordinator.c, `/dev/wbrc`): when the Bluetooth HAL opens it, WiFi is powered, its firmware loaded and `WLC_UP` run "so that minresmask is programmed", then its link suspended (W/wl_android.c 5635-5703). WiFi first, always.
- **An ordinary WiFi off does not move WL_REG_ON.** With `WLAN_ACCEL_BOOT` (W/Kbuild 263) WiFi off only suspends the bus (W/wl_android.c 5345-5363); REG_ON is cycled only when `wl_accel_force_reg_on` is set: a bus error or a firmware mode change (W/dhd_linux.c 6619, 6873-6913, 7037-7046). A recovery tries an FLR first, with the "BT quiesce" step off for this chip's PCIe core (buscorerev ≥ 66, W/dhd_pcie.c 2883-2893): an FLR leaves Bluetooth alone; only a failed FLR cycles REG_ON (W/dhd_linux.c 23766).
- **Before a REG_ON cycle, Bluetooth is asked to reset.** After a "big hammer" (the PCIe link down, no trap acknowledged: W/dhd_common.c 855, W/dhd_pcie.c 3325 and 6354), `wl_android_wifi_on` calls `wl2wbrc_req_bt_reset` (W/wl_android.c 5419-5426): a BT_RESET message (G#43) over `/dev/wbrc`, answered by the HAL's acknowledgement (G#81) within 12 s; the HAL closes Bluetooth, acknowledges, and opens it again (`BtHalRecoveryByRestart = true`, shusky bt_vendor_overlay.conf).
- **Runtime coexistence is the chip's own.** The arbiter between the two cores is inside the chip (GCI/SECI; W/siutils.c 500-530 names its chip-control fields), set up by WiFi's firmware from the NVRAM: `btc_mode=1`, `btc_prisel_mask=0x1`, `btc_prisel_ant_mask=0x0`, `clb_swctrl_smask_ant0/1=0xc00ff` (/mnt/Harbor/firmware/husky/bcmdhd.cal_USI_G5SN_9957_V17 533-541, the same in every husky .cal). The driver changes it only around DHCP and only with a SCO link (`BTCOEXMODE`, W/wl_cfg_btcoex.c 462 on). No host AFH classification and no Bluetooth vendor coexistence command is in the tree (the Bluetooth HAL's source is not: unknown). So the shared antenna is arbitrated once WiFi's firmware runs with its NVRAM, as pcie1 loads it; while WiFi is down there is nothing to arbitrate.

**Options** (for the owner; none built):
- **A. As now.** pcie1 switches Bluetooth off before every REG_ON cycle; the driver finds out at its next 8 s look, waits 16 s, and comes back. Every WiFi restart drops every Bluetooth link for 16 to 30 s, and the accessory's own supervision timeout fires first.
- **B. Android's power model.** pcie1 keeps WL_REG_ON up across ordinary restarts (a bus reset or an FLR, as `WLAN_ACCEL_BOOT` does) and cycles it only after a bus error; then a WiFi restart leaves Bluetooth's links alone. A pcie1 change, and a measurement first: whether its restarts today are ones an FLR would cover.
- **C. A doorbell instead of the two timers.** pcie1 tells `bt` before it switches Bluetooth off (its links end at once, with a reason, instead of found 8 s later) and again once WiFi's firmware is up (Bluetooth comes back at once, instead of 16 s later): Android's `/dev/wbrc` handshake, push not poll. It removes the 8 s look and the 16 s wait.
- B and C together are Android's design.

## Photon

Init starts `nfc` and `bt` before Photon and gives Photon `use nfc` and `use bt` (home slots 80 and 84). While the client is unbound, each binding request it mints (every 3.5 min) gets:

- a fresh 32-byte S, committed in the request's `nh` field (`fgtw::pair::nfc_secret_hash`) and served on the NFC tag;
- the request's beacon id (`fgtw::pair::beacon_id(hp, device key, stamp)`), advertised.

Both stop when the device is bound or the identity is forgotten.

## Next

- HOST_WAKE / DEV_WAKE low-power.
- An end-to-end ceremony: attest on the device, then the owner's Photon device adds it by beacon and tap.
- **Stage 1:** the AB Shutter3 remote over classic HID (BR/EDR: inquiry, ACL, L2CAP's BR/EDR channels, HID), its keys delivered by ferros's HID host as keys.
- **Pairing (later, the client's and stage 1's):** pairing records live under the identity as `pair.ble.<address>` (ACCESSORIES.md "Decisions"; AUTH.md "Pairings" has `pair.bt.<peer>` for link keys). Nothing is paired or stored by this driver yet.
- **Extended advertising:** the driver scans with the legacy commands, so it hears legacy advertising only (31 bytes of data, 31 more in a scan response). The beacon's legacy advertising and the extended commands cannot be mixed until a reset (Core Vol 4 Part E 3.1.1), so moving to extended scanning moves the beacon too.
- **The kernel's end watch, one a watcher:** a session older than the newest is found gone only when a reply to it fails or at the 8 s look. A watch per word (`end_watch` keyed by the word as well as the watcher) would watch every session's process.
