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
  - It refuses while WiFi's WL_REG_ON (gph3-4, gpio-hsi2) is low. The coordinator powers WiFi first.
- **Process** (`payloads/bt`, endpoint `bt`):
  - the UART through its grant;
  - the patch from vendor_a `/firmware/brcm/BCM.hcd` (Android's file for this module, G5SN V17);
  - entropy for the advertising address.
- **Dev tool:** `tools/husky-radio.sh bt [16-byte hex] | call bt <hex> | kill bt`.

## UART

- **Clocks:** running from boot. CMU_APM mux 1, divider 2, gates automatic (G#1540_1000, G#1540_1818, G#1540_20F4/20F8/2148). The clock is 200 MHz (Google's tables). The divisor is clock/baud: UBRDIV = div/16 − 1, UFRACVAL the remaining sixteenths.
- **Pads:** no pin modes to set.
- **USI init:** USI_CON 1 then 0, USI_OPTION CLKREQ_ON.
- **Port:** UMCON RTS trigger 1 with automatic flow control, interrupts masked (polled), FIFOs reset then on, UCON G#3C5, 8N1.

## Bring-up

Android's HAL sequence, captured on 2026-10-31 by kprobing `uart_write` / `n_tty_receive_buf2` with the HAL restarted:

1. At 115200: HCI_Reset, then G#FC79 (chip id).
2. G#FC18 to 4 Mbaud, and the host follows.
3. G#FE72 `00 01` (fast download), then G#FC2E Download_Minidriver.
4. Every Write_RAM record of the .hcd (3104), with no wait for events.
5. Launch_RAM, waited for.
6. The chip restarts at 115200: HCI_Reset, then 4 Mbaud again.

Measured: the patch takes 2.2-4 s. Read_Local_Name then says `BCM4398D0 G5SN_V17_HK3 REL BR:0169 FW:ec1b3c47bc`.

The public address (G#FC01) is never written: a radio address belongs to an identity (memory: radio addresses belong to the identity).

## Photon's beacon

The joiner's proximity half of pairing v2 (photon `PhotonBeacon.kt`, `pairing_beacon.rs`) is 16 opaque bytes, `fgtw::pair::beacon_id`. It goes out as one complete list of 128-bit service UUIDs (AD `11 07`, the UUID little-endian on air, byte 0 the most significant) in a legacy ADV_NONCONN_IND every 100 ms. Each start uses a fresh non-resolvable private address.

On 2026-10-31 the dev box's BlueZ heard `f0e1d2c3-b4a5-9687-7869-5a4b3c2d1e0f` from ferros, exactly the 16 test bytes in order. Android's scanner reads the same AD.

The `bt` endpoint:

| Op | Request | Reply |
|---|---|---|
| 0 STATE | — | text |
| 1 BEACON | 16 bytes | [0], or [1] refused |
| 2 STOP | — | [0] / [1] |

## WiFi and Bluetooth share the chip

The rule, following Android's coordinator: Bluetooth is off before WL_REG_ON is cycled. Cycling it under a powered Bluetooth core wedged the chip and, through WiFi's PCIe, the phone. On 2026-10-31 there were two watchdog resets, each within a minute of the WiFi keeper restarting pcie1.

- **WiFi driver:** pcie1 holds the `bt` power switch (its grant slot 18) and turns Bluetooth off just before its REG_ON cycle.
- **Bluetooth driver:**
  - At boot it gives WiFi 16 s first.
  - Every 8 s it asks Read_Local_Version. Silence means the core was reset; it then waits 16 s and comes back up (patch, 4 Mbaud) with the beacon it had, from a fresh address. Repeated failures back off from 4 s to 1024 s.

Verified: two pcie1 restarts with a beacon out. The phone stayed up, and the beacon came back and was heard by the dev box.

## Photon

Init starts `nfc` and `bt` before Photon and gives Photon `use nfc` and `use bt` (home slots 80 and 84). While the client is unbound, each binding request it mints (every 3.5 min) gets:

- a fresh 32-byte S, committed in the request's `nh` field (`fgtw::pair::nfc_secret_hash`) and served on the NFC tag;
- the request's beacon id (`fgtw::pair::beacon_id(hp, device key, stamp)`), advertised.

Both stop when the device is bound or the identity is forgotten.

## Next

- HOST_WAKE / DEV_WAKE low-power.
- An end-to-end ceremony: attest on the phone, then the owner's Photon device adds it by beacon and tap.
