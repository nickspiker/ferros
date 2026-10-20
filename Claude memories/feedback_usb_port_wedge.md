---
name: feedback_usb_port_wedge
description: If the phone vanishes from USB after a storm of -71 enumeration errors, the HOST port is wedged (Linux "usbN-portN: unable to enumerate"), not the phone — ask for a replug before diagnosing the phone
metadata:
  type: feedback
---

After a burst of `device not accepting address … error -71` / `unable to enumerate USB device` on the Fedora host, Linux stops seeing that port entirely — new connects included — until the cable is replugged. On 2026-10-14 I spent the night believing the phone had panicked; Android had been up and healthy the whole time (uptime 10 h).

**Why:** the host gives up on the port after repeated failures; the phone can't tell.

**How to apply:** when the phone goes silent on USB and the journal shows that pattern, check `journalctl -k | grep usb3-port3` and ask the owner to unplug/replug (no root here to reset the port) before concluding anything about the phone. See [[husky_cold_slot_b]].

**Update 2026-10-18:** when the device still enumerates (lsusb shows it) but fastboot/adb hang, `usbreset 18d1:4ee0` (by vendor:product, no root needed) un-wedged fastboot immediately on leviathan; `usbreset /dev/bus/usb/...` said "no such device". When the port itself has given up (no device in lsusb, host log "usb3-port3: Cannot enable"), only a replug or root on `/sys/bus/usb/devices/usb3/3-0:1.0/usb3-port3/disable` helps; a powered hub between box and phone would absorb the storms. Reboot loops (hv bring-up) make the storms far more frequent.
