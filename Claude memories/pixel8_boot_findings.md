---
name: pixel-8-boot-findings
description: "Critical findings from first Pixel 8 boot attempts — entry code, ABL behavior, display pipeline"
metadata: 
  node_type: memory
  type: project
  originSessionId: 7c0559c4-1684-4522-9dd3-bb5f5f140193
---

## FIRST SUCCESSFUL BOOT (2026-08-14)

ferros booted on the Pixel 8 **Pro** (husky, serial 38261FDJG0023A — NOT shiba; same Tensor G3/ABL) from slot a: LZ4-legacy-compressed boot.img v4 with `--pad 16` (image_size patched), pKVM disabled, bootloader already unlocked.
Ran from ABL jump to ~60s watchdog with NO entry fault — the MMU-check entry guard works; the old 2s crash is gone.
Reset was `APC Watchdog Early` (G#CBEA, CLUSTER0_NONCPU_WDTRESET); ABL then silently retries the slot (~70s frozen-splash cycles) until retries exhaust.
AVB logs ERROR_VERIFICATION for the unsigned image but boots anyway while unlocked.
WATCHDOG DEFEATED same day: writing 0 to WTCON (base+0) on `watchdog_cl0@G#10060000` + `watchdog_cl1@G#10070000` (Samsung s3c2410-style, bases from live DTB via adb on slot b) first thing in kernel_main — kernel then survived 5+ min with no reset, no fallback. Just stopping the counter suffices; no PMU write needed.
PT DISPATCH — SPEC/ACK works, DATA->COMPLETE hangs (2026-08-14): wired the M1 PT dispatch loop into pixel8 kernel_main (was enumerate-only). On hardware: `ferros-bridge diag` gets "SPEC ACK received" (device decodes the SPEC control packet on bulk OUT + replies on bulk IN — first bidirectional PT on the Pixel), then times out waiting for COMPLETE after the DATA blast. NOTE: this dispatch code was ported from the M1 path which was NEVER hardware-validated, so this is its first real-silicon run.
DATA->COMPLETE BISECTED TO DWC3 LAYER (2026-08-15, commit 21e3672): ferros_pt/tests/wire_replay.rs replays the exact bridge->kernel wire sequence on the host, 512-byte padding included — BOTH tests pass. So per-chunk BLAKE3, seq_width, padding handling, finish/COMPLETE encode-decode are all proven sound; the bug is in the DWC3 driver layer (event delivery, bulk_out_read, or bulk_in_send drop — bulk_in_send returns false when busy and the old code ignored it). Tooling now in place to pinpoint it in ONE boot: VENDOR_REQ_DBG (G#5A) EP0 vendor readout serves 16 live counters (kernel PT loop bumps slots 0-11 per dispatch stage, driver fills 12-15) and `ferros-bridge dbg` pretty-prints them. EP0 readout works even when the bulk path is wedged.
ENUMERATION WATCHDOG (2026-08-15): kernel re-runs full PHY + DWC3 init after 4s of USB silence without reaching configured; re-init drops off the bus so the host restarts enumeration fresh. Disarms ONLY on SET_CONFIGURATION; any USB event pushes the deadline. Lesson from v1 (disarmed on bus reset — WRONG): host dmesg showed a NEW flaky-boot failure signature — device attaches at high speed, then EP0 wedges (descriptor read error -110, "device not accepting address" -62, host power-cycles the port and gives up). Attach/chirp working does not mean the data path works. Host-side dmesg (`journalctl -k | grep -i usb`) is the diagnostic of choice for enumeration failures — distinguishes silent-PHY from EP0-wedge instantly.
PT DISPATCH DONE (2026-08-15, commits 8bc8704 + fix validation): FULL bidirectional PT command round trip on Pixel 8 Pro hardware — `ferros-bridge diag` returns 125 bytes of live diagnostics, root hash verified. ROOT CAUSE of the DATA->COMPLETE hang: bulk_out_arm's UNCONDITIONAL ENDTRANSFER errored a DEPCMD on every re-arm and generated a spurious len=0 ep2 completion that ate the first DATA packet after SPEC. Fix: ENDTRANSFER only when bulk_out_resource_idx != 0 (a transfer actually in flight). Found in ONE boot via the EP0 vendor debug counters — the dbg readout showed ep2 events=2 but DATA seen=0 with read-None=1.
RESPONSE FRAMING: device responses are PT-framed (SPEC + DATA packets each pre-padded to exactly 512 so the idle-pump chunks align with packet boundaries; blast mode, NO FIN — bridge pt_recv exits on all_received, so a trailing FIN would sit unread and poison the next command's first recv). Covered by ferros_pt/tests/wire_replay.rs response_recv_replay.
ENUMERATION WATCHDOG VALIDATED: three boots in a row self-healed to enumeration (attempts 2-3, 35-80s wall) with zero button intervention. Iteration drill is now: fastboot flash -> reboot -> wait ~1-2 min -> bridge just works. /tmp/ferros-autoflash.sh (log /tmp/ferros-autoflash.log) automates flash+test when fastboot appears.
HOT-RELOAD DONE (2026-08-15, commits 3a41b59/437e658/1db01aa): kernel-over-USB proven through 3 consecutive generations, zero exceptions each. `bridge reload <flat.bin>` -> PT transfer -> stage -> write-verify -> flush COMPLETE -> clear Run/Stop (graceful detach) -> ic iallu -> jump. KEY FACTS: (1) ABL loads at G#8008_0000 on husky (DRAM base + 512K); (2) generations PING-PONG between exactly two proven slots (ABL base <-> base+32MiB) — jumping to base+64MiB is FATAL (staging writes hash-verify fine, execution never returns, no D+ attach; some carveout). Slot handoff rides the jump's x0 = own_base|1 (ABL's DTB ptr is 8-aligned so bit 0 tags a reload handoff); DIAG reports BASE=/STAGE=. (3) The entry path is fully PC-relative (adrp/adr) — image runs at either slot unmodified. (4) MMU is OFF on Tensor (ABL disables it before the jump — entry guard comment confirms) so staging is straight physical stores and there are no mapping/permission concerns. (5) Bitmap-sizing bug: inbound bitmaps were sized with outbound_bitmap_words(count) (treats count as byte length) — silently dropped any SPEC over 64 chunks; fixed to bitmap_words(count), regression test kernel_sized_send_replay (45KB/95 chunks) covers it. Reload turnaround 30-80s (enumeration watchdog rolls the PLL dice). Fastboot now only needed if a hot kernel bricks itself.
PAYLOAD-EXEC DONE (2026-08-15, commit 775b621): `bridge run <blob.bin> [hex]` hot-loads a program over PT and CALLS it (returns, unlike RELOAD's jump). Cap ferros.dev.run; ABI extern "C" fn(in_ptr, in_len, out_ptr, out_cap) -> u64, entry at blob offset 0, PC-relative only; response [ret:8 LE][out]. Shares the staging slot with RELOAD — mutually-exclusive ok-flags prevent cross-exec. Template: payloads/hello (no_std + payload.ld pinning .text.entry at 0). Hardware-validated: payload reported its own PC = staging slot, echoed input, kernel EXC=0 after repeated runs. This is the dev-lab primitive: poke hardware/run experiments without kernel rebuilds.
BUTTON-FREE PATH: when Android holds the phone (slot b active), `adb reboot bootloader` -> fastboot -> `--set-active=a` + flash + reboot -> ferros. To hand back: fastboot --set-active=b. The button dance is only needed if ferros itself is wedged AND USB is dead.
ENUMERATION FORENSICS + TUNED WATCHDOG (2026-08-15, commit 664a64e): payloads/usbprobe dumps PHY/USBCON/DWC3 over RUN; kernel snapshots (LTSTATE_HIS, LINK_DEBUG_L, DSTS) before each watchdog re-init, DIAG dumps history. FINDINGS: failed inits look IDENTICAL to healthy at the link controller (LTSTATE G#FFFF4, LINKDBG G#115) — the failure is the ANALOG path never lighting up; real fix = eUSB repeater init, which needs a bare-metal Exynos HSI2C driver (eusb_repeater.c in phy-ref, 1146 lines of I2C tuning — filed for later). DSTS bit 17 (RXFIFOEMPTY) discriminated healthy-vs-failed 11/11. Watchdog now two bounds: 2s event-silence (dead attempts emit zero events) + 4s since-init without SET_CONFIGURATION (kills the EP0-wedge mode that rode host descriptor-retries for 15s+). MEASURED: reload turnaround 13-33s (was 13-230s), ~4s per failed attempt (was ~16s).
REBOOT-TO-FASTBOOT FROM FERROS = DEAD END VIA PMU (2026-08-16, commit 8abf6cd): the Pixel reboot mechanism is a PMU scratch write (pixel_reboot driver: exynos-pmu @ G#1546_0000, reboot-cmd-offset G#810 from live DT via Magisk root; magic G#8000_00FC=bootloader/FA=fastbootd/FF=recovery from upstream nvmem-reboot-mode dtsi). BUT S2MPU BLOCKS EL2 WRITES TO THE PMU — proven with pmupoke payload (write G#8000_00FC to G#1546_0810, readback=0). ferros writes USB/DWC3/watchdog fine (S2MPU permits) but PMU is firmware-locked to secure world; that's why Android's EL1 pixel_reboot can stamp it and ferros's EL2 cannot. Real paths for later: (1) BCB/misc-partition write via UFS (Android-standard bootloader control block, ABL reads it, sidesteps S2MPU entirely — also enables slot-flip for `bridge android`), or (2) PSCI SYSTEM_RESET2 (0x84000012) with a vendor cookie through the working EL3/SMC path (cookie unknown, needs firmware source). Meanwhile `adb reboot bootloader` from Android is the working route. DIAGNOSTIC TOOLING: payloads/pmuprobe (read-only register window dump) + pmupoke (write-readback S2MPU test) — the model for any MMIO investigation via RUN, no kernel rebuild. Also: /dev/mem is compiled out on the Pixel Android kernel (no userspace poke); read DT nodes as root via /proc/device-tree.
ENUMERATION FLAKINESS IS THE #1 BLOCKER (2026-08-16): every hardware test rides the enumeration coin-flip; single-shot bridge commands frequently hit "No device found" (device drops off the bus repeatedly even post-config). Seen one 4+ minute total failure. This made reboot-testing miserable. The eUSB repeater HSI2C init (real root-cause fix) should jump ahead of feature work — until enumeration is deterministic, everything else is slow and unreliable. Interim: tight retry loops around bridge commands catch up-windows.
REPEATER GROUNDWORK LAID (2026-08-16, commit aa4efaf): REPEATER.md is the authoritative design doc. Chip = samsung,eusb-repeater, TI-style eUSB2 repeater, 8-bit regs on an Exynos HSI2C (USI-I2C) bus. Bring-up milestone = one read of REV_ID (G#B0). Register map, I2C framing (repeated-start reads), and HSI2C controller layout all captured from tools/pixel8/phy-ref/eusb_repeater.c. payloads/repeaterprobe is the probe skeleton (builds), iterable over RUN with no reflash. TWO UNKNOWNS remain, both from live DT (Android+Magisk root): HSI2C_BASE (parent i2c-bus node 'reg') + REPEATER_ADDR (eusb-repeater node 'reg', 7-bit) — exact adb commands are in REPEATER.md "The two unknowns". NEXT SESSION = get those 2 values from Android root, drop into repeaterprobe, run, iterate REV_ID read → tuning. Reuse ABL's HSI2C timing (don't recompute — same trick that fixed the PHY).
NEXT (reprioritized): (1) fill repeaterprobe's 2 DT constants from Android root + iterate to deterministic enumeration — THE blocker; (2) vault/manifestus onto the Pixel UFS ferros partition (real OS work); (3) BCB-via-UFS for `bridge android`/reboot-fastboot (also the reboot-fastboot path since PMU is S2MPU-blocked); (4) bridge echo/ping/terminal wiring.
PAYLOAD TOOLING PATTERN (established this session): payloads/{hello,usbprobe,pmuprobe,pmupoke,repeaterprobe} — each a no_std blob (payload.ld pins .text.entry at 0), run via `bridge run <blob.bin> [hex]`, kernel CALLS it and returns [ret][out]. This is THE way to investigate hardware on the Pixel: read/probe/poke any MMIO in seconds with zero kernel rebuild and zero reboot risk. Reach for a payload before touching kernel init.

USB WORKS (2026-08-14): ferros enumerates as 1209:4665 on the host. Fix = faithful port of phy_exynos_eusb_initiate (eUSB2 PHY) in kernel_main. The old inline PHY block had right OFFSETS but wrong BITS (rptr_mode b1→b10, pll_fb_div [11:0]→[19:8], pll_ref_div [3:0]→[11:8]) and NEVER cleared TESTSE.test_iddq (b6) — analog IDDQ left the PHY powered down, so nothing reached the wire. Correct bits + test_iddq=0 + real timing (10/10/1000/28/2500 us) → enumerates. Source pulled from Google gs-google kernel (android-gs-shusky-5.15-android15-qpr1, drivers/phy/samsung/) into gitignored tools/pixel8/phy-ref/ (phy-exynos-eusb.c, eusb-con-reg.h, exynos-usb-blkcon.c, eusb_repeater.c, etc). ABL leaves repeater(I2C)+PMU-isol+clocks up across the jump; only analog IDDQ/enable needed. Remaining: udev rule for 1209:4665 (MODE 0666 / uaccess) so ferros-bridge can open it (root-only default → "Permission denied os error 13"), then PT diag + hot-reload loop. NOTE: fastboot fetch works for vendor_boot_a but misc/boot_a are "restricted partition" — not a general readback channel.

--- superseded earlier theories (both wrong) ---
USB ROOT CAUSE (2026-08-14): `ferros_hal::usb::Dwc3Dev::init()` runs QUALCOMM QCM6490 PHY code on Samsung/Exynos silicon — `QCOM_WRAPPER = G#0A6F8800` (Tensor-foreign address) + femtoPHY SIDDQ/POR/UTMI sequence. kernel_main already inits the correct Samsung eUSB2 PHY (G#11100000/G#11110000), then Dwc3Dev::init clobbers it / writes unmapped MMIO (likely the silent stall). DWC3 core regs (GCTL/DCTL/DCFG, Run/Stop asserted usb.rs:867) are standard Synopsys and fine. Live DTB (via adb on slot b, no root) confirmed: PHY node phy@11100000, DWC3 usb@11210000, two phy sub-devices (.10 eUSB2 + .11 USB3), no separate repeater node. FIX: Exynos path in Dwc3Dev::init that skips ALL Qualcomm PHY/QSCRATCH writes, DWC3-core-only (soft reset → PRTCAPDIR device → DCFG HS → event bufs → Run/Stop); ref dwc3-exynos.c; do it with hardware in the loop. Kernel also has a UFS diag channel (writes SNPSID/GCTL/DSTS to ferros partition LBA) — needs root to read back.
Force-reboot note: with watchdog disabled a hung kernel strands the phone; recover via Power+VolDown held ~40s (hard PMIC power-cut) then into fastboot.
ferros parked in slot a; Android active in slot b. Full ABL log: tools/pixel8/abl_dmesg_first_ferros_boot_2026-08-14.txt.

## Pixel 8 Boot — What We Know (2026-03-31)

### ABL (Android Bootloader) Behavior
- ABL is LK-based (Little Kernel), build: ripcurrent-16.4-14540574
- `fastboot boot` command DOES NOT WORK on Tensor G3 — it accepts the image ("Booting OKAY") but never actually jumps to it. Must flash to a slot instead.
- `fastboot oem dmesg` dumps the full ABL log — invaluable for debugging
- `fastboot oem pkvm disable` works, gives EL2 access
- Boot.img v4 header required (header_size=1584, header_version=4)
- Kernel must be LZ4-compressed (legacy format, magic 0x02214C18)
- ABL decompresses LZ4 then jumps to the ARM64 Image entry point
- Hardware watchdog fires at ~60 seconds if kernel doesn't pet it
- A/B rollback: if slot fails, ABL marks it failed and tries other slot or enters fastboot

### What Executes, What Doesn't
- **WORKS**: Code injected at offset 0x100 in the decompressed GrapheneOS kernel (37MB) — branches from offset 0 to 0x100, executes, hangs for 60s (watchdog)
- **WORKS**: MMIO reads (DECON, DPP registers) — don't crash
- **WORKS**: DRAM writes under ABL's MMU — don't crash (but didn't hit FB)
- **CRASHES (2s)**: ferros `_entry` code — the cache clean + SCTLR MMU/cache disable sequence faults on Tensor G3, same issue as M1. Need a `_pixel8_entry` that skips this.
- **FAILS SILENTLY**: Small (12KB) kernel images — ABL decompresses but probably validates image_size or other ARM64 header fields. Injecting into the full 37MB GrapheneOS kernel works.

### Display Pipeline — Samsung Exynos
- Panel: google-shoreline, 1080x2400, command mode, DSC compression
- DECON0 at G#19470000 (Display Enhancement Controller)
- DPP0 at G#19900000 (Display Post Processor)
- DSIM0 at G#19440000 (Display Serial Interface Master — MIPI DSI)
- SYSMMU (Samsung IOMMU) between DECON and DRAM — framebuffer is NOT at a fixed physical address
- `vframe` DMA heap: 512MB shared pool, dynamically allocated, no fixed base
- Writing to random physical addresses (even with CPU MMU off) doesn't hit the display — DECON reads through its own SYSMMU
- **To find FB**: need to read DECON's SYSMMU page tables, or init our own display pipeline, or bypass display entirely and use USB for output

### USB Status (2026-03-31)
- DWC3 at G#11210000 IS clocked (GSNPSID reads valid)
- DWC3 warm_init() added (skip CSFTRST) — but USB never enumerates
- Root cause: ABL shuts down the eUSB PHY before jumping to kernel
- PHY at G#11100000 (samsung,exynos-usbdrd-phy) needs re-init
- Need Samsung eUSB PHY driver init sequence from Android kernel source
- USB SYSMMU at G#11040000 — bypass write safe but not sufficient
- Display SYSMMU at G#19840000 — write-protected by S2MPU (locks CPU on write)
- Display framebuffer still not found (SYSMMU blocks all approaches)

### Next Steps
1. Samsung eUSB PHY init (from AOSP kernel: drivers/phy/samsung/phy-exynos-usbdrd.c)
2. Keep ABL's MMU and cache settings intact (identity-mapped physical memory)
3. Skip framebuffer for now — use USB DWC3 for first output (ABL already inits it: `[UDC] new high-speed device found`)
4. Inject ferros code into GrapheneOS kernel image for size (ABL may reject small images)
5. Or: figure out ABL's minimum image_size requirement and pad ferros appropriately

### Hardware Info from ABL dmesg
- SoC: Zuma (Tensor G3), Product ID 0x09865, Rev B1
- UFS: Samsung KLUDG4UHGC-B0E1, v3.1, 128GB, HS Gear 4
- Panel product ID: 0x0080a00a
- Haptics: CS40L26
- PMIC: S2MPG M/S (Samsung)
- USB: DWC3 with DisplayPort alt-mode (DPTX), eUSB rev 3
