//! Structured event types — the payload section of ledger entries.
//!
//! Every production payload is a typed event. No free-form strings. Debug-only string events exist for dev builds (stripped in production).

use crate::category::Category;

/// A ledger event — the payload that gets posted.
///
/// Each variant maps to a specific category. The ledger daemon routes events to the correct chain based on the variant.
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

    // ---- USB::PT ----
    /// SPEC received — new inbound transfer.
    PtSpecRx {
        sid: u8,
        count: u64,
        total: u64,
    },
    /// SPEC sent — new outbound transfer.
    PtSpecTx {
        sid: u8,
        count: u64,
        total: u64,
    },
    /// DATA chunk received.
    PtDataRx {
        sid: u8,
        seq: u64,
        len: u16,
    },
    /// DATA chunk sent.
    PtDataTx {
        sid: u8,
        seq: u64,
        len: u16,
    },
    /// Transfer completed (inbound or outbound).
    PtComplete {
        sid: u8,
        success: bool,
        total: u64,
    },
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

            Self::PtSpecRx { .. }
            | Self::PtSpecTx { .. }
            | Self::PtDataRx { .. }
            | Self::PtDataTx { .. }
            | Self::PtComplete { .. } => Category::UsbPt,
        }
    }

    /// Encode this event's payload into `buf`. Returns bytes written.
    ///
    /// Format: event_tag (1 byte) + field values (EWE-encoded). No free-form strings — every field is typed.
    pub fn encode(&self, buf: &mut [u8]) -> usize {
        let mut pos = 0;

        match self {
            Self::BootStarted {
                el,
                sctlr,
                dtb_addr,
            } => {
                buf[pos] = 0x01;
                pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *el as u64);
                pos += crate::ewe::encode_u64(&mut buf[pos..], *sctlr as u64);
                pos += crate::ewe::encode_u64(&mut buf[pos..], *dtb_addr);
            }
            Self::BootDtbParsed { total_size } => {
                buf[pos] = 0x02;
                pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *total_size as u64);
            }
            Self::BootUartProbed { ok } => {
                buf[pos] = 0x03;
                pos += 1;
                buf[pos] = if *ok { 1 } else { 0 };
                pos += 1;
            }
            Self::BootPstoreFound { base, size } => {
                buf[pos] = 0x04;
                pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *base);
                pos += crate::ewe::encode_u64(&mut buf[pos..], *size as u64);
            }
            Self::BootDpuProbed { hw_ver, ok } => {
                buf[pos] = 0x05;
                pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *hw_ver as u64);
                buf[pos] = if *ok { 1 } else { 0 };
                pos += 1;
            }
            Self::BootUsbInitStarted => {
                buf[pos] = 0x06;
                pos += 1;
            }
            Self::BootUsbInitDone { ok } => {
                buf[pos] = 0x07;
                pos += 1;
                buf[pos] = if *ok { 1 } else { 0 };
                pos += 1;
            }
            Self::BootHalted => {
                buf[pos] = 0x08;
                pos += 1;
            }

            Self::UsbReset => {
                buf[pos] = 0x20;
                pos += 1;
            }
            Self::UsbConnectDone { speed } => {
                buf[pos] = 0x21;
                pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *speed as u64);
            }
            Self::UsbDisconnect => {
                buf[pos] = 0x22;
                pos += 1;
            }
            Self::UsbBulkRx { len } => {
                buf[pos] = 0x23;
                pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *len as u64);
            }
            Self::UsbBulkTxComplete => {
                buf[pos] = 0x24;
                pos += 1;
            }

            Self::UsbSetupPacket {
                bm_request_type,
                b_request,
                w_value,
                w_index,
                w_length,
            } => {
                buf[pos] = 0x30;
                pos += 1;
                buf[pos] = *bm_request_type;
                pos += 1;
                buf[pos] = *b_request;
                pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *w_value as u64);
                pos += crate::ewe::encode_u64(&mut buf[pos..], *w_index as u64);
                pos += crate::ewe::encode_u64(&mut buf[pos..], *w_length as u64);
            }
            Self::UsbSetConfiguration { config } => {
                buf[pos] = 0x31;
                pos += 1;
                buf[pos] = *config;
                pos += 1;
            }
            Self::UsbSetupStall => {
                buf[pos] = 0x32;
                pos += 1;
            }

            Self::PtSpecRx { sid, count, total } => {
                buf[pos] = 0x40;
                pos += 1;
                buf[pos] = *sid;
                pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *count);
                pos += crate::ewe::encode_u64(&mut buf[pos..], *total);
            }
            Self::PtSpecTx { sid, count, total } => {
                buf[pos] = 0x41;
                pos += 1;
                buf[pos] = *sid;
                pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *count);
                pos += crate::ewe::encode_u64(&mut buf[pos..], *total);
            }
            Self::PtDataRx { sid, seq, len } => {
                buf[pos] = 0x42;
                pos += 1;
                buf[pos] = *sid;
                pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *seq);
                pos += crate::ewe::encode_u64(&mut buf[pos..], *len as u64);
            }
            Self::PtDataTx { sid, seq, len } => {
                buf[pos] = 0x43;
                pos += 1;
                buf[pos] = *sid;
                pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *seq);
                pos += crate::ewe::encode_u64(&mut buf[pos..], *len as u64);
            }
            Self::PtComplete {
                sid,
                success,
                total,
            } => {
                buf[pos] = 0x44;
                pos += 1;
                buf[pos] = *sid;
                pos += 1;
                buf[pos] = if *success { 1 } else { 0 };
                pos += 1;
                pos += crate::ewe::encode_u64(&mut buf[pos..], *total);
            }
        }

        pos
    }
}
