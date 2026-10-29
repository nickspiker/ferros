//! Fingerprint-sensor bus hunt (SENSORS.md §Fingerprint, FINGERPRINT.md).
//!
//! The Goodix under-display sensor's normal-world driver (gf_spi.c) does no SPI at all: it binds as a platform_driver, only toggles a reset (gpp1-0) and forwards an IRQ (gpa7-3) over netlink, and carries zero register access. Capture and matching live in the Trusty TA, and the sensor's SPI controller and its data pins are left out of the normal-world device tree entirely - no SPI pin group anywhere uses gpp0 or gpp1, so the sensor node exposes only its reset and its interrupt. Which SPI controller the secure world drives it from cannot be read off the tree.
//!
//! So this scans every USI the normal world can name across PERIC0, PERIC1 and HSI0, reads each one's personality (UART/SPI/I2C) from its sysreg SW_CONF field, and flags any controller in SPI mode that is not one of the known devices (camera I2C, the eSE, UWB, the amps, touch, GNSS). The odd one out - an SPI controller nobody in the normal world claims - is the fingerprint's. This only pays off under the handoff build with Trusty and Android live, where the secure world has configured the sensor's USI; a cold boot leaves every PERIC0 USI at reset (SW_CONF 0), nothing to see.
//!
//! Read-only. The SW_CONF fields live in the PERIC/HSI0 syscons the normal world owns (always clocked, read cleanly). A controller's own SPI block (G#108x/10Cx/111x_0000) is only read when its SW_CONF is non-zero, since an idle USI may be clock-gated and a read of a gated block can hang; a configured one is clocked. Every read goes through a fault-safe probe: on the handoff build the synchronous-abort fault sink is terminal by design (main.rs .Lexc_recover warm-resets to preserve the report), so it is disarmed for the window of each read and restored after - a secure-locked read then skip-and-counts and probe32 reports it as a fault instead of resetting the phone.

use crate::platform::append_hex;
use alloc::vec::Vec;

const SYSREG_PERIC0: usize = 0x1082_1000;
const SYSREG_PERIC1: usize = 0x10C2_1000;
const SYSREG_HSI0: usize = 0x1102_0000;
/// The earlycon console (PERIC0 USI0): a known normal-world MMIO block, the control that proves a read can succeed.
const USI0_UART: usize = 0x1087_0000;

// s3c64xx SPI register offsets (same IP as spi.rs).
const CH_CFG: usize = 0x00;
const MODE_CFG: usize = 0x08;
const STATUS: usize = 0x14;

/// Every USI the normal world can name: (label, SW_CONF address, SPI block base, owner). "?" marks a controller no known device claims - a fingerprint candidate. spi_1..4/14 are the camera's I2C (camera.rs), not SPI. APM-side spi_17 is left out: reading the always-on power domain cold is not worth the risk.
const USIS: &[(&str, usize, usize, &str)] = &[
    ("P0 spi_1 ", SYSREG_PERIC0 + 0x00, 0x1088_0000, "camera I2C"),
    ("P0 spi_2 ", SYSREG_PERIC0 + 0x04, 0x1089_0000, "camera I2C"),
    ("P0 spi_3 ", SYSREG_PERIC0 + 0x08, 0x108A_0000, "camera I2C"),
    ("P0 spi_4 ", SYSREG_PERIC0 + 0x0C, 0x108B_0000, "camera I2C"),
    ("P0 spi_5 ", SYSREG_PERIC0 + 0x10, 0x108C_0000, "?"),
    ("P0 spi_6 ", SYSREG_PERIC0 + 0x14, 0x108D_0000, "?"),
    ("P0 spi_14", SYSREG_PERIC0 + 0x28, 0x1098_0000, "camera PMIC I2C"),
    ("P1 spi_0 ", SYSREG_PERIC1 + 0x00, 0x10C8_0000, "?"),
    ("P1 spi_9 ", SYSREG_PERIC1 + 0x04, 0x10C9_0000, "?"),
    ("P1 spi_10", SYSREG_PERIC1 + 0x08, 0x10CA_0000, "?"),
    ("P1 spi_11", SYSREG_PERIC1 + 0x0C, 0x10CB_0000, "?"),
    ("P1 spi_12", SYSREG_PERIC1 + 0x10, 0x10CC_0000, "eSE"),
    ("P1 spi_13", SYSREG_PERIC1 + 0x14, 0x10CD_0000, "?"),
    ("P1 spi_15", SYSREG_PERIC1 + 0x18, 0x10CE_0000, "?"),
    ("H0 spi_18", SYSREG_HSI0 + 0x1014, 0x111B_0000, "UWB"),
    ("H0 spi_19", SYSREG_HSI0 + 0x1018, 0x111C_0000, "amps"),
    ("H0 spi_20", SYSREG_HSI0 + 0x101C, 0x111D_0000, "touch"),
    ("H0 spi_21", SYSREG_HSI0 + 0x1020, 0x111E_0000, "GNSS"),
    ("H0 spi_22", SYSREG_HSI0 + 0x1024, 0x111F_0000, "?"),
];

/// USI SW_CONF personality (spi-s3c64xx.c USI_SW_CONF_MASK = 0x7: bit0 UART, bit1 SPI, bit2 I2C).
fn mode_name(sw: u32) -> &'static str {
    match sw & 0x7 {
        0x1 => "UART",
        0x2 => "SPI",
        0x4 => "I2C",
        0 => "reset/none",
        _ => "mixed?",
    }
}

