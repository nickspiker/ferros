# Work stopped by the safety filter (Opus 5.5 sessions), for another model to continue

Opus 5.5's safety classifier stopped these pieces of work partway. The text it withheld is not reproduced here; this lists only where each piece stands, what is already committed, and what is left, so a model without that filter (Opus 4.8) can pick it up.

## 1. Cellular modem cold boot (stopped 2026-10-19 night, right after commit f10d1b2)

Done and committed:
- `docs/oracle/MODEM-BOOT-RECON.md`: the modem boot from the GPL sources (GPIO power sequence, PCIe ch0 root complex, shared-memory layout, ROM boot handshake) and the captured boot-channel G#F1 protocol (section order and command numbers, `modem.bin` TOC layout, which per-unit files supply the NV sections).
- `tools/oracle-decode-modem-ring.py`; hv knobs `FERROS_HV_MODEM_RING`, `FERROS_HV_ORACLE_STEP`, `FERROS_HV_ORACLE_SMC`, one-shot oracle marker (afa464a).
- `ferros_ext4` (host-tested), `ferros_kernel/src/partfs.rs`, `diag modem files` / `diag modem readmain` (not yet run on hardware).
- `ferros_kernel/src/modem.rs`: `diag modem s0..s5` (survey, GPIO power-on, PHY, link, endpoint, ROM boot) written and building, never run.

Left to do, in order:
1. Run `diag modem files`, `diag modem readmain` (expect MAIN BLAKE3 prefix 14df88e07f8aad008ab8fff8a4256d12), then `diag modem s0` up through `s5` on a cold boot, one stage at a time, and fix what the hardware says.
2. If s3/s4 misbehave: the full PCIe ch0 register trace (`/tmp/.../pcie-cap.sh` recipe in this session: pages G#1202_0000-2000, G#1204_0000, G#1210_0000-2000, G#1211_0000-5000, G#1212_0000-5000, G#1213_0000-D000, G#1214_0000-3000, G#1240_0000-2000, G#1250_0000, G#1270_0000, G#4006_0000, G#40FF_E000-G#4100_0000, plus SMC log) with the one-shot marker, and diff against modem.rs.
3. After s5 (boot_stage G#3FFF): the link cycle (drop link, wait CP2AP_WAKEUP, Gen3 x2, doorbell G#1_000E), then the channel-G#F1 download in the captured order, then the INIT_START / PHONE_START handshake, then a channel-41 file server.
4. Per-unit NV files (efs `nv_normal.bin`, `nv_protected.bin`; `modem_userdata/replay_region.bin` on f2fs) must never be logged or committed.

## 2. Camera DMA command lists (stopped 2026-10-19 during the camera work)

Stopped while writing a userspace tool to read the camera HAL's PDMA command lists out of its process memory on Android. Still open: getting the front end's DMA configuration (see docs/oracle/CAMERA.md "Next"). The hv route (`FERROS_HV_PDMA_DUMP`) is parked; the next idea there is trapping the ISPFE SYSMMU register pages to see where Linux writes the page-table base.

## 2b. Camera first frame (stopped 2026-10-20 morning)

Done and committed: the PDMA list format and the full decoded front-end program (docs/oracle/CAMERA.md, camera-ispfe-pdma-writes.txt, tools/oracle-decode-pdma.py); tools/camera-prog.py (builds a CPR2 program: CPU setup before the PDMA start, the PDMA program, CPU setup after, buffer IOVAs moved to PA G#8800_0000+); `diag cam frame` in camera.rs (FERROS_CAM_FRAME=<local bundle> embeds init + program at build time; the bundle stays in target/cam, never the repo).
State at the stop: on a cold boot the sensor streams and CSI link 1 counts frames, the whole program applies with no hang, but nothing in the ISPFE pages changes and no output buffer is written. The link-to-front-end path is still not running.
Left to do: find what connects CSI link 1 to the ISPFE input (compare a full ISPFE + CSIS register snapshot from Linux while streaming against ferros's), then read a frame back.
Also open: the cold vault accepts only the first write per boot and that write does not survive a reboot (store put returns "vault error" afterwards).

## 2c. Kernel vault moved onto manifestus (stopped 2026-10-20 mid-verification)

Done: ferros_kernel/src/kvault.rs (manifestus over two UFS mirror regions of the ferros partition, values sealed, a sealed name index for `store ls`); vault_boot/service switched to it, no silent format any more; flash log (flashlog.rs, `diag log`) and manifestus storage events logged; `diag kv selftest`; manifestus no_std fix and vsf 0.12 bump pushed.
Verified on the phone: genesis once (17.6 s), later opens ~6 ms, small values and genesis survive reboots, a 30 KB value stored through `store put` reads back identical with the current build.
Left: the earlier `Corrupt("furrow 0 unsealed")` reads came from values written by an older kernel build in the same regions (each such 30 KB set cost ~400 blocks); confirm with a reboot that new large values persist, decide whether to wipe the regions written by the old builds, re-provision `init` and `sealed`, then remove ferros_vault from the kernel (shim.rs entry_genesis, UfsDevice, ira.rs AnchorKey, wairua NonceSource).
Also: kete, photon and other dependents of ../vsf still ask for vsf 0.11 and will not resolve against 0.12.

## 3. Attestation / Play Integrity discussion (2026-10-19)

Conversation only, no code: what spoofing verified-boot / attestation state would expose for banking and Wallet apps. Nothing in the repo depends on it.
