---
name: feedback_usb_port_wedge
description: If the phone vanishes from USB after a storm of -71 enumeration errors, the HOST port is wedged (Linux "usbN-portN: unable to enumerate"), not the phone — ask for a replug before diagnosing the phone
metadata:
  type: feedback
---

After a burst of `device not accepting address … error -71` / `unable to enumerate USB device` on the Fedora host, Linux stops seeing that port entirely — new connects included — until the cable is replugged. On 2026-10-14 I spent the night believing the phone had panicked; Android had been up and healthy the whole time (uptime 10 h).

**Why:** the host gives up on the port after repeated failures; the phone can't tell.

**How to apply:** when the phone goes silent on USB and the journal shows that pattern, check `journalctl -k | grep usb3-port3` and ask the owner to unplug/replug (no root here to reset the port) before concluding anything about the phone. See [[husky_cold_slot_b]].