/// A fault-safe read that is also safe on the handoff build: the terminal sync-abort sink is disarmed for the read so a secure-locked access skip-and-counts (main.rs .Lexc_recover) and comes back as None, not a warm reset. SError external aborts are counted regardless of the sink.
fn probe(a: usize) -> Option<u32> {
    let saved = unsafe { core::ptr::read_volatile(&raw const crate::__exc_sink) };
    unsafe { core::ptr::write_volatile(&raw mut crate::__exc_sink, 0) };
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    let r = crate::aoc::probe32(a);
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    unsafe { core::ptr::write_volatile(&raw mut crate::__exc_sink, saved) };
    r
}

/// `diag fp`: find the fingerprint's SPI controller by elimination - the one SPI-mode USI no known device owns.
pub fn report(out: &mut Vec<u8>) {
    // Control: a known normal-world PERIC0 block must read, or the probe path itself is what is broken.
    out.extend_from_slice(b"control USI0-UART[0] (console)=");
    match probe(USI0_UART) {
        Some(_) => out.extend_from_slice(b"read\n"),
        None => {
            out.extend_from_slice(b"FAULTED - probe path suspect, scan below is unreliable\n");
            return;
        }
    }

    out.extend_from_slice(b"USI personality scan (sysreg SW_CONF):\n");
    let mut candidates = 0u32;
    let mut cold = true;
    for &(name, swconf, base, owner) in USIS {
        let sw = probe(swconf);
        out.extend_from_slice(b"  ");
        out.extend_from_slice(name.as_bytes());
        out.push(b' ');
        match sw {
            None => out.extend_from_slice(b"SW_CONF FAULTED"),
            Some(v) => {
                if v & 0x7 != 0 {
                    cold = false;
                }
                out.extend_from_slice(mode_name(v).as_bytes());
                out.extend_from_slice(b" [");
                out.extend_from_slice(owner.as_bytes());
                out.push(b']');
                // A configured USI is clocked; read its SPI block for signs of use. Skip idle ones (a gated block can hang).
                if v & 0x7 != 0 {
                    if let Some(ch) = probe(base + CH_CFG) {
                        append_hex(out, b" CH_CFG=G#", ch);
                        let last = out.len() - 1;
                        out[last] = b' ';
                        append_hex(out, b"MODE=G#", probe(base + MODE_CFG).unwrap_or(0));
                        let last = out.len() - 1;
                        out[last] = b' ';
                        append_hex(out, b"STATUS=G#", probe(base + STATUS).unwrap_or(0));
                        let last = out.len() - 1;
                        out[last] = b' ';
                    }
                }
                // The find: an unclaimed controller the secure world switched to SPI.
                if v & 0x7 == 0x2 && owner == "?" {
                    out.extend_from_slice(b"  <== CANDIDATE");
                    candidates += 1;
                }
            }
        }
        out.push(b'\n');
    }

    out.extend_from_slice(b"VERDICT: ");
    if candidates == 1 {
        out.extend_from_slice(b"one unclaimed SPI controller - likely the fingerprint's bus (see CANDIDATE above); next is to drive it (new spi.rs Port) and see if the Goodix chip answers\n");
    } else if candidates > 1 {
        out.extend_from_slice(b"more than one unclaimed SPI controller in SPI mode - narrow by reset/IRQ proximity or a flex trace before driving one\n");
    } else if cold {
        out.extend_from_slice(b"every USI reads reset/none - cold boot, no secure world has configured the sensor; re-run under the handoff build with Trusty+Android live\n");
    } else {
        out.extend_from_slice(b"no unclaimed SPI controller is visible - the fingerprint's USI is either idle right now or secure-hidden (its SW_CONF reads 0 from NS-EL2); a logic analyser on the flex is then the only way to find the bus\n");
    }
}

/// Write the USI personality scan into the ramoops report block for the handoff path, where USB does not come up to answer `diag fp` interactively (2026-10-29: the handoff watchdog-resets during the USB bring-up). Each USI's SW_CONF (low 3 bits) is packed one byte per USI into r[6..9] (RAMOOPS_RESULT + 0x30/0x38/0x40, the order of the USIS table above, 0xFF = the read faulted). Run early, right after arm_safety, so it survives the watchdog reset; the next Android boot prints it as the "ferros: r[06..08]" dump, decoded against USIS. SW_CONF is a syscon read (always clocked, safe this early); the SPI blocks are not touched here.
pub fn scan_to_report() {
    let mut w = [0u64; 3];
    for (i, &(_, swconf, _, _)) in USIS.iter().enumerate() {
        let v = probe(swconf).map(|x| x & 0x7).unwrap_or(0xFF) as u64;
        w[i / 8] |= v << ((i % 8) * 8);
    }
    unsafe {
        core::ptr::write_volatile((crate::platform::RAMOOPS_RESULT + 0x00) as *mut u64, crate::platform::RESULT_MAGIC);
        core::ptr::write_volatile((crate::platform::RAMOOPS_RESULT + 0x30) as *mut u64, w[0]);
        core::ptr::write_volatile((crate::platform::RAMOOPS_RESULT + 0x38) as *mut u64, w[1]);
        core::ptr::write_volatile((crate::platform::RAMOOPS_RESULT + 0x40) as *mut u64, w[2]);
        ferros_hal::mmio::cache_clean(crate::platform::RAMOOPS_RESULT, 0x180);
    }
}
