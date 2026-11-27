---
name: feedback-pro-reload-freely
description: "Reload the Pro whenever the work needs it, no ask; the owner unplugs it when he doesn't want that"
metadata:
  type: feedback
---

Owner, 2026-11-17: "You can reload the pro all you want. If I don't want you to, I generally unplug it."

**Why:** asking before every reload cost round trips while the owner was waiting at the device; the cable is his off switch.

**How to apply:** kernel reloads, payload restarts and capsule stores on the Pro (husky, FERROS_PORT=1-1.2 since 2026-11-27; it was 1-2.2: check `/sys/bus/usb/devices/*/product` for "ferros" when a port says no device) need no go; a missing device on the port means hands off. Still say when a reload drops the identity, so he knows to type his handle. Flashing partitions, fastboot, Android-side experiments and anything on the 8a (his wife's identity) keep their own rules: [[android_boot_experiments_rescue_party]], [[akita_pixel8a]], [[feedback_lock_prep_not_lock]].
