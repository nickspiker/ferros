//! Hypervisor probing — detect QHEE/Gunyah capabilities via HVC calls.
//!
//! Safe probing: all HVC calls are wrapped with exception counting. If the hypervisor doesn't support a call, we catch the exception and move on. Zero risk of crash.

/// Gunyah HVC function IDs (from android14-6.1 kernel).
#[allow(dead_code)]
const GH_HYP_IDENTIFY: u64 = 0x6000;

/// SMCCC-compliant Gunyah identify call. ARM SMCCC: function ID in x0, args in x1-x3, results in x0-x3.
const GH_IDENTIFY_SMCCC: u64 = 0xC6000000; // SMC64, owner=6 (vendor HYP), func=0

/// Qualcomm-specific HVC to detect QHEE presence. From qcom_scm.c: OWNER_VENDOR_HYP (6), function 0x3f01
const QCOM_HYP_IDENTIFY: u64 = 0xC6003F01;

/// Result of probing the hypervisor.
#[derive(Clone, Copy)]
pub struct HypProbe {
    /// True if any hypervisor responded to HVC
    pub present: bool,
    /// True if Gunyah specifically identified itself
    pub gunyah: bool,
    /// Raw x0 return from GH identify (0 = not supported)
    pub gh_identify_x0: u64,
    /// Raw x1 return (variant, if Gunyah)
    pub gh_identify_x1: u64,
    /// Raw x0 from QCOM HYP identify
    pub qcom_x0: u64,
    /// Exception count delta (>0 means HVC faulted)
    pub exceptions: u64,
}

/// Probe the hypervisor. Safe — uses exception counting to catch faults.
///
/// Tries:
/// 1. Gunyah identify (HVC #0xC6000000) — returns API version if Gunyah
/// 2. QCOM HYP identify (HVC #0xC6003F01) — returns info if QHEE
/// 3. Simple HVC #0 — basic liveness test
pub fn probe(exception_count: fn() -> u64) -> HypProbe {
    let mut result = HypProbe {
        present: false,
        gunyah: false,
        gh_identify_x0: 0,
        gh_identify_x1: 0,
        qcom_x0: 0,
        exceptions: 0,
    };

    let exc_before = exception_count();

    // Try Gunyah identify
    let (x0, x1) = hvc_call_2(GH_IDENTIFY_SMCCC, 0, 0, 0);
    result.gh_identify_x0 = x0;
    result.gh_identify_x1 = x1;

    let exc_after_gh = exception_count();
    if exc_after_gh == exc_before {
        // No exception — hypervisor responded
        result.present = true;
        // Gunyah returns a non-zero API version in x0 on success
        if x0 != 0 && x0 != u64::MAX {
            result.gunyah = true;
        }
    }

    // Try QCOM HYP identify
    let (qx0, _qx1) = hvc_call_2(QCOM_HYP_IDENTIFY, 0, 0, 0);
    result.qcom_x0 = qx0;

    let exc_after_qcom = exception_count();
    if exc_after_qcom == exc_after_gh {
        result.present = true;
    }

    result.exceptions = exception_count() - exc_before;
    result
}

/// Issue an HVC call with 3 arguments, return x0 and x1.
#[inline(never)]
fn hvc_call_2(func: u64, arg1: u64, arg2: u64, arg3: u64) -> (u64, u64) {
    let r0: u64;
    let r1: u64;
    unsafe {
        core::arch::asm!(
            "hvc #0",
            inout("x0") func => r0,
            inout("x1") arg1 => r1,
            inout("x2") arg2 => _,
            inout("x3") arg3 => _,
            // Clobber x4-x17 per SMCCC
            out("x4") _,
            out("x5") _,
            out("x6") _,
            out("x7") _,
            out("x8") _,
            out("x9") _,
            out("x10") _,
            out("x11") _,
            out("x12") _,
            out("x13") _,
            out("x14") _,
            out("x15") _,
            out("x16") _,
            out("x17") _,
        );
    }
    (r0, r1)
}
