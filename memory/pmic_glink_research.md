# pmic-glink Research — QCM6490/SM7325 Bare-Metal EL1

**Target:** Fairphone 5, QCM6490/SM7325, EL1 bare-metal, no Linux
**Goal:** Battery voltage/state, charge control, RTC wall clock
**Date:** 2026-03-17

---

## 1. Architecture Overview

On SM7325/QCM6490, the charger (PM7250B SID 8) is owned by the ADSP (LPASS, a.k.a. "charger_pd"). The APPS processor communicates with the ADSP via:

```
APPS → IPCC doorbell → ADSP
         ↓
    SMEM GLINK FIFO (SMEM items 478/479/480)
         ↓
    GLINK protocol (open channel, TX_DATA, intentless)
         ↓
    RPMsg channel "PMIC_RTR_ADSP_APPS"
         ↓
    pmic_glink_hdr framing (owner + type + opcode)
         ↓
    BATTMGR messages (battery, USB, charge control)
```

The ADSP must already be running (ABL boots it). We do not need to start it.

---

## 2. Hardware Registers

### 2.1 IPCC Mailbox

**Base address:** G#408000 (confirmed in kodiak.dtsi / sm8350.dtsi)

| Register | Offset | Description |
|----------|--------|-------------|
| IPCC_REG_CONFIG | G#08 | Configuration |
| IPCC_REG_SEND_ID | G#0C | Write to ring doorbell on remote |
| IPCC_REG_RECV_ID | G#10 | Read to get pending inbound signal |
| IPCC_REG_RECV_SIGNAL_ENABLE | G#14 | Enable inbound IRQ for a signal |
| IPCC_REG_RECV_SIGNAL_DISABLE | G#18 | Disable inbound IRQ for a signal |
| IPCC_REG_RECV_SIGNAL_CLEAR | G#1C | Acknowledge inbound signal |
| IPCC_REG_CLIENT_CLEAR | G#38 | Clear client state |

**SEND_ID / RECV_ID encoding:**
```
bits[31:16] = client_id
bits[15:0]  = signal_id
```
- `IPCC_NO_PENDING_IRQ` = G#FFFFFFFF (no pending when RECV_ID reads this)

**Relevant IDs (from dt-bindings/mailbox/qcom-ipcc.h):**

| Name | Value |
|------|-------|
| IPCC_CLIENT_LPASS | 3 |
| IPCC_CLIENT_APSS | 8 |
| IPCC_MPROC_SIGNAL_GLINK_QMP | 0 |
| IPCC_MPROC_SIGNAL_SMP2P | 2 |

To ring the ADSP GLINK doorbell from APPS:
```rust
// Signal ADSP that TX FIFO has data:
let hwirq: u32 = (IPCC_CLIENT_LPASS << 16) | IPCC_MPROC_SIGNAL_GLINK_QMP;
// = (3 << 16) | 0 = 0x00030000
write32(IPCC_BASE + 0x0C, hwirq);
```

To poll for inbound signals from ADSP:
```rust
loop {
    let pending = read32(IPCC_BASE + 0x10);
    if pending == 0xFFFF_FFFF { break; }
    // client = pending >> 16, signal = pending & 0xFFFF
    write32(IPCC_BASE + 0x1C, pending); // clear
    // process...
}
```

To enable ADSP→APPS GLINK interrupt (so WFI wakes):
```rust
let adsp_glink: u32 = (IPCC_CLIENT_LPASS << 16) | IPCC_MPROC_SIGNAL_GLINK_QMP;
write32(IPCC_BASE + 0x14, adsp_glink); // enable
```

### 2.2 SMEM (Shared Memory)

**Base:** G#80900000, **Size:** G#200000 (2MB)

SMEM layout (from smem.c):

