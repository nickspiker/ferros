# ira entropy sources — husky (Pixel 8, Tensor G3 / "zuma")

Grounded 2026-08-20 against the gs-google reference tree at `/mnt/Harbor/ferros-ref` (soc-gs, shusky, display-gs).
Companion to VAULT-KEY.md (anchor-key sourcing) and photon/docs/fleet-key.md (what the ira is FOR).

## Why this matters: the ira is a BRAND, and a brand must be UNFORGEABLE

The ira is not a secret you hide; it is the permanent device identity the fleet key is wrapped to (see photon/docs/fleet-key.md).
So the property that matters is not confidentiality (the ira is public by construction) but *unforgeability*: an attacker must not be able to stand up a device — real or emulated — that derives an ira the fleet already trusts.
If a forgeable ira is registered AND unlocked in the fleet, the attacker becomes an unevictable, permanently-rekeyed member: every SHRINK re-wraps the fleet key to that ira, and you cannot lock it without locking your own device, because you cannot distinguish the brands.
That is why serial-only keying is not enough: a structured serial is settable in emulation and readable with brief possession.
The ira secret must root in material that is high-entropy AND physically bound AND not attacker-settable — the PUF-class sources below — with the vendor-issued identifiers layered on top for breadth and collision-resistance.

## Two layers

- **Unforgeable physical core** — PUF-class, bound to this die, needs fuzzy extraction (secure sketch + ECC, public helper data) because it is noisy. This is the root of forgery-resistance.
- **Vendor-issued uniqueness** — chip serial, UFS serial, IMEI, MACs, OTP. Knowable and technically forgeable, but Google guards extraction, so it raises the attacker's dossier cost. Breadth, never a sole root.

## Tiering (so a drifting bit never bricks the vault)

- **Tier 0 — key today, deterministic:** chip `unique_id`, UFS `iSerialNumber`. Stable, forgeable-but-breadth.
- **Tier 1 — graduate via the two-boot + temperature-cycle probe:** the chipid trim family (`ap_hw_tune`, `asv_tbl`, `hpm_asv`, `dvfs_version`). Stable fuses, but must be proven populated + bit-stable before keying; quantize/bin to shed noisy LSBs.
- **Tier 2 — fuzzy-extracted, the real forgery-resistance:** UFS bad-block signature, camera dark-frame/hot-pixel set, SRAM power-on PUF, MCT clock-ratio ring-osc. Secure sketch + BCH/Reed-Muller, public helper data in the vault, rank-based encoding for temperature.

Final: `ira_secret = derive("ferros.ira.husky.vN", tier0_serials ‖ extracted_puf_core ‖ qualified_trims)`.
No un-probed bit contributes. Helper data is public but each source's entropy is budgeted so helper leakage stays bounded.

## Grounded sources

### Reachable NOW (direct MMIO / the UFS link we already hold)

**chipid block — base G#1000_0000, size G#D000** (`zuma.dtsi` chipid@10000000; `gs-chipid.c`). All reads are `readl`/`readb` at base+offset, little-endian.

| Field | Offset | Width | Per-die? | Notes |
|---|---|---|---|---|
| `product_id` | G#0 | 4 B | no | SoC model (ZUMA = G#0986_5000). Model-constant. |
| `unique_id0` | G#4 | 4 B | YES | die serial low; lot_id = low 21 bits. Tier 0. |
| `unique_id1` | G#8 | 4 B | YES | die serial high. Tier 0. |
| `revision` | G#10 | 4 B | no | main/sub/pkg rev bits. Model/stepping-constant. |
| `dvfs_version` | G#900C | 1 B | YES | per-die speed bin. Tier 1. |
| `asv_tbl` | G#9000 | 64 B | YES | per-die leakage/voltage trim. Tier 1. |
| `hpm_asv` | G#A000 | 64 B | YES | per-die HPM/leakage data. Tier 1. |
| `ap_hw_tune` | G#C300 | 32 B | YES | un-quantized per-die trim. Tier 1, already stubbed in ira.rs. |

Today ira.rs keys on `unique_id` alone; `dvfs_version`, `asv_tbl`, `hpm_asv` are free, stable, per-die material sitting right beside it — all should be read by the probe.

**UFS — HCI base G#1320_0000** (`zuma-ufs.dtsi`; `ufs-pixel.c`). Via the query UPIU flow the link already runs:
- Device Descriptor (IDN G#0) `iSerialNumber` (~offset G#18) → per-unit serial. Tier 0.
- Health Descriptor (IDN wear/PE-cycle) and Geometry Descriptor (total-vs-usable block delta) → grown-defect signal.
- **Factory bad-block map is the Tier-2 prize** (physical, per-die, stable, flash cousin of hot pixels) but the stock tree exposes no standard descriptor for the manufacturer defect list — it needs a Samsung/SK-Hynix vendor query (attr range ~G#14–G#3F) or RPMB, i.e. vendor docs. Flagged, not yet grounded.

**MCT free-running counter — base G#100D_0000, CNT_L G#110 / CNT_U G#114** (`exynos_mct_v3.c`). Timebase for a clock-ratio ring-osc PUF: gate it against a CMU-divided PLL over a fixed window, the count ratio is per-die. CMU bases: TOP G#2604_0000, MIF G#27C0_0000, CPUCL0 G#29C0_0000, APM G#1540_0000. Tier 2.

### Blocked / walled off

- **TMU TRIMINFO** (per-die thermal cal fuses, TMU_TOP G#100A_0000 / TMU_SUB G#100B_0000, TRIMINFO at +G#10+p·4) — access on zuma is mediated by **ACPM IPC**, not direct AP MMIO. Not a bare-metal win without bringing up the ACPM mailbox path. Revisit if/when we have ACPM.
- **DRAM PHY training values** — not EL2-mapped after secure boot; live in the DMC / behind SMC. Not present in the tree as readable offsets. Drop.

### Needs its subsystem up first (later tiers of breadth)

- **Display panel ID / DDIC** — DSI DCS reads G#A1 (7 B panel id) and G#D6 (5 B DDIC id, unlock G#F0 G#5A G#5A), plus per-panel gamma/mura in DDIC OTP (`panel-samsung-drv.c`). Needs display up.
- **Cirrus CS35L41 smart-amp** — per-unit `CAL_R` (speaker impedance) and `DIAG_F0` (resonant freq) in DSP cal memory over SPI (`gs101-*-audio.dtsi`, husky cs35l41 cal blobs). Physical, per-unit; access is DSP-mediated.
- **Camera module OTP** (serial + AWB/LSC/AF/PDAF) and **fuel-gauge ROM id** — sensor/fuel-gauge drivers are NOT in this reference tree (proprietary/binary). Present on the device, but no grounded read path here. Fuel gauge is replaceable → breadth only, never stability-critical.
- **IMEI + modem RF calibration** — per-unit, in modem NV; falls out of radio bring-up. Google-issued = the "opacity is a plus" case.

## Next step

Extend the `entry_vault` measure-before-keying instrumentation (already reports `ap_hw_tune` + a wairua sample to ramoops) into a multi-source probe: read every reachable-now candidate — chipid trim family, UFS serial + geometry delta, MCT counter windows — dump across two boots and a temperature swing, and score each source on `entropy x per-bit stability x unforgeability`.
That ranking decides what graduates from Tier 1/2 into `derive_ira`. Measured, not guessed.
