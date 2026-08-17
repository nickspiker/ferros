# UFS on Pixel 8 (Zuma) — status and findings — Zil (0)

## MORNING PLAN (2026-08-16 overnight) — debug kernel proved it, mclk fixed, FWTRACE next

**What the custom debug kernel proved (ground truth from Linux's WORKING UFS):**
- **Secure-DMA / GSA theory: DEAD.** Linux runs `WSECURITY=0` (write path "secure") with descriptors in plain low DRAM (`utrdl_dma=0x87570000`) and works. No secure memory / GSA needed.
- **The real bug (partially): mclk was wrong.** Linux logs `mclk: 178000000`; HCI_1US_TO_CNT_VAL reads G#B2=178. ferros hardcoded 133 MHz and calibrated the entire M-PHY for the wrong clock. **FIXED** in `ferros_hal/src/ufs_cal.rs` (MCLK_RATE=178_000_000, derived consts recompute). Committed e03494e.
- **But mclk alone didn't fix it:** ferros full_init still fails at DME_LINKSTARTUP (device silent, MXGR=0), and the live-link doorbell still won't execute even with ABL's own descriptors (`REUSE_OCS=F`). So there's a FURTHER difference vs Linux.

**THE decisive next step — flash the enhanced debug kernel, get the FWTRACE:**
The debug kernel is REBUILT with MMIO write tracing (`FWTRACE <region> <ofs>=<val>` for every std/hci/unipro/pma write across the whole working bring-up) plus the FERROS_UFSP/DESC dumps. Built at `/mnt/Harbor/husky-kernel/out/shusky/dist`. Flash it (needs fastboot — device is on ferros now, so hold Power+VolDown):
```
# device in bootloader fastboot:
/mnt/Harbor/husky-kernel/flash-debug-kernel.sh      # handles logical partitions via fastbootd
# after boot:
adb bugreport /tmp/br.zip && unzip -p /tmp/br.zip | grep -aE 'FWTRACE|FERROS_'
```
That yields Linux's EXACT ordered register-write sequence of a working link bring-up. Diff it against `ferros_hal/src/ufs.rs::full_init` (and the cal in `ufs_cal.rs`) to find the missing/different step that makes DME_LINKSTARTUP succeed. **This is the artifact that ends the guessing.**

**Device state overnight:** boot_a = ferros (with the mclk fix + clean-NOP diag), boot_b = the earlier debug Linux kernel. Device sitting on ferros, enumerating. To return to Android: reflash the husky factory boot images (`/mnt/Harbor/tmp/husky-restore/` + factory image), or the debug kernel via the script above. USB stable on the 1m cable (the 3m cable caused all the earlier flakiness). Reboots decrement A/B retry — if a slot exhausts, ABL falls to the other slot (both bootable).

---


## FULL RE-INIT BUILT + THE NEW WALL (2026-08-16 pm): link-startup handshake never completes

Built the complete Exynos UFSHCI re-init path and tested it exhaustively on hardware. The wall moved from "transfers don't complete" down one layer to "the UniPro link won't start up for us at all."

**What was built (all committed-ready in the tree):**
- `ferros_hal/src/ufs_cal.rs` — faithful Rust port of Samsung's `ufs-cal-if` zuma tables (`/mnt/Harbor/ferros-ref/soc-gs/drivers/ufs/zuma/`): the full `init_cfg` PMA/PCS/UNIPRO pre-link table (57 rows), `post_init_cfg`, `calib_of_hs_rate_b`, the PCS AUX-lane-select window, line-reset tick math, mclk periods. Fixed params measured from ABL via `payloads/ufsdump` (mclk 133.33 MHz, 38.4 MHz refclk, 2 lanes, evt0, AH8 off). The critical `unlock_clocks` (clear HCI_FORCE_HCS auto clock-stop enables) is applied before every PMA/UNIPRO touch — this is what made the vendor region readable without the AP hang, definitively killing the old "vendor region hangs" theory.
- `ferros_hal/src/ufs.rs` — `full_init()` (HCE reset → SW_RST → config_host → device reset → HCE on → pre_link cal → DME_LINKSTARTUP → post_link → lists → NOP → fDeviceInit → HS-G4 PMC), plus `uic_cmd`/`dme_set`/`dme_get`/`query_flag` and a 20-field `InitReport`. Retries the whole hba-enable as a unit (like ufshcd). Config values are ABL's proven ones captured via ufsdump (DBG_SUITE1=G#98913C1C etc), not the gs-kernel runtime values.
- Kernel boot runs `full_init` and reports all fields over DIAG (`UFS_STEPS`, `UFS_FAIL`, `UFS_LS_RES`, `UFS_UECPA_LS`, …). Validated by hot-reload.

**The hardware result (consistent across every variant):**
```
UFS_STEPS=1FE   every step up to LINKSTARTUP ran
UFS_FAIL=9      step 9 = DME_LINKSTARTUP
UFS_LS_RES=1    DME_LINKSTARTUP returns result code 1 = FAILURE
UFS_LS_CNF=1    UNIP_DME_LINKSTARTUP_CNF_RESULT = 1 (same, via the direct UNIPRO reg path)
UFS_UECPA=80000010   PHY-adapter error latched (valid + code G#10)
HCS never sets DP    device-present never asserts; link never comes up
AVAIL_RX=2           M-PHY APB is alive
```

**Isolation experiments (payloads, no reboots needed except where noted):**
- `payloads/ufsdump` — vendor/UNIPRO/PMA regions ALL read fine with clocks unlocked. ABL's programmed state captured (NEXUS=G#FFFFFFFF, PRDT=G#C, etc).
- `payloads/ufsinit2` — bare HCE cycle + config + LINKSTARTUP on **pristine ABL cal** (HCS_before=G#10F, fully healthy handoff): LINKSTARTUP still fails. So the HCE reset throws away the working link and neither ABL's surviving cal nor ours rebuilds it.
- `payloads/ufsdme` — **DME command path WORKS**: `DME_GET` of PA_AvailTx/RxDataLanes returns 2/2, result 0. Direct UNIPRO `DME_LINKSTARTUP_REQ` (G#7850) also fails (CNF=1). So the host DME engine is fine; it's the link handshake with the device that fails.
- `payloads/ufsreset` — tried, all no effect on LINKSTARTUP: (a) real device reset via the `gph5-1` RST_n pin (pinctrl G#1306_0000) driven directly as GPIO, held high; (b) routing `gph5-0` ufs_refclk_out to function 2; (c) PHY isolation check at PMU G#1546_3EC0 (already bypassed, bit0=1); (d) device VCC power-cycle via `gpp0-1` (GPIO_PERIC0 G#1084_0000).

**Diagnosis:** the host side is healthy (DME works, M-PHY APB alive, PHY not isolated, cal applied). `DME_LINKSTARTUP` fails at the PHY-adapter/UniPro handshake — the **device** never re-enters its link boot. The most likely cause is that none of the reset paths tried actually resets the UFS device on this board: a UniPro device already in `LinkUp` ignores link-startup, so unless it's truly power/reset-cycled it will keep refusing. `gph5-1`/`gpp0-1` may not be the real reset/VCC controls on husky, or the writes need a different mux/sequence. The DTB (extracted from the factory `vendor_kernel_boot.img`, `/tmp/husky.dtb`) confirms the pin/PMU/regulator mapping matches the dtsi exactly.

**PRISTINE-LINK TRANSFER TEST (2026-08-16, flashed kernel, definitive):** Flashed a kernel that tests a NOP on ABL's untouched link FIRST (before any reset), reporting the DMA physical address. Fresh boot, ABL's link fully healthy:
```
UFS_HCS_PRISTINE=010F   ABL link healthy: DP+UTRLRDY+UTMRLRDY+UCRDY, UPMCRS=1
UFS_BUF_PHYS_LO=8008F400  UFS_BUF_PHYS_HI=0   our UTRD at physical G#8008_F400 (low DRAM, reachable)
UFS_CTRL_UTRLBA=8008F400            controller holds EXACTLY our buffer address
UFS_DBR_DUP=0                       no doorbell-duplication error
UFS_LIVE_OCS=F   UFS_LIVE_DBR=1   UFS_LIVE_IS=0   NOP accepted, never executes, no interrupt, no error
```
**Addressing is perfect and the DMA-address theory is dead.** Cache-coherency is also ruled out: on husky ABL hands off with the **MMU and D-cache OFF** (`ferros_kernel` boot stub confirms it skips teardown because SCTLR.M is already clear), so DRAM writes are immediate and the controller reads the true descriptor. So: healthy link + correct address + valid UTRD + NEXUS set + list running → the Exynos controller STILL fetches/executes nothing, with no error bit. The transfer-list engine simply does not run for us on ABL's handed-off controller.

**The wall is now two-sided and confirmed:**
- **Reuse ABL's link** → the list-fetch engine never executes our doorbell (this test).
- **Full HCE reset** → can re-arm the list engine but can't rebring the PHY link (DME_LINKSTARTUP fails).

Neither "correct addressing" nor "cache" nor "NEXUS" is the gap. The missing piece is Exynos-specific controller state that (a) ABL's handoff doesn't expose to a second consumer of the transfer list, and (b) a full reset loses along with the PHY link. Prime unexplored suspects for the list engine not running: `HCI_UFS_AXI_DMA_IF_CTRL` (VS+G#F8) / `HCI_WRITE_DMA_CTRL` (VS+G#74) — an AXI-DMA interface enable the controller needs before it will issue descriptor fetches; or the Exynos per-doorbell timer block (`HCI_UTRL_DBR_TIMER_*`, VS+G#144). For the re-link path, the device-side refclk/reset (the device won't re-enter link boot; RST_n pin experiments changed nothing).

**CAL PROVEN CORRECT + FAILURE NARROWED (2026-08-16, later):** A row-by-row audit of the entire `ufs_cal.rs` port against Samsung's zuma `ufs-cal.h`/`ufs-cal-if.c` found **zero functional transcription errors** — all 57 pre-link rows, post-link, HS-rate-B, the lane-skip logic, PCS window, line-reset ticks, and mclk constants are faithful. So the M-PHY cal is NOT the bug. Confirmed independently by `payloads/ufsinit2` (link startup fails even reusing ABL's untouched PHY cal).

Instrumented `full_init` then captured the exact failure state (DIAG, Phase B):
```
UFS_CAL_TO=0        M-PHY PLL LOCKS — EmbCalWait(G#C74) cal-done poll succeeds; analog cal works
UFS_PA_STATE=0      DBG_PA_CTRLSTATE — PHY adapter never leaves idle
UFS_PA_TX_STATE=0   DBG_PA_TX_STATE — host PA NEVER TRANSMITS the link-startup negotiation
UFS_DME_ERR=0  UFS_UEC_PACK=0  UFS_UECPA=0   zero errors anywhere (UECPA G#80000010 was stale/benign — clean here)
UFS_LS_RES=1  UFS_LS_CNF=1     DME_LINKSTARTUP returns plain FAILURE, no error detail
UFS_AVAIL_RX=2      M-PHY APB alive, 2 lanes
UFS_GPH5_DAT=0      gph5 DAT bit1 reads 0 after GPIO_OUT=1 — but pad is muxed to UFS function 2, so GPIO DAT is likely not meaningful (inconclusive on whether reset_n actually toggles)
```

**The precise signature: PLL locked, state clean, but the host PHY-adapter never transmits.** Link startup returns FAILURE because the PA never engages the line negotiation — not because the device fails to answer (the host isn't even talking). This is upstream of the device entirely.

Config was also aligned to the reference re-link path (was speculatively using ABL's captured values): `AXIDMA_RWDATA_BURST_LEN` now includes `WLU_EN`, `DBG_SUITE1/2` back to the gs-kernel values G#90913C1C/G#E01C115F. Neither changed the result (expected — they're not PA-transmit gates).

**THE concrete next lead (register-diff against ABL's working link):** boot fresh (ABL link UP + working, HCS=G#10F), dump the full PMA + PA control register state via an extended `ufsdump`, then dump the same after `full_init`'s failed startup. The register(s) that differ are what ABL sets to enable PA transmit that the zuma cal table alone doesn't — likely a PA power-on / TX-lane-enable / `DME_ENABLE`(UNIPRO G#7830) / `DME_POWERON`(G#7800) step the core ufshcd driver does that `full_init` omits. The cal table is faithful, so the gap is in the *orchestration around* it (a DME enable/power-on or PA TX power-up), not the table values. Suspect specifically: a missing `DME_ENABLE_REQ`/`DME_POWERON_REQ` before `DME_LINKSTARTUP`, or a TX-lane power-up the standard ufshcd core issues that we skip.

**REGISTER-DIFF DONE — HOST SIDE CLEARED, DEVICE IS SILENT (2026-08-16, decisive):** Flashed a kernel that snapshots ABL's WORKING-link PMA/PA state (Phase A, fresh boot HCS=G#10F) and the post-`full_init` FAILED state (Phase B), same 12 registers, one boot. Result:
```
             WORKING(ABL)   FAILED(full_init)
PMA 0x000    00000011       00000011     same (top-level power state)
PMA 0x140    00000000       00000000     same
PMA 0x150    00000088       00000088     same
PMA 0x19C    0000004C       0000004C     same
PMA 0x1A0    000000AE       0000004C     differ — but cal SETS 0x4C; 0xAE is the post-negotiation value
PMA 0xC74    00000019       00000019     same (cal-done)
PMA 0x9F0    00000000       000000D0     differ — cal SETS 0xD0; 0x00 is the post-negotiation value
PMA 0x9F4    00000000       00000000     same (squelch)
PMA 0xA00    00000030       00000030     same
PA_CTRLSTATE 00000000       00000000     same  <- PATX/PACS read 0 on BOTH; NOT diagnostic
PA_TX_STATE  00000000       00000000     same     (kills the earlier "PA never transmits" idea)
MAXRXHSGEAR  00000004       00000000     differ — THE TELL
```
The only PMA diffs (0x1A0, 0x9F0) are registers the cal writes to 0x4C/0xD0 (which the FAILED state correctly shows) and which only change to 0xAE/0x00 once a link successfully negotiates — i.e. **consequences of link-up, not causes of failure.** The failed PMA state exactly matches the correct post-cal state.

**`MAXRXHSGEAR`: 4 (working) vs 0 (failed).** That register holds the max HS gear the DEVICE advertises during the link-startup capability exchange. 0 = the device advertised nothing = **the device is completely silent during link startup.** Combined with PLL-locks + cal-correct + zero-errors, this is conclusive: the host/M-PHY side is fully correct; the flash DEVICE does not respond to our link-startup. It is not being reset (or reference-clocked) into a re-negotiation-ready state.

**So the entire remaining problem is device-side.** The host calibration/sequence is proven correct by three independent lines (audit, ufsinit2, register-diff). The device stays silent. Candidate causes, in order:
1. **Device reset not actually happening.** `full_init` pulses HCI_GPIO_OUT bit0 (the reference `exynos_ufs_dev_hw_reset` mechanism) — but if that GPIO doesn't reach the device reset_n on husky, or the pulse is too short, the device never drops its ABL link state and ignores our new link-startup. Decisive test: on ABL's live link, pulse GPIO_OUT=0 and watch HCS.DP — if the device drops, GPIO_OUT controls reset; if not, it doesn't and that's the bug.
2. **Reference clock to the device not running after our reset** — the device PHY needs REFCLKOUT to boot; if our HCE/SW reset stops it and we don't restart it, the device is clockless. (We keep FORCE_HCS refclk-stop bits clear, but there may be a separate refclk enable ABL/Linux sets via the clk framework.)
3. **My device reset is HARMING** — untested hypothesis: skip the device reset entirely in `full_init` (or give a much longer post-reset settle) and see if link-startup then completes. Linux gives the device time via the slow clk/regulator framework path; our tight sequence may reset then link-startup before the device finishes booting.

**HIBERNATE-EXIT REFUTED (2026-08-16, latest):** Tested whether ABL parked the link in a low-power state gating device traffic. On ABL's live link (HCS=G#10F): `DME_HIBERNATE_EXIT` SUCCEEDS (`HIB_EXIT=0`, and IS bit5 UHXS fires = G#20) — but the device round-trip (`DME_PEER_GET` PA_Granularity) STILL returns G#0A and a NOP STILL never completes. So hibernate is not the blocker. (Caveat: `DME_PEER_GET`=G#0A may itself be arg-confounded — PA_Granularity may not be peer-readable — so it's a weak reachability probe; but the NOP not completing after a successful hibernate-exit is solid.)

**The consistent, robust signature across all live-link tests:** local DME register ops work (DME_GET, config reads, HCS=G#10F, gear-4 negotiated by ABL), but EVERY operation requiring the controller to do a device ROUND-TRIP fails for us — transfers (doorbell accepted, never executes) and peer-DME both. ABL does these round-trips fine; we can't, on the same healthy link, with correct addressing/NEXUS/run-stop and MMU off. And a full re-init to re-own the link fails because the device goes silent at link-startup (MXGR=0). Two-sided wall, deeply characterized, cal proven correct — this is a genuine multi-session controller-state investigation (candidates: Exynos AH8 FSM state, the DL/T_CPORT connection state ABL leaves that a second consumer can't drive, or the exact transfer-fetch gating). Not a quick fix.

**CORE ufshcd.c REVIEWED (2026-08-16) — software confirmed faithful, wall is hardware-state:** Cloned the AOSP common kernel android14-6.1 (`/mnt/Harbor/aosp-common`, sparse `drivers/ufs`) to get the core `ufshcd.c` that orchestrates the exynos vendor hooks (previously reconstructed from memory). Diffed my `full_init` against `ufshcd_hba_execute_hce` / `ufshcd_hba_enable` / `ufshcd_link_startup` / `ufshcd_probe_hba`:
- HCE sequence matches: `hba_stop`(HCE=0) → hce_enable_notify PRE (exynos SW_RST + config_host + GPIO device reset) → `hba_start`(HCE=1, CONTROLLER_ENABLE only; crypto bit only if inline-crypto) → wait ready → POST. Exynos uses the non-BROKEN_HCE path, so NO separate DME_RESET/DME_ENABLE (I correctly omit them). Confirmed.
- Link startup matches: PRE cal → DME_LINKSTARTUP → device-present check → retry-with-hba_enable on fail. Confirmed. (`UECPA` read at 5023 = the benign LINERESET clear, matches my finding.)
- probe order matches: link_startup → NOP (`verify_dev_init`) → fDeviceInit (`complete_dev_init`) → params → PMC. Confirmed.

So the software is faithful — this RULES OUT a missing core step and points hard at hardware/framework state Linux manages that bare-metal doesn't replicate. Two concrete leads the core surfaced:
1. `ufshcd_host_reset_and_restore` calls `ufshcd_scale_clks(hba, true)` (clocks to MAX via the clk framework) BEFORE reinit. On bare metal I inherit ABL's clock state. If ABL scaled the UNIPRO clock away from 133 MHz after handoff, cal timing is wrong → device can't sync → silent. **Verify the ACTUAL CMU_HSI2 UFS clock rate (mux/divider), not just that ABL programmed DBG_PRD for 133 MHz.** (Payload, no reboot.)
2. exynos has NO `.device_reset` vop → `ufshcd_device_reset` returns -EOPNOTSUPP; Linux relies SOLELY on the GPIO reset in hce_enable PRE. Same write I do. So if that GPIO physically resets the device for Linux, it should for me — unless husky's pinmux/regulator (VCC gpp0-1 via the regulator framework) differs in our boot state. Device silence = reset or refclk or power not reaching the device.

(Note on the "analyze the produced assembly" idea: for core ufshcd we now have the C source, which is strictly better than disassembly for logic. Disassembly only pays off for closed blobs like ABL, where the value is limited since ABL's cal == our proven port.)

**HOST SIDE EXHAUSTIVELY VERIFIED (2026-08-16, final elimination) — the residual is wire-level:**

| What | How verified | Result |
|---|---|---|
| PMA (analog) cal | register-diff W vs F | lands correctly |
| PCS (digital) cal | read-back after cal (`UFS_PCS_READBACK=000279F6` = F6/79/02 expected) | lands correctly |
| M-PHY PLL | `CAL_TO=0` (EmbCalWait G#C74) | locks |
| Reference clock | `CLKSTOP=0` (REFCLKOUT_STOP clear) | running to device |
| All UFS clocks | CMU QCH + UNIPRO gate | on |
| Init sequence | diffed vs core `ufshcd.c` (hba_execute_hce/link_startup/probe_hba) | faithful |
| Device reset (GPIO_OUT) | non-cached live-link test (UEC + HCS) | **NO-OP — doesn't reach device reset_n on husky** |

Every measurable host-side prerequisite is correct, yet `DME_LINKSTARTUP` returns result=1 with `MAXRXHSGEAR=0` (capability exchange never completes), zero error codes.

**The Linux paradox:** Linux (core ufshcd + exynos vendor) uses the SAME no-op GPIO reset and re-links successfully from ABL's handoff. So either (a) `HCI_GPIO_OUT` bit0 drives the pad for Linux (EL1, full kernel with pinctrl/regulator/clk frameworks live) but not for us (EL2 bare-metal) due to a pad power/routing state we don't set up, or (b) ABL writes device-specific UNIPRO/PCS attributes (outside the OSS cal table) that HCE/SW reset wipes and Linux's frameworks restore. Both are beyond register-diff to see.

**Conclusion: register-level diagnostics are exhausted.** The host is verifiably correct in every dimension we can read; the link-startup PACP/PA handshake fails for a reason not visible in any status register (no error latches anywhere). Resolving it needs a different CLASS of visibility:
1. **Wire-level** — a logic analyzer / scope on the UFS RESET_n, REFCLK, and M-PHY lanes during link startup, to see whether the device is physically driven and whether it responds. Directly answers "does GPIO_OUT toggle the pad" and "is the device transmitting."
2. **ABL exact-register-diff** — deep-RE ABL's UFS bring-up (addresses built inline via movz/movk, not literal pools, so it needs following the code from the CMU_HSI2 xrefs) to get the EXACT register writes ABL makes, then diff against ours. Finds any device-specific step outside the OSS cal.
3. **Pad/power investigation** — why `GPIO_OUT` is a no-op: check the gph5-1 pad's GPIO-vs-function state, output driver enable, and whether the reset_n path needs a PMIC/GPIO the reference frameworks touch that we don't. If the device truly never resets for us, find husky's real reset path.

**PINMUX DEVIATION FOUND + FIXED, still silent (2026-08-16):** `GPH5CON_BEFORE=0x21` on fresh boot = gph5-1(ufs_rst_n) function 2 BUT gph5-0(ufs_refclk_out) function **1**, not the DT's function 2. Real deviation from the Linux/pinctrl state. `full_init` now routes both to function 2 (`GPH5CON_AFTER=0x22`) before the device reset — but link startup STILL fails (`LS_RES=1`, `MXGR_AFTER=0`). So the refclk PIN routing wasn't the fix (device likely has its own refclk, or the refclk output needs more than the CON mux — drive strength `pin-drv=X3`, or a separate enable). Register/pinmux space is now exhaustively covered.

**ABL RE scoped (2026-08-16):** The extracted `abl` FBPK partition (off G#13400, size G#83000, `/tmp/abl.bin`, disasm `/tmp/abl.asm`) has ZERO references to the UFS register bases (0x1320/0x1328/0x1304) — only CMU_HSI2 (0x1300, 2 movk sites). So ABL inherits an already-initialized UFS from an EARLIER stage (bl2/pbl load ABL from UFS, so they own the UFS bring-up). The UFS init to diff against is NOT in `abl` — it's in bl2/pbl or a loadable (ldfw/gsa/gsa_bl1). FBPK entry parsing to extract those is fragile (stride 0x68, entry base off@+0, size@+8, name@+0x1c — worked for abl/bl31 but not lower stages); needs a proper FBPK2 parser. This is the tractable-but-heavy next path.

**BOOTLOADER RE = DEAD END; THE ANSWER IS A LINUX MMIO TRACE (2026-08-16, decisive strategy pivot):** Extracted all FBPK stages (`/tmp/stg_*.bin`). UFS-base references: pbl=9, bl31=4, bl2=1, abl=0. Disassembled pbl (`/tmp/pbl.asm`, only 16KB): it does UFS **transfers** (UPIU build at G#33xx, doorbell poll at G#1af8 — reads UTRLDBR G#13200058, pokes UTRLCLR, waits bit0) but has **no HCE reset, no M-PHY cal, no DME_LINKSTARTUP**. So the bootloader chain (BootROM→pbl→bl2→abl) INHERITS an already-linked UFS from the **BootROM** (silicon mask ROM, not extractable) and only runs transfers. RE'ing the bootloader cannot reveal the link-init — the bootloader never re-inits.

**Therefore the ONLY AP-side re-init that exists is Linux's** — and Linux works. `full_init` is a faithful static replica of Linux's sequence (proven by the core-ufshcd diff) yet fails, so the bug is a runtime discrepancy no static reading has surfaced. **The decisive next step: capture Linux's ACTUAL MMIO write trace during its (working) UFS init on this exact Pixel 8, then diff against `full_init`'s writes.** Any register Linux writes that we don't — or writes in a different order/value — is the fix. Concretely: boot rooted Android/Linux, add an ftrace/kprobe on `writel`/`ufshcd_writel` (or the exynos `hci_writel`/`unipro_writel`/`pma_writel` inlines) filtered to the UFS reg ranges, trigger a UFS reset/re-init (`ufshcd_link_recovery` via error injection, or a suspend/resume cycle that re-runs `ufshcd_host_reset_and_restore`), and dump the ordered (offset,value) list. That IS the ground truth of the working re-init on this silicon — the one thing register-diffing and static RE can't give us, and it needs no exotic hardware.

**BREAKTHROUGH LEAD — UFS DMA is SECURE-marked, ferros is non-secure (2026-08-16, live-probe via payloads/memprobe):** The pbl-reuse test found ABL's UFS descriptor ring at G#F8C42000 — a region the non-secure CPU reads as ZERO (= secure memory). Chased the mechanism:
- **S2MPU refuted:** the v9 UFS S2MPU (HSI2) is correctly bypassed — `PROT_EN_PER_VID`(0x50)=0 for both USB(HSI0) and UFS(HSI2), and re-writing the CLR sticks. Not the blocker.
- **UFSP (UFS Protector, G#132A0000) is the lead:** `UFSPRSECURITY`(0x10)=G#FFE26492 → read path AxPROT[1]=1 = **non-secure** (so the controller CAN read our non-secure descriptor → doorbell accepted). But `UFSPWSECURITY`(0x110)=**0** → write path AxPROT[1]=0 = **secure**. Read≠write security.
- Writing G#FFE26492 to `UFSPWSECURITY` **did not stick** (stayed 0) — the write-path security register is **locked from non-secure** (set by the secure world / GSA; the UFS node has `gsa-device`).

**Hypothesis (most coherent explanation of the whole saga):** the UFS controller reads descriptors as non-secure (works, from our buffer) but does its write-backs (OCS, response UPIU, data) as SECURE. If the NS bit aliases the address space (secure writes to NS=0 space ≠ our NS=1 buffer), the OCS lands in a phantom secure address we never see → doorbell accepted, OCS stays F, no error, transfer "never completes." ABL avoids this by keeping its descriptors in the secure carveout (G#F8C42000). USB has no protector, so it works. This also explains why re-link never helped: the transport was never the problem.

**Caveat (unconfirmed):** on many systems a SECURE master CAN write non-secure DRAM, which would weaken this. Needs confirmation — e.g. does the OCS appear at the secure-alias of our buffer, or does a full HCE/UFSP reset clear WSECURITY to a non-secure default. If the theory holds, the fix requires secure-world cooperation (GSA protocol via the gsa-device) or running ferros with secure access to that register — an architectural question, not a register poke.

**Two paths from here (next session):**
1. **Make the device actually reset.** Find the true RST_n/VCC control (the `ufs_fixed_vcc` regulator is `gpio = <&gpp0 1>` — confirm gpp0-1 is really wired to VCC-enable and that our GPIO write reaches the pad; may need the pad's pull/drive set, or the reset is via a PMIC register not a SoC GPIO). If the device power-cycles, ABL's-equivalent link startup should take.
2. **Don't reset at all — fix transfers on ABL's live link.** Revisit the ORIGINAL problem with clean tooling: on a fresh ABL boot the link is UP (HCS=G#10F, DP set); only our *transfers* never complete. Early payloads that "proved" NOP-fails-on-clean had the UTRLCLR-offset corruption bug. A minimal, bug-free "init_transfer_list + NOP on the untouched ABL link" test (one reboot to get ABL's link back) would re-check whether a DMA-address / cache / UTRD-format fix makes transfers complete — potentially much closer to done than re-linking. Suspect: UTRD/UCD **physical** address the UFS master sees (node is `dma-coherent`, no iommus → physical DMA), or the `fixed-prdt-req_list-ocs` quirk's UTRD OCS handling.

Reboots roll back the A/B retry counter → ABL fell back to Android once during this session; recovered with `adb reboot bootloader && fastboot --set-active=a && fastboot reboot`. Budget reboots carefully.

---


## THE FINDING (2026-08-16): our UFS commands have NEVER completed on husky

Discovered while building `payloads/miscprobe` (GPT scan → misc partition → boot-control block). Evidence chain:

- On a **fresh boot**, before any payload ran: `UTRLBA=G#8008D400` (the kernel's own buffer) with **doorbell slot 0 already stuck at 1**. The kernel's boot-time `write_block(FERROS_PART_LBA)` (the "FERROS v3" boot log) rang the doorbell and the command never completed. The kernel discards `write_block`'s return value, so this was silent — `link_is_up()` passes, transfers don't.
- No error bits: `IS=0` after 10M-spin poll, `HCS=G#10F` (all ready bits), link up. The command just never finishes.
- A stuck slot is sticky: ringing an already-set doorbell is a no-op, `UTRLCLR` (active-low per-slot clear) did NOT clear it, so every later command silently never starts. Only a reboot (ABL re-init) clears it.
- The write may or may not have reached the device — assume the ferros partition boot-log block is NOT being written.

Consequences: the vault/manifestus work has no storage path on husky until this is fixed. The M1 path is unaffected.

## Forensics (2026-08-16) — every DMA/state hypothesis eliminated, isolated to the vendor region

Kernel boot now runs one read-only `read_block(1)` (GPT header) and dumps the full state via DIAG (`UFS_*` lines; code in `kernel_main`, `ufs_diag[]`). Measured on hardware via hot-reload:

```
UFS_LINK=1          link up
UFS_READ_OCS=F      our read returned the pre-armed INVALID sentinel — controller never wrote a real OCS
UFS_LASTOCS=F       UTRD OCS field never written back
UFS_IS=0            no completion interrupt, no error interrupt
UFS_HCS=10F         DP+UTRLRDY+UTMRLRDY+UCRDY all set, power mode healthy
UFS_DBR=1           doorbell slot 0 accepted, still set — command never completed
UFS_RSR=1           transfer list IS running
UFS_AHIT=0          auto-hibernate DISABLED
UFS_CAP=1383FF1F    32 slots, 64-bit addressing — sane
UFS_RSP0/1=0        response UPIU all zero — controller never wrote a response
UFS_DATA0/1=0       no data delivered (no "EFI PART")
UFS_S2MPU_CTRL0=0   HSI2 S2MPU disabled — our bypass took
UFS_UECPA=80000010  latched PHY-adapter UIC error (valid+code G#10); UECDL/UECN/UECT/UECDME all 0
```

**Eliminated:**
- **S2MPU** — CTRL0 reads back 0 (disabled), and USB DMA works through the identical disable on the HSI0 S2MPU.
- **SysMMU / IOMMU** — the UFS DT node has `dma-coherent` and NO `iommus`; the only HSI2 sysmmu (`sysmmu@131C0000`) is `samsung,pcie-sysmmu`, port `PCIe_CH1`, status **disabled**. UFS DMAs physical addresses directly.
- **Run-stop** (RSR=1), **list-not-ready** (UTRLRDY=1), **auto-hibernate** (AHIT=0).

**Conclusion:** the controller accepts the doorbell but never fetches/executes the request — no descriptor writeback, no response UPIU, no interrupt — with nothing in the DMA path blocking it. That isolates the cause to the **Samsung vendor-specific controller config**, i.e. `HCI_UTRL_NEXUS_TYPE`.

## BREAKTHROUGH + WALL (2026-08-16, with the recoverable watchdog as safety net)

With hangs now auto-recoverable, ran the vendor-region experiments directly (payloads `vendorprobe`, `ufsfix`):

- **The vendor region does NOT hang.** `vendorprobe` read `NEXUS_TYPE` (`G#1320_1140`) fine = `G#FFFFFFFE`. The earlier "freeze" was a payload misfire (miscprobe also wrote the WRONG offset for UTRLCLR — 0x54/UTRLBAU instead of 0x5C — corrupting the base), NOT the vendor read. **The whole "VS region hangs" theory was wrong.**
- **Root cause found:** `NEXUS_TYPE = G#FFFFFFFE` — every tag marked a SCSI nexus EXCEPT bit 0, the tag we use. Necessary fix, but not sufficient (below).
- **Real HAL bug fixed:** `read_block`/`write_block`/query built the Command UPIU with **task tag 1** while `send_command` rings **doorbell slot 0**. In UFSHCI the task tag IS the slot index — must match. Fixed to tag 0 (the NOP path was already correct). This driver was written for the FP5 (QCM6490) and never validated (FP5 bricked), so latent bugs like this were expected.
- **Vendor config was already set by ABL:** `TXPRDT/RXPRDT = G#C`, `DATA_REORDER = G#A`. Applying the full `config_host` block changed nothing.
- **Link is fine:** `DME_HIBERNATE_EXIT` UIC command completes (the UIC interface works); clearing `UECPA` and re-running does NOT re-latch a PHY error, so `G#80000010` was historical link-setup noise, not per-command.
- **THE WALL — even NOP fails on a clean controller:** on a freshly-rebooted controller (`HCS=010F` healthy, `HCE=1`, NEXUS bit 0 set), a bare NOP OUT (`probe()`) returns `OCS=F`, no response, no completion. The controller accepts the doorbell but processes NOTHING — not even the simplest transfer.

### Conclusion: ABL's handoff state can't process our transfer requests

HCE=1 and the link is up, but the doorbell mechanism is inert for us. Two candidate explanations:
1. **Enable-time latching** (cheap fix if true): vendor config like `NEXUS_TYPE` is sampled when HCE goes 0→1. ABL enabled HCE with `NEXUS_TYPE=G#FFFFFFFE`, so bit 0 is latched-clear and our runtime write to set it is visible in the register but unused. **Test:** submit a request in slot **1** (whose NEXUS bit IS set) — ring `UTRLDBR=1<<1` with the UTRD in list slot 1 and UPIU tag 1. If it completes, latching is confirmed and using a pre-set slot is the whole fix (no re-init). Needs a manual 2-slot UTRD list (the HAL is single-slot).
2. **Full re-init required** (the big fix): HCE reset (0→1) with our config applied at enable time, then `DME_LINKSTARTUP` to re-establish UniPro, then M-PHY re-calibration (the Exynos `ufs-cal-if` library — thousands of lines, device-specific tuning). ABL did this; redoing it is a real driver effort.

**RESOLVED — it's path 2 (full re-init).** `payloads/slot1nop` hand-built a 2-slot UTRD list and submitted a NOP in slot **1** (NEXUS bit 1 set since ABL enable): it ALSO fails (`slot1_OCS=F`, doorbell stuck). So latching is NOT the cause — the controller processes no request in any slot. Combined with NOP-fails-clean, this is conclusive: **ABL's handoff state cannot accept host transfers; the controller needs a real re-initialization.**

### The remaining work (a real driver effort, next project)

Full Exynos UFSHCI bring-up from HCE reset:
1. `HCE = 0`, wait not-ready; apply `config_host` vendor block; `HCE = 1`, wait ready.
2. `DME_LINKSTARTUP` (UIC 0x16) → establish UniPro link; wait `HCS.DP`.
3. **M-PHY calibration** — the hard part. Exynos uses `ufs-cal-if` (`drivers/ufs/ufs-cal-if.*`, thousands of lines of device-specific PMA/M-PHY tuning) plus `ufs-vs-regs.h` UNIPRO DME writes (`G#7860`+ hibernate, PA_* attributes). This is what ABL runs; redoing it is the bulk of the effort.
4. Power-mode change to HS gear (`DME_SET` PA attributes + `PA_PWRMode`), then NOP → device init → SCSI.

Alternative worth considering: rather than re-init, find why ABL's already-working link won't take OUR requests — maybe a single "start transfer processing" or power-mode step is missing, cheaper than full cal. But the evidence (NOP fails in a healthy-looking HCE=1/DP=1 state) points to re-init being the honest path.

All the groundwork — vendor region readable, NEXUS semantics, tag/slot fix, UIC command interface working, doorbell clear — is proven and in `payloads/ufsfix` + `slot1nop`.

## (superseded) The blocker: UTRL_NEXUS_TYPE is in a region that hangs on access

- **`HCI_UTRL_NEXUS_TYPE` (`reg_hci`+G#40 = `G#1320_1140`)**: per-tag bit, 1 = this tag is a SCSI/nexus transfer. The Linux driver sets `G#FFFFFFFF` at init (`config_host`) AND per-command (`exynos_ufs_set_nexus_t_xfer_req`). On a stock UFSHCI, RSR+doorbell+ready = execute; on Exynos the controller ignores the doorbell unless the tag's NEXUS bit is set. If ABL left it clear (or a reset cleared it), our command is silently ignored — matches every symptom above.
- **But `reg_hci` (the vendor region, base `G#1320_1100` — confirmed via the driver's ioremap order, resource index 1) HANGS the AP on any CPU read** (froze the phone via miscprobe; `NEXUS_TYPE` at `G#1320_1140` is inside it). And it is NOT auto-hibernate (AHIT=0), so the cause of the gating is something else — most likely a CMU HSI2 UFS clock gate (the vendor region's APB clock stopped) or the auto-clock-gating controlled by `HCI_FORCE_HCS` (VS+G#B4 — itself in the gated region, a catch-22).
- Init also programs `HCI_DATA_REORDER=G#A`, TX/RXPRDT entry sizes, AXIDMA burst — all in the same gated region.

## The trap (learned the hard way, phone frozen twice today)

**The VS block (`G#1320_1100`, from the zuma-ufs.dtsi reg list) is NOT CPU-accessible in our current boot state — a read hangs the AP dead.** No SError we can catch, no recovery, physical power-cycle required (we disable the cluster watchdogs). Likely APB clock gating (`HCI_FORCE_HCS` VS+G#B4 controls clock-stop enables — but it's in the same gated block) or a peripheral protector. The standard HCI region (`G#1320_0000`) reads fine.

**Rule: never touch a new MMIO region from a RUN payload.** A payload hang freezes the phone (no preemption, no watchdog). New-region experiments go in **kernel boot code** instead — if boot hangs, ABL's A/B fallback recovers the phone remotely (costs slot-A retries, resettable via `fastboot --set-active=a`).

## DT reg map (zuma-ufs.dtsi, `ufs@0x13200000`)

```
G#1320_0000  G#200   HCI standard      (validated: readable)
G#1320_1100  G#2000  Vendor specified  (HANGS on CPU read — see trap)
G#1328_0000  G#8000  UNIPRO
G#132A_0000  G#A014  UFS protector
G#1320_4000  G#4000  PHY
G#1320_8000  G#804   CPORT
```

Also relevant: `s2mpu_s0_hsi2@131f0000`, `sysreg_ufs@13020000`, CMU HSI2 clock domain.

## CMU clock gate checked (2026-08-16): core UFS clock appears ON

Found the UFS Q-channel gate: `QCH_CON_UFS_EMBD` at `CMU_HSI2`(`G#1300_0000`)+`G#30C4` = **`G#1300_30C4`** (bits [0]=ENABLE/HWACG, [1]=CLOCK_REQ, [2]=IGNORE_FORCE_PM). Base + offset from `cmucal-sfr.c` / `cmucal-qch.c`. **The CMU read did NOT hang** (it's core infra behind the already-disabled HSI2 S2MPU — safe), and returned:

```
UFS_QCH     = 00000002   ENABLE=0 (HWACG off), CLOCK_REQ=1 (clock requested/on)
UFS_QCH_FMP = 00000002   same for the FMP (inline-crypto) clock
```

So the UFS_EMBD **core clock is on and not auto-gating** — which *weakens* the "vendor region is clock-gated" theory. The vendor-register block must be on a separate gate (a UNIPRO/HCI APB PCLK), or the hang is not a clock issue at all (a link-level low-power stall, or a genuine bus fault on that sub-range).

**Caveat on the hang itself:** it has only ever been observed from the miscprobe *payload*, which also ran a doorbell-recovery loop, `UTRLCLR`, and `read_block` — the vendor read was never isolated as the sole cause. Before chasing the gate further, the hang needs clean confirmation.

## Next steps (in order)

The problem still reduces to: **set `NEXUS_TYPE=G#FFFFFFFF` in the `reg_hci` vendor region (`G#1320_1100`)** — everything else is proven fine. But the vendor region's accessibility is the open question.

1. **Cleanly confirm (or refute) the vendor-region hang** with an isolated single read of `NEXUS_TYPE` (`G#1320_1140`). **The safety prereq is now DONE** — the recoverable cluster watchdog (b602854) auto-resets a hang in ~87s (proven with `payloads/hangtest`), so an isolated vendor read that hangs recovers on its own instead of freezing the phone. Do the read in an on-demand DIAG/command path (after USB is up) so the loop is petting right up to the read; if it hangs, the watchdog recovers and the *next* boot confirms "vendor read hangs" (optionally leave a ramoops/pstore breadcrumb before the read for a definitive post-mortem). If it returns a value → the vendor region is fine and the whole saga was a payload-specific misfire; just set `NEXUS_TYPE=G#FFFFFFFF`.
2. If the vendor region reads fine → the whole saga was a payload-specific misfire; just set `NEXUS_TYPE=G#FFFFFFFF` + `DATA_REORDER=G#A` + PRDT sizes (per `config_host`), retest `read_block(1)` → expect OCS=0 and "EFI PART".
3. If it truly hangs → find the vendor-region APB/UNIPRO PCLK gate (separate from `UFS_EMBD` QCH) or the link low-power state keeping it stalled; ungate via a CMU/sysreg register (accessible), then step 2.
4. Then `miscprobe` works → boot-control block, and the vault/manifestus storage path opens.

The safety prereq in step 1 (framebuffer breadcrumb or recoverable watchdog) is itself the highest-value next task — it de-risks every future new-MMIO experiment on this device, not just UFS.

**Fallback if ungating proves too deep:** a full UFSHCI HCE reset + our own Samsung-style re-init (link startup DME commands + `config_host` VS programming) — heavier, and still needs vendor-region access, so ungating comes first regardless.

## Instrumentation left in place

`kernel_main` runs the read-only `read_block(1)` forensic every boot and reports `UFS_*` via DIAG (see `ufs_diag[]`). It leaves doorbell slot 0 stuck (UFS is non-functional anyway during bring-up). Remove once the command path works. The old boot-time `write_block(FERROS_PART_LBA)` (the "FERROS v3" log) is retired — it was an unverified write that never landed and polluted the partition LBA.

## Misc / boot-control context (slot-rollback fix — BLOCKED on format RE)

Goal was to stop ABL's A/B rollback by marking slot A "successful" (a successful slot is never decremented). Investigated 2026-08-16 from Android + Magisk root; **Pixel does NOT use the AOSP-standard layout**:

- `misc` offset 0: BCB command field (`bootonce-bootloader`). Offset 2048: the ASCII string `theme-dark`, **not** the `bootloader_control` struct. No `BCAB`/`ABAB` magic anywhere in the first 64 KB of misc.
- No dedicated `slot-metadata` partition. Slot state lives in the proprietary **`devinfo`** partition (`DEVI` magic; slot-looking bytes at offset ~32), managed by the gs-common boot HAL `device/google/gs-common/bootctrl/aidl/BootControl.cpp`.
- `tools/pixel8/mark-slot-a-successful.py` assumed the standard offset; its read-verify gate caught the mismatch and refused to write (working as intended — a blind write here is the S2MPU/PMU-blind-write class of bug). Kept as a runnable record of the finding.

**Workaround (works now):** `fastboot --set-active=a` — the bootloader writes the proprietary format correctly itself; resets slot A's retry count each time it rolls back. **Real fix (future):** get `device/google/gs-common` source, decode `BootControl.cpp`'s devinfo storage, then replicate from a rooted-Android script (short-term) or ferros itself (long-term, and itself blocked on the UFS command path above).
