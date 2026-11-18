---
name: stored-capsules-need-flashed-kernel
description: "capsules stored while a RAM-reloaded kernel runs must work under the FLASHED kernel too, or the next cold boot and every jump (the gate's packed kernel) start no process: boot circle, black, no USB"
metadata:
  type: project
---

2026-11-17, the Pro: fgtw, ahau and the vault were stored (masked, sealed) while kernel 267870e3 ran from a RAM reload; the kernel flashed in boot_a (packed in the hv gate) was older. The next jump (b926) ran the gate's packed kernel: every jump step passed, then init never made one spawn call ("jump: no host in 60 s"). The cold boots after (b927, b928) did the same: vault opened, no spawn, no USB, boot circle then black. Flashing forge 9eaa7afa to boot_a (signed with `ferros-mkimg avb-footer`, this morning's target/avb/boot.img as template) brought it back with the same capsules (b929).

**Why:** a jump and a cold boot both run the flashed kernel, never the RAM one; a refused first spawn under 2^-7 s leaves no flash-log line.

**How to apply:** after storing capsules built against a newer kernel, flash that kernel to the booting slot before the session ends (or store only capsules the flashed kernel runs). Keep both slots' gates the same build (HV.md); slot b of the Pro is still the older gate as of b929. tools/husky-cold.sh lost its signing inputs with Harbor tmp and still runs --set-active: don't use it on husky until fixed. Related: [[husky_cold_slot_b]], [[hv_jump_to_ferros]], [[feedback_harbor_tmp_is_scratch]].
