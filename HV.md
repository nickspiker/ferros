# HV — ferros hv, the live oracle under Android

The thin EL2 layer that boots stock Android at EL1 on husky and watches it from above: first a dormant pass-through, then a stage-2 map with the UFS controller trapped so Android can run for real without being able to write the boot chain, and finally the m1n1-style tracer that lets us read any register Android touches while it is alive. Decided 2026-10-17 ("Oracle. Sounds delicious."). Code: `ferros_hv/`, tool: `tools/husky-hv.sh`.

## Why

BOOT.md measured the alternatives dead: UFS power-on write protect covers only the bootloader LUs and clears on the device reset Android's own UFS driver performs at probe; the LU layout cannot be re-provisioned without erasing the bootloader; GPT attributes carry no slot state; devinfo is re-synced by ABL from the Boot LU attribute. The only place left to stand between Android and the flash is EL2. ferros already owns EL2 on this phone (ABL jumps to the kernel at EL2 once pKVM is disabled), and Android's kernel is a GKI build that boots fine at EL1 (then without KVM: AVF/Microdroid off, nothing the owner uses).

The same layer is a register oracle: every MMIO access Android makes to a trapped page is a line in a log we can read, which is how the display, camera and modem bring-ups get their ground truth without the handoff dance.

## Measured facts the design rests on (2026-10-17)

- ABL loads the `boot_a` kernel at G#8008_0000 (text_offset G#8_0000) and jumps to its first instruction at EL2 with the MMU and caches off, x0 = DTB. It accepts a kernel blob with the ARM64 header + PE stub ferros's cold image uses.
- The Linux Image in `boot_a` (6.1.124-android14, LZ4-legacy in the boot.img): 35.4 MB, `image_size` G#226_0000, header flags G#A: 4 KiB pages, **physical placement anywhere** (relocatable, KASLR on). text_offset 0.
- ABL puts the ramdisks at G#8600_0000.. (49 MB with bootconfig), DTB 460 KB (base + merged overlays), `/psci` method is SMC, `/reserved-memory` exists with ~60 children.
- DRAM: G#8000_0000–G#FFFF_FFFF and G#8_8000_0000–G#A_FFFF_FFFF (12 GiB). Android's kernel today: "CPU: All CPU(s) started at EL2", KVM IPA 40 bits, cmdline `kvm-arm.mode=protected` (pKVM disabled by `fastboot oem pkvm disable`, so KVM runs unprotected).
- Cores: MPIDR Aff1 = core number 0–8 (Aff2 cluster). PSCI from EL3 starts a core at the entry given to CPU_ON in AArch64 EL2, MMU off, x0 = context.
- The ramoops result block at G#FD60_0000 survives resets and the trip through fastboot; the next ferros boot reads it (`PREV_*`).

## Milestones

### M0 — dormant hv: Android boots and runs under it (code written 2026-10-17, untested)

`boot_a`'s kernel = `[ferros_hv flat binary, padded to G#18_0000][Linux Image]`, LZ4-legacy, mkbootimg v4 with the original (empty) ramdisk and cmdline (`tools/husky-hv.sh`). ABL loads it at G#8008_0000, so the Image sits at G#8020_0000, 2 MiB aligned as its header asks.

At entry the hv:
1. Copies itself to **HV_BASE = G#A_FC00_0000** (the last 64 MiB of DRAM; linked there, the pre-copy code is PC-relative), zeroes bss, takes a stack.
2. Rebuilds the DTB at G#A_FBF0_0000 (ordinary RAM just below, which Linux reserves itself when it reads the tree) with one node added: `/reserved-memory/hv@afc000000 { reg = <0xa 0xfc000000 0x0 0x4000000>; no-map; }`. The `reg` and `no-map` string offsets are taken from the existing strings block. Any surprise in the tree → crumb G#E1 and stop (better than letting Linux allocate over the hv).
3. Programs EL2 so EL1 sees a bare machine: HCR_EL2 = RW | TSC | APK | API | ATA (VM = 0, E2H = 0, TGE = 0, no interrupt routing to EL2); CPTR_EL2 with TFP and TZ clear (FP/SIMD and SVE through, ZCR_EL2 = max where SVE exists); MDCR_EL2 with HPMN = all counters, SPE and trace buffers owned by EL1, no debug traps; CNTHCTL_EL2 = EL1PCTEN | EL1PCEN, CNTVOFF_EL2 = 0; ICC_SRE_EL2 = G#F, ICH_HCR_EL2 = 0; VPIDR/VMPIDR = the real core; HSTR_EL2 = 0; SCTLR_EL1 = reset value; VBAR_EL2 = the hv vectors.
4. `eret` to G#8020_0000 as EL1h, DAIF masked, x0 = the new DTB, x1–x3 = 0.

