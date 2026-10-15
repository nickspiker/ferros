# KOHI-SOURCES — where husky's randomness comes from — Zil (0)

KOHI gathers the session's wairua (GLOSSARY; `ferros_kernel/src/kohi.rs`), every source health-tested the PIPE way (RCT, APT, iterated GF(2) popcount levels) and only healthy ones mixed. The wairua needs 256 good bits per power session; everything downstream (the entropy syscall, nonces, the WiFi firmware's RNG seed) is BLAKE3-derived from it. So rate is not the constraint — **independence is**.

**The single-egg rule (owner):** never one basket. Target: **four independent sources** — no single firmware, vendor block or physical effect can decide the wairua alone. Every other source joins as its hardware comes up anyway (camera, microphone, IMU — Lumis and the rest bring them up regardless).

| Source | Status | Raw rate | Bring-up | Interfacing pain |
|---|---|---|---|---|
| **SoC hardware TRNG** (EL3 SMCCC `TRNG_RND64`) | **live**, healthy | **~200 kbit/s measured** (65,536 bits in 329 ms; the per-SMC cost limits it) | done | low — but trusts EL3 to pass it through raw |
| **CPU timing jitter** (cycle-counter deltas over cache-missing loads) | **live**, healthy | **~4.4 Mbit/s raw measured** (65,536 in 15 ms); min-entropy per bit well under 1, needs an SP 800-90B estimate | done | low |
| **Cross-oscillator jitter** (generic timer vs the always-on 32 kHz clock or the WiFi chip's TSF — separate crystals) | **next** | ~kbit/s (est.) | hours | low |
| **WiFi radio noise** (per-frame RSSI / noise-floor LSBs, `phy_noise`; maybe the BCM4398's own RNG via an iovar) | **next** | hundreds of bit/s from RSSI (est.); dongle RNG unknown | hours | low — the driver already does ioctls; a dongle RNG would be Broadcom firmware, so mixed, never trusted alone |
| Fuel gauge current-sense noise (Maxim gauge, I2C) | later | tens of bit/s (est.) | a day or two | medium (I2C driver process) |
| Ring oscillators (MCT ring-osc, IRA-ENTROPY-SOURCES.md), fine timing | later | kbit/s–Mbit/s (est.) | days | medium (undocumented block) |
| Microphone thermal noise | with audio | ~100–200 kbit/s (est.) | weeks | high (audio runs through the always-on DSP's firmware) |
| IMU (accelerometer / gyro) LSB noise | with sensors | ~kbit/s (est.) | weeks | high (behind the same DSP / context hub) |
| Camera CMOS dark-frame noise (read + shot noise) | with Lumis | hundreds of Mbit/s (est.) | weeks–months | very high alone; free once the camera stack exists |
| CPU `RNDR` | **absent** on these cores (CPU-FEATURES.md) | — | — | — |
| Thermal sensor (TMU) | blocked (IRA-ENTROPY-SOURCES.md) | — | — | — |
| **PIPE** on-die 16-ring TRNG | future | design target | own silicon | the source that needs nobody's firmware |

Measured rows: `bridge diag` KOHI_* (2026-10-15: TRNG ones 32,902 / 65,536, jitter 33,068 / 65,536, all tests passed). Estimated rows are estimates until measured the same way.
