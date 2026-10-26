---
name: feedback_usb_port_wedge
description: -71 storm then silence = HOST port wedged, recover with tools/usb-kick.sh; silence after a real replug with no host event = the phone's TCPC USB switch (fixed 2026-10-25, logs say which)
metadata:
  type: feedback
---

After a burst of `device not accepting address … error -71` / `unable to enumerate USB device` on the Fedora host, Linux stops seeing that port entirely — new connects included — until the cable is replugged. On 2026-10-14 I spent the night believing the phone had panicked; Android had been up and healthy the whole time (uptime 10 h).

**Why:** the host gives up on the port after repeated failures; the phone can't tell.

**How to apply:** when the phone goes silent on USB and the journal shows that pattern, check `journalctl -k | grep usb3-port3` and ask the owner to unplug/replug (no root here to reset the port) before concluding anything about the phone. See [[husky_cold_slot_b]].

**Update 2026-10-18:** when the device still enumerates (lsusb shows it) but fastboot/adb hang, `usbreset 18d1:4ee0` (by vendor:product, no root needed) un-wedged fastboot immediately on leviathan; `usbreset /dev/bus/usb/...` said "no such device". When the port itself has given up (no device in lsusb, host log "usb3-port3: Cannot enable"), only a replug or root on `/sys/bus/usb/devices/usb3/3-0:1.0/usb3-port3/disable` helps; a powered hub between box and phone would absorb the storms. Reboot loops (hv bring-up) make the storms far more frequent.

**Update 2026-10-20 — self-recovery:** the owner installed udev rule `/etc/udev/rules.d/99-usb3-port3-kick.rules` (group wheel may write `/sys/bus/usb/devices/3-0:1.0/usb3-port3/disable`). `tools/usb-kick.sh` toggles it (= an unplug; the phone re-enumerates in ~3 s) and restarts the adb server; `--if-wedged` kicks only when nothing is on 3-3 and the kernel logged "unable to enumerate" since the last check, and husky-hv/cold/hotload call it from their wait loops. Don't ask for a replug first: kick, retry a few times, then ask. The box has only this one USB port; the owner works over the network, so kicking USB is always fine (it must never touch networking).

**Update 2026-10-25 — a phone-side wedge, now fixed:** silent after a real unplug/replug with NO -71 storm and no host event at all (kicks useless) was the phone: the MAX77759's USB data switch (TCPC G#25, vendor reg G#93 USBSW_CTRL) opens on detach for BC1.2 and nothing re-closed it. ferros's USB re-init now writes G#09 when CC is attached and VBUS present (commit 48ef80b). If it recurs, the flash log's `usb: re-init N: tcpc ... usb switch G#..` lines say which side; volume up+down → fastboot recovers the phone. Not the eUSB2 repeater, not the charger mode (both ruled out).