Afterwards the hv runs only on **SMC traps** (TSC):
- PSCI **CPU_ON** (G#C400_0003): the target core's requested entry and context are recorded by core index, the entry is replaced with `_secondary`, the call forwarded. `_secondary` configures EL2 on the new core and erets to the recorded entry with the context EL3 delivered in x0.
- PSCI **CPU_SUSPEND** (G#C400_0001) and **SYSTEM_SUSPEND** (G#C400_000E) name a resume address too; same swap (harmless for standby states, where the call returns normally).
- Every other SMC is forwarded with x0–x17 and the results returned. HVC from EL1 gets SMCCC NOT_SUPPORTED (Linux at EL1 has no hyp stub to call).
- Any other exception at EL2 writes ESR/ELR/FAR to the report block (G#FD60_0058/60/68) with stage G#E2 and parks the core.

Crumbs: G#FD60_0040 stage (A1 entered, A2 DTB rebuilt, A3 EL2 configured, A4 eret; E1 DTB failed, E2 unexpected trap, E3 panic), +G#48 SMC count, +G#50 CPU_ON count.

**M0 passes when** Android reaches adb, `dmesg` says "CPU: All CPU(s) started at EL1", `/sys/firmware/devicetree/base/reserved-memory/hv@afc000000` exists, all nine cores come online (`/sys/devices/system/cpu/online` = 0-8), the phone suspends and wakes (screen off/on, CPU_SUSPEND resumes through `_secondary`), and `tools/husky-to-ferros.sh` brings slot b back. Restore if it does not: `fastboot flash boot_a /mnt/Harbor/tmp/husky-backup-2026-10-13/boot_a.img`. Failure to boot is self-limiting: ABL burns slot a's three retries and falls back to slot b (ferros), whose DIAG then shows the hv crumbs.

Known gaps to expect at M0: the vendor drivers that talk to pKVM (S2MPU IOMMU, power-domain `pkvm` modules) must take their "no hypervisor" path at EL1 — they did under `pkvm disable` with KVM unprotected, which is close but not identical. Pointer authentication, MTE and SVE are passed through; anything else that traps shows up as an E2 crumb with its ESR, which is exactly the information needed.

### M1 — stage 2 with an identity map and the UFS firewall

HCR_EL2.VM = 1 with VTCR_EL2 for 40-bit IPA, 4 KiB granule, a flat identity map of all DRAM and MMIO built once in the hv's 64 MiB (concatenated tables as needed), **except** the UFS host controller pages (G#1320_0000.., the HCI register block) which are unmapped so every access traps (data abort from lower EL, EC G#24, with the ISS saying load/store, width, register). The handler emulates the access against the real register and, on a doorbell write (UTRLDBR), reads the UTRD → UPIU → SCSI CDB from guest memory first: WRITE/UNMAP/FORMAT/WRITE BUFFER to LU1, LU2, LU3, or to LU0 LBAs inside `boot_b`, `vbmeta_b`, `dtbo_b`, `vendor_boot_b`, `vendor_kernel_boot_b`, `init_boot_b` and the `ferros` partition are answered with a synthetic failure (status CHECK CONDITION, data protect) without reaching the device; QUERY WRITE ATTRIBUTE bBootLunEn and SET FLAG fPowerOnWPEn / fPermanentWPEn likewise. Everything else goes through untouched. Cost: a few traps per I/O (doorbell, interrupt status, completion), microseconds each — unmeasurable against UFS latency. The GPT is read once at hv start from LU0 to know the LBA ranges.

That is the "Android can use the phone but cannot touch the boot chain" promise, enforced below the kernel. Android's own slot (a) stays writable so OTAs of Android itself still work — until they try to switch the Boot LU, which the firewall refuses; the owner switches slots from ferros.

Also M1: the kill switch at EL2. GICv3 group 0 reserved for the hv (the volume-key SPIs 29 and 32 as group 0 → FIQ → EL2 with HCR_EL2.FMO = 1), Linux keeps group 1. Linux's GICD writes to those two interrupts' group bits are trapped (GICD page) and ignored. Vol Up + Vol Down held → PSCI SYSTEM_OFF from EL2, whatever Android is doing.

### M2 — the live oracle

A ring in the hv's RAM of every trapped access (address, value, width, read/write, PC, core, timestamp) and a way to read it: ferros_pt over USB is Android's — so the oracle speaks through a shared-memory page Android exposes (a tiny root tool `cat`s it) or, better, through the display server's debug text when ferros is up next; the simplest first version is the ring surviving in the no-map region and the next ferros boot reading it (same way the report block works). Unmapping is per 4 KiB page, chosen by a list in the hv's config page: the display pipeline (DECON/DSIM/DQE/DPP) for the display bring-up, the camera blocks later, the modem's PCIe window — the register oracle the handoff used to be, but with Android alive and the phone usable while it logs.

### Later

- A hypervisor-side PT endpoint of its own (trap the DWC3 USB controller? No: share it by owning a second endpoint in Android's gadget — too deep; the shared page is enough).
- ferros itself as the EL2 host with Android as a guest *and* ferros processes beside it (the VHE kernel and the hv merge: same stage-2 code, ferros's scheduler owns the cores). Then "boot Android" is a ferros command and the gate's Google chevron starts a guest, not a reboot.
- Signing: once both slots are signed with the owner's key and the bootloader locked, the hv is part of the signed slot-a image and Android cannot be booted without it.

## Status

- 2026-10-17: M0 written (`ferros_hv/src/main.rs`, ~450 lines incl. the FDT editor), builds to 12 KB; `tools/husky-hv.sh` packs `target/hv/boot_a_hv.img` (16.6 MB) from the slot-a backup's Image. Not yet flashed — the owner's call, since it replaces Android's boot image (backup on disk, fallback to slot b automatic).
