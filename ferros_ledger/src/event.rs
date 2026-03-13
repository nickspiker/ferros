//! Structured event types — the payload section of ledger entries.
//!
//! Every production payload is a typed event. No free-form strings.
//! Debug-only string events exist for dev builds (stripped in production).

use crate::category::Category;

/// A ledger event — the payload that gets posted.
///
/// Each variant maps to a specific category. The ledger daemon
/// routes events to the correct chain based on the variant.
#[derive(Clone, Copy, Debug)]
pub enum Event {
    // ---- Kernel::Boot ----
    BootStarted {
        el: u32,
        sctlr: u32,
        dtb_addr: u64,
    },
    BootDtbParsed {
        total_size: u32,
    },
    BootUartProbed {
        ok: bool,
    },
    BootPstoreFound {
        base: u64,
        size: u32,
    },
    BootDpuProbed {
        hw_ver: u32,
        ok: bool,
    },
    BootUsbInitStarted,
    BootUsbInitDone {
        ok: bool,
    },
    BootHalted,

    // ---- USB::DWC3 ----
    UsbReset,
    UsbConnectDone {
        speed: u32,
    },
    UsbDisconnect,
    UsbBulkRx {
        len: u32,
    },
    UsbBulkTxComplete,

    // ---- USB::Enumeration ----
    UsbSetupPacket {
        bm_request_type: u8,
        b_request: u8,
        w_value: u16,
        w_index: u16,
        w_length: u16,
    },
    UsbSetConfiguration {
        config: u8,
    },
    UsbSetupStall,
}

impl Event {
    /// The category this event belongs to.
    pub fn category(&self) -> Category {
        match self {
            Self::BootStarted { .. }
            | Self::BootDtbParsed { .. }
            | Self::BootUartProbed { .. }
            | Self::BootPstoreFound { .. }
            | Self::BootDpuProbed { .. }
            | Self::BootUsbInitStarted
            | Self::BootUsbInitDone { .. }
            | Self::BootHalted => Category::KernelBoot,

            Self::UsbReset
            | Self::UsbConnectDone { .. }
            | Self::UsbDisconnect
            | Self::UsbBulkRx { .. }
            | Self::UsbBulkTxComplete => Category::UsbDwc3,

            Self::UsbSetupPacket { .. }
            | Self::UsbSetConfiguration { .. }
            | Self::UsbSetupStall => Category::UsbEnumeration,
        }
    }

    /// Encode this event's payload into `buf`. Returns bytes written.
    ///
    /// Format: event_tag (1 byte) + field values (EWE-encoded).
    /// No free-form strings — every field is typed.
    pub fn encode(&self, buf: &mut [u8]) -> usize {
        let mut pos = 0;

        match self {
            Self::BootStarted { el, sctlr, dtb_addr } => {
                buf[pos] = 0x01; pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *el as u64);
                pos += crate::ewe::encode_u64(&mut buf[pos..], *sctlr as u64);
                pos += crate::ewe::encode_u64(&mut buf[pos..], *dtb_addr);
            }
            Self::BootDtbParsed { total_size } => {
                buf[pos] = 0x02; pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *total_size as u64);
            }
            Self::BootUartProbed { ok } => {
                buf[pos] = 0x03; pos += 1;
                buf[pos] = if *ok { 1 } else { 0 }; pos += 1;
            }
            Self::BootPstoreFound { base, size } => {
                buf[pos] = 0x04; pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *base);
                pos += crate::ewe::encode_u64(&mut buf[pos..], *size as u64);
            }
            Self::BootDpuProbed { hw_ver, ok } => {
                buf[pos] = 0x05; pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *hw_ver as u64);
                buf[pos] = if *ok { 1 } else { 0 }; pos += 1;
            }
            Self::BootUsbInitStarted => {
                buf[pos] = 0x06; pos += 1;
            }
            Self::BootUsbInitDone { ok } => {
                buf[pos] = 0x07; pos += 1;
                buf[pos] = if *ok { 1 } else { 0 }; pos += 1;
            }
            Self::BootHalted => {
                buf[pos] = 0x08; pos += 1;
            }

            Self::UsbReset => {
                buf[pos] = 0x20; pos += 1;
            }
            Self::UsbConnectDone { speed } => {
                buf[pos] = 0x21; pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *speed as u64);
            }
            Self::UsbDisconnect => {
                buf[pos] = 0x22; pos += 1;
            }
            Self::UsbBulkRx { len } => {
                buf[pos] = 0x23; pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *len as u64);
            }
            Self::UsbBulkTxComplete => {
                buf[pos] = 0x24; pos += 1;
            }

            Self::UsbSetupPacket { bm_request_type, b_request, w_value, w_index, w_length } => {
                buf[pos] = 0x30; pos += 1;
                buf[pos] = *bm_request_type; pos += 1;
                buf[pos] = *b_request; pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *w_value as u64);
                pos += crate::ewe::encode_u64(&mut buf[pos..], *w_index as u64);
                pos += crate::ewe::encode_u64(&mut buf[pos..], *w_length as u64);
            }
            Self::UsbSetConfiguration { config } => {
                buf[pos] = 0x31; pos += 1;
                buf[pos] = *config; pos += 1;
            }
            Self::UsbSetupStall => {
                buf[pos] = 0x32; pos += 1;
            }
        }

        pos
    }
}
