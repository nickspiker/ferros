---
name: ferros-flasher-tool
description: "Planned user-facing ferros flasher: one GPT/partition library, a CLI first, later a cross-platform fluor GUI where people pick sizes and options (keep Android, pure ferros)"
metadata:
  node_type: memory
  type: project
  originSessionId: a83238a3-4495-4510-b376-d5e6ba884c36
  modified: 2026-11-06T17:30:46.387Z
---

Owner, 2026-11-06: "ideally we have a flashing tool for users ... GUI for all platforms cuz fluor ... they can adjust the size and any specific options when they flashey thing like no android, pure ferros, etc." Timing open ("not sure when we want to start that").

**Why:** repartitioning a Pixel for ferros is delicate (the GPT is capped at 34 entries; a clear-and-recreate wipes the A/B slot attributes and bricks it), and people other than the owner will need to do it.

**How to apply:**
- Build every partitioning step as a library first (proposed name `ferros-flash`): read and verify a GPT, plan a layout from the person's choices, apply it as the surgical edit (grow the entry count, shrink userdata, add `ferros` with the BLAKE3("ferros") type GUID, both CRCs, primary and backup), write and verify, the original table kept.
- A CLI on it now (akita, the Pixel 8a, is its first user); the fluor GUI later wraps the same library.
- The per-unit calibration backup (persist, efs, efs_backup, modem_userdata, devinfo, mfg_data, ...) is a built-in first step, never optional ([[akita-pixel8a]], [[husky-factory-partitions-backup]]).
- Options to support: keep Android (userdata shrunk, Android as the escape), pure ferros, sizes chosen by the person.
- Automatic device selection (owner, 2026-11-06: "keep that in mind for the future end user installer/flash tool with auto device selection"): find the phone by its product, show which it is, and never write to a product the chosen image is not for (the dev scripts' tools/lib/devices.sh guard is the same rule).
- No Python (husky's 2026-08 GPT edit was a Python script; that is not the pattern).

**2026-11-10: the plan is written up and shelved in docs/FLASHER.md** (owner: "one signed executable, click thru some GUI options, then click download and flash, then it'll walk user thru unlocking bootloader"; "it does need done for both targets"). Order: firmware read from the phone's own vendor partition (no Google files shipped, a reset loses none), board manifests with `ferros-flash provision`, native fastboot, the CLI wizard, the fluor GUI and signing; hosting and certificates are the owner's.
