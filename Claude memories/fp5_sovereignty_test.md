---
name: fp5_sovereignty_test
description: "The FP5 was bricked on purpose as a sovereignty test; Fairphone failed by shipping no firehose/EDL programmer, so the project moved to Pixel"
metadata: 
  node_type: memory
  type: project
  originSessionId: 873bc7fe-6da9-4b85-b1db-5d51c15b7843
---

The Fairphone 5 (FP5/QCM6490) was NOT bricked by accident — Nick bricked it **on purpose as a sovereignty test**: can the owner reassert full low-level control of hardware they own? Fairphone FAILED the test by not shipping a Qualcomm **firehose programmer** (EDL-mode low-level flasher), so there was no sovereign recovery path. That failure is why the project switched to the Pixel 8 (Google), not any technical FP5 limitation per se.

Why this matters: Nick evaluates hardware by whether the OWNER can sovereignly control and RECOVER it at the lowest level, and deliberately drops vendors that lock that away. This is core to ferros's whole premise (owner sovereignty, killswitch, hardware key storage). So: deliberate bricking-as-a-test is a legitimate move here, not a mistake to prevent; and low-level recoverability (fastboot, factory images, EDL-equivalents) is a first-class value, not an afterthought. The Pixel keeps `fastboot oem pkvm disable` + unlockable bootloader + factory images as its recovery surface. See [[pixel8_dedicated_dev_device]] (free to wipe) and [[ufs_linux_handoff]].
