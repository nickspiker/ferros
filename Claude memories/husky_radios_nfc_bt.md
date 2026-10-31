---
name: husky-radios-nfc-bt
description: NFC (ST54L) + Bluetooth (BCM4398 on APM UART) userspace drivers on ferros since 2026-10-31; WiFi must turn BT off before cycling WL_REG_ON or the phone watchdog-resets
metadata:
  node_type: memory
  type: project
  originSessionId: fef83e98-ba6e-4773-9615-f79c25605c30
  modified: 2026-10-31T19:06:07.832Z
---

NFC and Bluetooth run on ferros as the `nfc` and `bt` processes (docs/NFC.md, docs/BT.md), started by init. Photon uses them while unbound: the NFC tag serves S, and BLE advertises `beacon_id`.

Bring-up facts:
- NFC: the ST54L is on hsi2c_0 G#08. Firmware is already on the chip, so nothing is flashed.
- BT: patch from vendor_a `/firmware/brcm/BCM.hcd` at 4 Mbaud, about 3 s.
- Verified on 2026-10-31:
  - a tag read (UID 04 6D F0 EA D2 5B 80);
  - the beacon heard by the dev box's BlueZ.

**The REG_ON rule:** cycling WiFi's WL_REG_ON while the BT core is powered wedged the combo chip, and through PCIe the whole phone. That caused two watchdog resets, each within a minute of the WiFi keeper restarting pcie1.
- **Fix:** pcie1 holds the `bt` power switch (its slot 18) and turns BT off first.
- **Recovery:** `bt` health-checks every 8 s and comes back 16 s later.

**Why:** WiFi on husky restarts often (the keeper restarts pcie1 every 1-2 min when joins fail), so every restart takes BT down, and its advertising address changes each time.

**How to apply:**
- Any new code that touches WL_REG_ON or BT_REG_ON must respect this rule.
- When scanning for the beacon from the dev box, look up the UUID inside the same bluetoothctl scan session. Addresses are fresh after every recovery.

Other traps:
- A native payload's spawn input lands on its `.bss`: copy the input first.
- `bridge spawn epcall --use X` while X is busy holds the kernel's USB service, and the watchdog can fire.

Related: [[radio-addresses-belong-to-identity]], [[photon-on-ferros-no-bootstrap]].