```
+0x000000: smem_header (offset 0)
  [0x000] proc_comm[4]:   4 × 8 = 32 bytes (legacy IPC, ignore)
  [0x010] version[32]:    32 × 4 = 128 bytes
            version[7] = bootloader version (must be G#B for version 11 or G#C for version 12)
  [0x090] initialized:    u32
  [0x094] free_offset:    u32 (offset to next free byte in heap)
  [0x098] available:      u32 (free bytes remaining)
  [0x09C] reserved:       u32
  [0x0A0] toc[512]:       512 × 16 = 8192 bytes (global item table)
             each smem_global_entry: { allocated: u32, offset: u32, size: u32, aux_base: u32 }

+0x1FC000: smem_ptable (last 4KB of region, at base + size - G#1000)
  magic: [0x24, 0x54, 0x4F, 0x43] = "$TOC"
  version: u32
  num_entries: u32
  reserved[5]: u32
  entry[N]: smem_ptable_entry
    { offset: u32, size: u32, flags: u32, host0: u16, host1: u16,
      cacheline: u32, reserved[7]: u32 }  → 48 bytes each
```

**SMEM Host IDs:**
- `SMEM_HOST_APPS` = 0
- ADSP/LPASS = 2 (matches `qcom,remote-pid = <2>` in DTS)
- `SMEM_GLOBAL_HOST` = G#FFFE

**Private partition lookup:** find entry where (host0==0 && host1==2) or (host0==2 && host1==0).

**Item lookup for global heap (version 11):**
```
smem_global_entry *e = &header->toc[item_id];
if e->allocated == 0: item does not exist
data_ptr = smem_base + e->offset
data_size = e->size
```

### 2.3 GLINK SMEM Items

