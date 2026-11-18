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
  - a tag read (its UID not written down: it attests the owner since 2026-11-11, AUTH.md "Attest with a tag");
  - the beacon heard by the dev box's BlueZ.

**The REG_ON rule:** cycling WiFi's WL_REG_ON while the BT core is powered wedged the combo chip, and through PCIe the whole phone. That caused two watchdog resets, each within a minute of the WiFi keeper restarting pcie1. Bluetooth itself runs with WL_REG_ON low (measured on the Pro 2026-11-09; the kernel no longer refuses it): the rule is only about cycling WL_REG_ON under a powered Bluetooth core.
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

**2026-11-18: NFC dead after a RAM reload, narrowed, not fixed** (NFC.md "Cold boot against reload"). Cold boot: reset reason 01, ST note `..07 07..`, CORE_RESET answered (reason 02) 3 ms later, with the modem's CP still off. Reload: the same first note, then `6F 02 .. 31 20 00 81 06 01 ..` or `.. 82 06 01 ..` and CORE_RESET never answered, every time with the CP powered (the reload leaves it running; the crowbar only drops its PCIe link). Decide it with a device run before any fix: kill and respawn `nfc` on a cold session with the modem up.

**2026-11-16: the REG_ON rule was broken by an undriven pin.** With Bluetooth switched off (or never on this boot) BT_REG_ON gpn8-0 stays an input and reads high (con 0, dat 1, pull-down on); the old `bt::power(false)` wrote only data bits, so pcie1's "Bluetooth off" did nothing. A WiFi cycle of a running chip then wedged it: after a RAM reload pcie1's stage 6 BAR read (G#6000_0040) never completed, the modem's access on the other link waited behind it, and keeper restarts read vendor id 0. PERST low on WiFi's link ends a stuck access (measured by hand). Fixed: off drives the pins; a reload or jump crowbars the radios (pcie_guard.rs); the kernel takes a link down under any access 2^-3 s unanswered. WIFI.md "A BAR access that never completes".
