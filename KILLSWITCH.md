# Killswitch Architecture

## Mechanism
Vol Up + Vol Down (held simultaneously) → GPIO poll → PSCI SYSTEM_OFF → PMIC power cut.

## Why This Works

### Key Storage: ARM PAC Registers
The master key lives in ARM Pointer Authentication key registers (APIAKey, APIBKey,
APDAKey, APDBKey, APGAKey) — 5 × 128-bit = 640 bits of EL2-only storage.

These are **flip-flop registers on the CPU die**, not DRAM. They are:
- Only accessible from EL2+ (hardware-enforced; EL1/EL0 access traps)
- Powered by the SoC core voltage rail (VCORE)
- Not backed by capacitors — state is maintained by active transistor feedback loops

### Why We Don't Zero Before Power-Off
PSCI SYSTEM_OFF (ARM SMC G#84000008) instructs ARM Trusted Firmware (EL3) to
command the PMIC to cut power. When VCORE drops below the flip-flop retention
voltage (~0.4V on modern FinFET), the register state becomes physically undefined.
There is nothing to read — the bits no longer exist as logical values.

Explicit zeroing (`msr APIAKeyLo_EL1, xzr` etc.) would add ~5 nanoseconds before
the SMC. This is unnecessary because:

1. The SMC trap to EL3 takes ~100ns. During this time, EL2 registers are not
   accessible from any lower EL (hardware-enforced).
2. TF-A's SYSTEM_OFF handler runs at EL3. It commands the PMIC over SPMI/I2C
   (~1-10ms for the PMIC to actually cut rails).
3. Between the SMC and rail collapse, only EL3 code executes. EL3 is ROM-signed
   firmware — no attack surface.
4. Once VCORE drops, flip-flop state is physically destroyed. Not zeroed — destroyed.
   The distinction matters: zeroed memory has a known state (all 0). Destroyed
   flip-flops have no state — the output is determined by thermal noise, process
   variation, and residual charge, and is not recoverable.

### Cold Boot Attack Inapplicability
Cold boot attacks (Halderman et al., 2008) exploit DRAM charge retention after power
loss. DRAM cells are capacitor-based — charge decays over 1-4 seconds at room
temperature, extendable to 30-60 seconds with cooling.

This attack is **inapplicable** to flip-flop register storage because:
- Flip-flops are active circuits (cross-coupled inverters), not passive charge stores
- They have no retention time — state is lost when supply voltage drops below V_retention
- Cooling does not help because the mechanism is voltage loss, not charge decay
- The registers are on-die (not a separate chip) — physical probing requires
  decapping and FIB, which destroys the state being probed

### Attack Surface Analysis
| Attack | Effective? | Why |
|--------|-----------|-----|
| Cold boot (DRAM freeze) | No | Keys are not in DRAM |
| Software read from EL1/EL0 | No | Hardware traps on register access |
| JTAG/debug port | No | Disabled in production fuse config (FUSE LCS=PRODUCTION) |
| Bus probing | No | Registers are internal to CPU core, no external bus |
| Decap + FIB | Destructive | Physical probing destroys the state |
| Glitching SYSTEM_OFF | Maybe | If PMIC command is interrupted, power may not cut. Mitigation: watchdog as backup. |
| Intercepting SMC at EL3 | No | TF-A is ROM-signed, not modifiable |

### Timing
- GPIO poll loop: ~1μs per iteration
- Button detection to SMC: < 10μs
- SMC trap to EL3: ~100ns
- TF-A SYSTEM_OFF → PMIC command: ~1ms
- PMIC rail collapse: ~1-10ms
- **Total button-to-dead: < 15ms**

### What Remains in DRAM After Kill
All vault data in DRAM is encrypted with a key derived from the PAC register master key.
After power cut, DRAM contains only ciphertext. The key to decrypt it was in flip-flops
that no longer hold a defined state. The ciphertext decays naturally as DRAM capacitors
discharge (1-4 seconds at room temperature).

## Future Enhancements
- **GPIO interrupt (GIC)**: Replace polling with edge-triggered interrupt. Near-zero
  CPU usage, same response time. Wake from WFI.
- **NFC dead man's switch**: BLE/NFC tag proximity keepalive. Tag leaves range → kill.
- **PMIC watchdog**: Program S2MPG PMIC to hard-cut power if not periodically petted
  by the kernel. Defense against kernel hang/compromise.
- **Multi-core broadcast**: Current killswitch runs on one core. Broadcast an IPI to
  all cores to halt before the SMC, preventing any core from racing to read keys.

## Hardware Confirmed On
- Google Pixel 8 (Tensor G3 / Zuma), EL2, PSCI via Samsung TF-A
- Kill confirmed: phone powers off immediately, requires manual power-on to restart

## M1 MacBook Air — Killswitch Plan

### Trigger: Power Button (single press)
The M1 has no volume buttons. The power button is the killswitch trigger.
Unlike the Pixel 8 (GPIO poll), the M1 power button event comes thru the **SMC**
(System Management Controller) — an RTKit coprocessor at `/arm-io/smc` in the ADT.

### SMC Architecture
- RTKit firmware running on a dedicated coprocessor (not the AP)
- Communicates via mailbox protocol (Apple ASC mailbox at ADT-specified address)
- Power button generates an SMC event readable via the mailbox
- Asahi Linux has a working driver (`macsmc` in the Linux kernel)

### Kill Mechanism
Same as Pixel 8: PSCI SYSTEM_OFF (ARM SMC G#84000008).
m1n1 runs at EL2 and PSCI calls trap to Apple's SecureROM (EL3 equivalent).
SecureROM commands the PMU to cut power rails.

### Key Differences from Pixel 8
| Aspect | Pixel 8 | M1 MacBook |
|--------|---------|------------|
| Trigger | Vol Up + Vol Down (GPIO) | Power button (SMC) |
| Detection | GPIO register poll (~1μs) | SMC mailbox read (~10μs) |
| PAC registers | ARMv9 PAC (same) | ARMv8.3 PAC (same API, same registers) |
| PSCI | Samsung TF-A | Apple SecureROM |
| Power cut | S2MPG PMIC via SPMI | Apple PMU via SMC |
| Touch ID | N/A | SEP-owned, never accessible from AP |

### Implementation Steps (after USB/PT transport works)
1. Read SMC base address from ADT (`/arm-io/smc`)
2. Init RTKit mailbox (same protocol as DCP, simpler — no display state)
3. Register for power button events
4. On event: PSCI SYSTEM_OFF (identical to Pixel 8 kill path)

### Key Storage on M1
Identical to Pixel 8. M1's Icestorm/Firestorm cores implement ARMv8.3-PAuth.
PAC key registers (APIAKey, APIBKey, APDAKey, APDBKey, APGAKey) are EL2-only,
flip-flop-based, destroyed on power loss. Same security properties apply.

### Estimated Timing (M1)
- SMC event detection: ~10μs (mailbox read vs GPIO poll)
- SMC trap to EL3: ~100ns (same ARM mechanism)
- PMU power cut: ~1-10ms
- **Total button-to-dead: < 15ms** (same as Pixel 8)
