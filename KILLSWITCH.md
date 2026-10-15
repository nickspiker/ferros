# Killswitch Architecture

## Mechanism
Vol Up + Vol Down (held simultaneously) → key-edge interrupt → PSCI SYSTEM_OFF → PMIC power cut.

On husky (since 2026-10-14, `ferros_kernel/src/killswitch.rs`) the key presses ARE the interrupt; nothing polls:
- Both keys are always-on GPIOs that can each raise their own GIC SPI (Exynos wakeup EINT): vol-down = gpa4-1 → SPI 29 (INTID 61), vol-up = gpa6-2 → SPI 32 (INTID 64). They interrupt on both edges.
- `killswitch::arm()` is the first line of `kernel_main` on both boot paths. It powers off at once if both keys are already held. Otherwise it brings up the GIC (distributor group 1, this CPU's redistributor, system-register CPU interface), switches every other interrupt off (ferros polls everything else), routes physical IRQs to EL2 (HCR_EL2.IMO) and unmasks.
- The handler (`.Lirq` in main.rs) is a few instructions of assembly with no Rust state. Whatever fired, if both keys read held it issues SYSTEM_OFF on the spot, preempting whatever was running: UFS bring-up, the vault, a RUN payload, a wedged loop. Otherwise it clears the key's pending bit (or switches a stray interrupt off) and returns.
- If SYSTEM_OFF ever returned, it sits with IRQs masked until the unpetted watchdog resets the SoC.
- RELOAD masks IRQs before its copy stub overwrites the vector table; the new image re-arms first thing.
- DIAG: `KILL_GICR_FRAME`, `KILL_GICD_CTLR`, `KILL_KEYS_IDLE` (both keys read released after becoming EINTs), `KILL_IRQS` (non-kill interrupts taken), `DAIF`.

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
3. Between the SMC and rail collapse, only EL3 code executes. EL3 *can* read EL2's
   registers, but the EL3 code we inspected never does: by whole-system logic it cannot,
   within the radius of our disassembly and tracing, unless Google ships a different EL3
   (Google-signed firmware, not ROM).
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
- **NFC dead man's switch**: BLE/NFC tag proximity keepalive. Tag leaves range → kill.
- **PMIC watchdog**: Program S2MPG PMIC to hard-cut power if not periodically petted
  by the kernel. Defense against kernel hang/compromise.
- **Multi-core broadcast**: Current killswitch runs on one core. Broadcast an IPI to
  all cores to halt before the SMC, preventing any core from racing to read keys.

## Hardware Confirmed On
- Google Pixel 8 Pro (husky, Tensor G3 / Zuma), EL2, PSCI via Samsung TF-A
- Kill confirmed: phone powers off immediately.
- Interrupt-driven killswitch confirmed 2026-10-14, cold boot (slot b) and Linux handoff:
  - One key alone = two interrupts (press and release edges), nothing else happens.
  - Both keys = power cut. It was a true rail drop, not a reset: the next boot found the DRAM report block gone, which every warm or watchdog reset preserves.
  - Delivery to EL2 was also proven by software-pending each key's SPI (GICD_ISPENDR): handled, counted, cleared, no exception.
- **With USB power attached the phone powers straight back on:** the BootROM ("Pixel ROM Recovery", 18d1:4f00) enumerated within the same second as the kill. That's the PMIC's power-on-on-VBUS, not the kill failing. Unplugged it stays off until the power button.

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
