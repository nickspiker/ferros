# The DPU's clocks and sysregs as ABL leaves them (cold boot, 2026-11-03)

Read with `bridge read` from a cold ferros on slot a, with the three DPU domains on (PMU CONFIGURATION and STATUS both 1 for DPUB G#1546_1D00, DPUF0 1D80, DPUF1 1E00). The CAL engine restores these words after each power-on from what it saved at power-off; no source holds the values, so this capture is the table ferros restores (DISPLAY-DESIGN.md §8, layer 1).

| file | base | length |
|---|---|---|
| cmu_dpub.bin | G#1940_0000 | G#3100 |
| cmu_dpuf0.bin | G#1980_0000 | G#3100 |
| cmu_dpuf1.bin | G#19C0_0000 | G#3100 |
| sysreg_dpuf0.bin | G#1982_0000 | G#200 |
| sysreg_dpuf1.bin | G#19C2_0000 | G#200 |
| sysreg_dpub.bin | G#1942_0000 | G#1100 (the DSIM sysreg at +G#1000) |
| cmu_top_1000.bin | G#2604_1000 | G#1000 (DPU muxes at +G#50-5C, dividers +G#848-854) |
| cmu_top_2000.bin | G#2604_2000 | G#100 (DPU gates at +G#78-84) |

The words restored: CMU_DPUB +G#1800 = 1, +G#600 = G#10, +G#610 = G#10, QCH +G#3060-30F0, +G#800 = G#F11F_F03F; CMU_DPUF0/1 +G#600 = G#10, QCH +G#3044-30BC, +G#800 = G#F11F_F03F; sysreg DPUF0/1 +G#104 = 0, +G#108 = 0.