| Name | SMEM ID | Size | Notes |
|------|---------|------|-------|
| SMEM_GLINK_NATIVE_XPRT_DESCRIPTOR | 478 | 32 bytes | Head/tail pointers |
| SMEM_GLINK_NATIVE_XPRT_FIFO_0 | 479 | 16384 (G#4000) | TX from APPS perspective |
| SMEM_GLINK_NATIVE_XPRT_FIFO_1 | 480 | 16384 (G#4000) | RX from APPS perspective |

**Descriptor layout (32 bytes, all u32 LE):**
```
offset  0: TX tail (updated by remote/ADSP when it reads)
offset  4: TX head (updated by us when we write)
offset  8: RX tail (updated by us when we read)
offset 12: RX head (updated by remote/ADSP when it writes)
```

**FIFO ownership:**
- FIFO_0 (TX): APPS allocates and writes, ADSP reads. APPS owns head[1], ADSP owns tail[0].
- FIFO_1 (RX): ADSP writes, APPS reads. ADSP owns head[3], APPS owns tail[2].

Both FIFOs are circular buffers. All writes/reads are 8-byte aligned.

---

## 3. GLINK Protocol (qcom_glink_native.c)

### 3.1 Message Frame Format

Every GLINK command is a multiple of 8 bytes. The base header is 8 bytes:

```rust
#[repr(C)]
struct GlinkMsg {
    cmd: u16,     // LE - command type
    param1: u16,  // LE - depends on cmd
    param2: u32,  // LE - depends on cmd
    // optional payload follows
}
```

### 3.2 Command Types

| Const | Value | Direction | Description |
|-------|-------|-----------|-------------|
| GLINK_CMD_VERSION | 0 | both | Version negotiation |
| GLINK_CMD_VERSION_ACK | 1 | both | Version response |
| GLINK_CMD_OPEN | 2 | both | Open channel request |
| GLINK_CMD_CLOSE | 3 | both | Close channel |
| GLINK_CMD_OPEN_ACK | 4 | both | Open channel response |
| GLINK_CMD_INTENT | 5 | both | Advertise RX buffer (not used in intentless) |
| GLINK_CMD_RX_DONE | 6 | both | Release RX buffer (not used in intentless) |
| GLINK_CMD_RX_INTENT_REQ | 7 | both | Request more RX space (not used in intentless) |
| GLINK_CMD_RX_INTENT_REQ_ACK | 8 | both | Ack intent request (not used in intentless) |
| GLINK_CMD_TX_DATA | 9 | both | Data frame (first or only) |
| GLINK_CMD_CLOSE_ACK | 11 | both | Close ack |
| GLINK_CMD_TX_DATA_CONT | 12 | both | Data continuation |
| GLINK_CMD_READ_NOTIF | 13 | both | Notification that RX was read |
| GLINK_CMD_RX_DONE_W_REUSE | 14 | both | RX done, reuse buffer |
| GLINK_CMD_SIGNALS | 15 | both | Flow control signals |

### 3.3 Version Negotiation

```
APPS → ADSP:
  cmd=0 (VERSION), param1=1 (GLINK_VERSION_1), param2=features
  features = GLINK_FEATURE_INTENTLESS(BIT(1)) = 2

ADSP → APPS:
  cmd=1 (VERSION_ACK), param1=1, param2=supported_features
  effective_features = our_features & their_features
```

The ADSP will likely respond with `GLINK_FEATURE_INTENTLESS` since pmic-glink uses RPMsg on top which is always intentless. With intentless, all TX_DATA uses `liid=0` and there's no intent exchange.

### 3.4 Open Channel

```
APPS → ADSP: GLINK_CMD_OPEN
  struct: { cmd=2, param1=lcid(u16), param2=name_len(u32) }
  followed by: channel name bytes (null-terminated), padded to 8-byte alignment
  Total size = ALIGN(8 + name_len, 8)

ADSP → APPS: GLINK_CMD_OPEN_ACK
  struct: { cmd=4, param1=rcid(u16), param2=0 }
  (the rcid is their assigned channel ID for the connection we opened)

ADSP → APPS: GLINK_CMD_OPEN (they also open their side)
  struct: { cmd=2, param1=their_lcid, param2=name_len }
  + same name string

APPS → ADSP: GLINK_CMD_OPEN_ACK
  struct: { cmd=4, param1=their_lcid, param2=0 }
```

Channel name: `"PMIC_RTR_ADSP_APPS"` (18 chars + null = 19 bytes → padded to 24 bytes)

### 3.5 Send Data (Intentless Mode)

```
APPS → ADSP: GLINK_CMD_TX_DATA
  struct GlinkTxDataMsg {
      cmd: u16,        // 9
      param1: u16,     // lcid (our local channel ID)
      param2: u32,     // liid = 0 (intentless)
      chunk_size: u32, // bytes of payload in this packet
      left_size: u32,  // remaining bytes after this packet (0 if last/only)
  }  // 16 bytes header
  followed by: payload bytes
  total = ALIGN(16 + chunk_size, 8)
```

Max chunk is G#2000 (8KB). For pmic-glink messages (<G#400 bytes), a single TX_DATA suffices.

### 3.6 FIFO Write Procedure

```rust
fn fifo_write(fifo: &[u8], desc: &Descriptor, data: &[u8]) {
    let head = desc.tx_head as usize;  // read current head
    // write data at head, wrapping if needed
    // advance head by ALIGN(len, 8)
    // update desc.tx_head = new_head (LE u32)
    wmb(); // memory barrier before notifying remote
}
```

Write order: header first, payload second. Each sub-write wraps the circular buffer independently. After updating head pointer, issue IPCC doorbell.

---

## 4. pmic-glink Message Format

### 4.1 Message Header

```rust
#[repr(C, packed)]
struct PmicGlinkHdr {
    owner:  u32, // LE — identifies client/subsystem
    msg_type: u32, // LE — 1=request/response, 2=notification
    opcode: u32, // LE — operation code
} // 12 bytes
```

**Owner IDs (from include/linux/soc/qcom/pmic_glink.h):**
- `PMIC_GLINK_OWNER_BATTMGR` = A#32778 (G#800A)
- `PMIC_GLINK_OWNER_USBC` = A#32779 (G#800B)
- `PMIC_GLINK_OWNER_USBC_PAN` = A#32780 (G#800C)

**Message types:**
- `PMIC_GLINK_REQ_RESP` = 1
- `PMIC_GLINK_NOTIFY` = 2

### 4.2 Battery Manager Opcodes

| Opcode | Value | Direction | Description |
|--------|-------|-----------|-------------|
| BATTMGR_BAT_STATUS | G#01 | req→resp | Battery status (state, voltage, rate, temp) |
| BATTMGR_REQUEST_NOTIFICATION | G#04 | req→ack | Subscribe to async notifications |
| BATTMGR_NOTIFICATION | G#07 | ←notify | Async notification received |
| BATTMGR_BAT_INFO | G#09 | req→resp | Battery static info |
| BATTMGR_BAT_DISCHARGE_TIME | G#0C | req→resp | Time to empty |
| BATTMGR_BAT_CHARGE_TIME | G#0D | req→resp | Time to full |
| BATTMGR_BAT_PROPERTY_GET | G#30 | req→resp | Get single property |
| BATTMGR_BAT_PROPERTY_SET | G#31 | req→resp | Set single property |
| BATTMGR_USB_PROPERTY_GET | G#32 | req→resp | Get USB property |
| BATTMGR_USB_PROPERTY_SET | G#33 | req→resp | Set USB property |
| BATTMGR_WLS_PROPERTY_GET | G#34 | req→resp | Get wireless property |
| BATTMGR_WLS_PROPERTY_SET | G#35 | req→resp | Set wireless property |
| BATTMGR_CHG_CTRL_LIMIT_EN | G#48 | req→resp | Enable charge control |

### 4.3 Battery Status Request/Response

**Request (BATTMGR_BAT_STATUS, opcode G#01):**
```rust
struct BattStatusRequest {
    hdr: PmicGlinkHdr,  // owner=G#800A, type=1, opcode=G#01
    battery_id: u32,    // 0 for primary battery
} // 16 bytes total
```

**Response (same opcode, type=1):**
```rust
struct BattStatusResponse {
    hdr: PmicGlinkHdr,      // 12 bytes
    battery_state: u32,     // bitmask: bit0=discharging, bit1=charging, bit2=critical
    capacity: u32,          // percent 0-100
    rate: u32,              // current in mA (positive=charging, negative=discharging)
    battery_voltage: u32,   // millivolts
    power_state: u32,       // AC power flags
    charging_source: u32,   // 0=none, 1=AC, 2=USB, 3=wireless
    temperature: u32,       // tenths of degrees Kelvin
} // 40 bytes total
```

### 4.4 Battery Property Get

**Request (opcode G#30):**
```rust
struct PropertyGetRequest {
    hdr: PmicGlinkHdr,  // owner=G#800A, type=1, opcode=G#30
    battery: u32,       // battery index (0)
    property: u32,      // property ID (see below)
    value: u32,         // 0 for GET
} // 24 bytes
```

**Response (same opcode, type=1):**
```rust
struct PropertyGetResponse {
    hdr: PmicGlinkHdr,
    property: u32,  // echoed
    value: u32,     // the value
    result: u32,    // 0=success
} // 24 bytes
```

**Battery property IDs:**

| ID | Name | Unit |
|----|------|------|
| 0 | BATT_STATUS | enum |
| 1 | BATT_HEALTH | enum |
| 4 | BATT_CAPACITY | percent |
| 6 | BATT_VOLT_OCV | µV |
| 7 | BATT_VOLT_NOW | µV |
| 8 | BATT_VOLT_MAX | µV |
| 9 | BATT_CURR_NOW | µA (signed) |
| 12 | BATT_TEMP | tenths °C |
| 15 | BATT_CYCLE_COUNT | count |
| 17 | BATT_CHG_FULL | µAh |
| 24 | BATT_CHG_CTRL_EN | 0=disabled, 1=enabled |
| 25 | BATT_CHG_CTRL_START_THR | percent (50-95) |
| 26 | BATT_CHG_CTRL_END_THR | percent (55-100) |

### 4.5 USB Property Get (opcode G#32)

Same request/response format as battery property, but `owner` still G#800A.

| ID | Name | Unit |
|----|------|------|
| 0 | USB_ONLINE | bool |
| 1 | USB_VOLT_NOW | µV |
| 3 | USB_CURR_NOW | µA |
| 4 | USB_CURR_MAX | µA |
| 5 | USB_INPUT_CURR_LIMIT | µA |
| 6 | USB_TYPE | enum |
| 7 | USB_ADAP_TYPE | enum |

### 4.6 Subscribe to Notifications (opcode G#04)

```rust
struct NotificationRequest {
    hdr: PmicGlinkHdr,  // owner=G#800A, type=2 (NOTIFY), opcode=G#04
    battery_id: u32,    // 0
    power_state: u32,   // 0
    low_capacity: u32,  // lower capacity threshold for notification (e.g. 20)
    high_capacity: u32, // upper capacity threshold (e.g. 80)
} // 28 bytes
```

### 4.7 Charge Control (opcode G#48)

```rust
struct ChargeCtrlRequest {
    hdr: PmicGlinkHdr,  // owner=G#800A, type=1, opcode=G#48
    enable: u32,        // 1=enable limit, 0=disable
    target_soc: u32,    // target SOC percent (e.g. 80)
    delta_soc: u32,     // hysteresis (use G#05 = 5)
} // 24 bytes
```

---

## 5. RTC (Real-Time Clock)

### 5.1 Situation on SM7325 / Fairphone 5

The RTC is on PMK8350 (or equivalent "PMK" companion PMIC). On SM7325 systems, the PMK PMIC is typically at SPMI SID 0 (default, configurable via `PMK8350_SID` define). The RTC lives at SPMI registers:

- RTC module base: G#6100
- Read registers: G#6148 (4 bytes, LE, seconds since epoch)
- Write registers: G#6140 (4 bytes, LE)

**However:** PM7325 SPMI at SID 1 has RTC PID absent (per our SPMI arbiter research — PID G#61 is locked). PMK8350 at SID 0 might be accessible if it's in the HLOS APID map.

### 5.2 SPMI Read for RTC (if accessible)

The SPMI arbiter on SM7325 (QCM6490) is at G#C440000. Read registers:
```
Addr = (SID << 16) | (PID << 8) | register_offset_within_PID

For PMK8350 RTC read:
  SID = 0 (default PMK8350_SID)
  PID base = 0x61 (interrupt spec from pmk8350.dtsi: <PMK8350_SID 0x62 ...>)

Wait — DTS says: interrupts = <PMK8350_SID 0x62 0x1 ...>
  → SID=0, PID=0x62 for the alarm interrupt
  → RTC registers are at 0x6100 in full SPMI address space = SID=0, periph=0x61
```

To read: SPMI read at address (0 << 16) | (0x61 << 8) | 0x48 = G#6148

**4-byte read, little-endian = seconds since Unix epoch.**

Re-read LSB if it wrapped during the 4-byte read (carry check).

### 5.3 RTC via pmic-glink?

There is **no pmic-glink RTC message** in the Linux kernel battmgr driver. The RTC is accessed directly via SPMI if the arbiter allows it. If PMK8350 PID G#61 is in our HLOS APID map, we can read it directly. If not, there is no fallback via pmic-glink.

**Action:** Probe SPMI read at SID=0, PID=G#61, offset=G#48 (full address G#6148). If it returns plausible data (non-zero, reasonable epoch value), the RTC is accessible.

---

## 6. Step-by-Step Bare-Metal Init Sequence

### Step 0: Verify ADSP is Running

ABL boots the ADSP. We check readiness via SMEM. If the SMEM descriptor item 478 exists and has non-zero TX head (ADSP has written), ADSP is ready.

### Step 1: SMEM Init

1. Map SMEM at G#80900000 (read-only probe first)
2. Read `smem_header.version[7]` — should be A#11 or A#12
3. Read `smem_header.initialized` — should be A#1
4. Locate private partition for APPS(0)↔ADSP(2):
   - Scan ptable at base + G#1FC000
   - Find entry with host0=0,host1=2 or host0=2,host1=0

### Step 2: Allocate/Find GLINK SMEM Items

The ADSP will have already allocated items 478, 479, 480 when it initialized its GLINK transport. We need to find them:

```rust
// For version 11 (global heap):
let desc_entry = &smem_header.toc[478];
let fifo0_entry = &smem_header.toc[479];  // TX (APPS→ADSP)
let fifo1_entry = &smem_header.toc[480];  // RX (ADSP→APPS)

let desc  = smem_base + desc_entry.offset as usize;   // 32 bytes
let fifo0 = smem_base + fifo0_entry.offset as usize;  // 16KB TX
let fifo1 = smem_base + fifo1_entry.offset as usize;  // 16KB RX

// Descriptor layout:
let tx_tail = desc + 0;  // u32 LE — ADSP reads here, advances tail
let tx_head = desc + 4;  // u32 LE — we advance head after writing
let rx_tail = desc + 8;  // u32 LE — we advance tail after reading
let rx_head = desc + 12; // u32 LE — ADSP advances head after writing
```

If items 478-480 are not allocated: we must allocate them first and wait for ADSP to connect. If the system is in a clean boot state where ABL didn't run ADSP glink, this is needed. Otherwise skip.

### Step 3: GLINK Version Negotiation

Write to TX FIFO:
```
cmd=0, param1=1, param2=2 (GLINK_FEATURE_INTENTLESS)
```
Ring doorbell: `write32(G#408000 + G#0C, G#00030000)`

Wait for RX: poll RX FIFO for VERSION_ACK (cmd=1).

Accept any features the ADSP acknowledges. If ADSP responds with GLINK_FEATURE_INTENTLESS in features, we're in intentless mode (expected).

### Step 4: Open Channel "PMIC_RTR_ADSP_APPS"

Send GLINK_CMD_OPEN:
```
cmd=2, param1=1 (our lcid=1), param2=19 (strlen("PMIC_RTR_ADSP_APPS")+1)
+ "PMIC_RTR_ADSP_APPS\0" + padding to 8-byte align
```
Total = ALIGN(8 + 19, 8) = 32 bytes.

Wait for:
1. GLINK_CMD_OPEN_ACK (cmd=4, param1=rcid) — ADSP confirms our open
2. GLINK_CMD_OPEN from ADSP (they open their side) — respond with OPEN_ACK

After both exchanges, channel is open.

### Step 5: Subscribe to Notifications

Send pmic-glink message via TX_DATA:
```rust
let notif_req = NotificationRequest {
    hdr: PmicGlinkHdr {
        owner: 0x800A,    // PMIC_GLINK_OWNER_BATTMGR
        msg_type: 2,      // PMIC_GLINK_NOTIFY
        opcode: 0x04,     // BATTMGR_REQUEST_NOTIFICATION
    },
    battery_id: 0,
    power_state: 0,
    low_capacity: 15,
    high_capacity: 95,
};
```

Wrap in GLINK_CMD_TX_DATA:
```
cmd=9, param1=lcid, param2=0 (intentless), chunk_size=28, left_size=0
+ 28 bytes of notif_req
```
Total = ALIGN(16 + 28, 8) = 48 bytes.

Ring doorbell.

### Step 6: Request Battery Status

```rust
let status_req = BattStatusRequest {
    hdr: PmicGlinkHdr {
        owner: 0x800A,
        msg_type: 1,   // REQ_RESP
        opcode: 0x01,  // BATTMGR_BAT_STATUS
    },
    battery_id: 0,
};
```
Send as TX_DATA (16 bytes payload).

Poll RX FIFO for response. Parse BattStatusResponse:
- `battery_voltage` in mV
- `capacity` in percent
- `battery_state` bit 1 set = charging
- `temperature` in tenths of K (subtract A#2732 for tenths of °C)

### Step 7: Property Get (Voltage, Current)

For precise voltage/current:
```rust
// BATT_VOLT_NOW (property 7):
PropertyGetRequest { hdr: {owner: 0x800A, type: 1, opcode: 0x30}, battery: 0, property: 7, value: 0 }

// BATT_CURR_NOW (property 9, signed µA):
PropertyGetRequest { hdr: {owner: 0x800A, type: 1, opcode: 0x30}, battery: 0, property: 9, value: 0 }
```

Each is 24 bytes. Response is also 24 bytes with `value` field.

### Step 8: Charge Control

Disable charging (to 80% limit):
```rust
ChargeCtrlRequest { hdr: {owner: 0x800A, type: 1, opcode: 0x48},
    enable: 1, target_soc: 80, delta_soc: 5 }
```

---

## 7. RX Polling Loop

```rust
fn poll_rx(fifo1: *const u8, rx_tail: *mut u32, rx_head: *const u32) -> Option<GlinkMsg> {
    let head = read32_le(rx_head);
    let tail = read32_le(rx_tail);

    if head == tail { return None; } // empty

    // read 8-byte header from fifo1[tail]
    let msg = read_from_fifo(fifo1, tail, 8);
    let total_size = glink_msg_total_size(&msg); // cmd-dependent
    let payload = read_from_fifo(fifo1, tail + 8, total_size - 8);

    // advance tail (wrap at 16384)
    let new_tail = (tail + align8(total_size)) % 16384;
    write32_le(rx_tail, new_tail);
    rmb();

    Some(msg)
}
```

For TX_DATA messages: total_size = 16 + chunk_size, then align to 8.

---

## 8. Protocol Notes for Bare-Metal

### 8.1 If ADSP GLINK is Not Initialized

ABL on QCM6490 (as UEFI/ABL) boots the ADSP for charging purposes before handing off to the kernel. The GLINK transport should already be alive. Signs of life:
- SMEM items 478-480 are allocated (toc entries non-zero)
- TX_HEAD in descriptor may be non-zero (ADSP may have written a VERSION packet)

If items are not allocated, we must allocate them and wait for ADSP to connect (it will send a VERSION packet when it starts glink).

### 8.2 SMEM Allocation (if needed)

```
For global heap (version 11):
  Find free space at smem_header.free_offset
  Write smem_global_entry: allocated=1, offset=free_offset, size=align8(requested)
  Update smem_header.free_offset += size
  Update smem_header.available -= size
```

This requires a hardware spinlock on TCSR mutex 3 (at G#1F40000, lock slot 3).

### 8.3 Expected Response Sizes

| Request | Response size |
|---------|--------------|
| BAT_STATUS | 40 bytes |
| PROPERTY_GET | 24 bytes |
| USB_PROPERTY_GET | 24 bytes |
| REQUEST_NOTIFICATION | 16 bytes (just header+result) |

### 8.4 Intentless Mode Details

- No GLINK_CMD_INTENT exchange needed
- All TX_DATA: param2 (liid) = 0
- No GLINK_CMD_RX_DONE needed from receiver
- Remote allocates temporary buffers per-message internally

### 8.5 Message Routing

When a TX_DATA arrives in the RX FIFO, its payload is a PmicGlinkHdr + body. Route by owner:
- G#800A → BATTMGR handler
- G#800B → USBC handler
- Opcode G#07 → notification (async, not a response to a request)
- Opcode G#30 → property get response (match by pending request)

---

## 9. What ABL Likely Does

On QCM6490/SM7325, ABL (UEFI-based) starts the ADSP PAS (Peripheral Authentication Service loads ADSP firmware). The ADSP firmware includes the charger_pd service, which opens the GLINK channel "PMIC_RTR_ADSP_APPS" on its side. By the time our kernel runs, this channel should be available in SMEM.

Expected state at handoff:
- SMEM items 478/479/480 allocated by ADSP side
- ADSP has written GLINK_CMD_VERSION to FIFO_1 (RX for us)
- We should read and respond to it before sending our own VERSION

---

## 10. Summary: Addresses

| Resource | Address | Notes |
|----------|---------|-------|
| IPCC base | G#408000 | GIC SPI A#229 |
| IPCC SEND_ID | G#40800C | Write to ring doorbell |
| IPCC RECV_ID | G#408010 | Read for pending signals |
| IPCC RECV_CLEAR | G#40801C | Write to ack signal |
| SMEM base | G#80900000 | 2MB |
| SMEM ptable | G#801FC000 | smem_base + G#1FC000 |
| SMEM GLINK desc | via TOC[478] | 32 bytes |
| SMEM TX FIFO | via TOC[479] | 16KB |
| SMEM RX FIFO | via TOC[480] | 16KB |
| ADSP IPCC client | 3 (IPCC_CLIENT_LPASS) | |
| ADSP GLINK signal | 0 (GLINK_SIGNAL_GLINK_QMP) | |
| ADSP doorbell value | G#00030000 | client=3 << 16, signal=0 |
| PMK8350 RTC | SPMI SID=0, G#6148 | 4 bytes LE = epoch seconds |
| PMIC_GLINK_OWNER_BATTMGR | G#800A | A#32778 |
| GLINK channel name | "PMIC_RTR_ADSP_APPS" | 19 bytes with null |
