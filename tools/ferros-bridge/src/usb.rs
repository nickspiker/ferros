//! The link itself is ferros-link's (tools/ferros-link: finding the device, `FERROS_BUS`, bulk transfers), shared with ferros-send. Here only the bridge's development vendor requests on EP0: the kernel's debug blocks and the answer to the "Ready?" gate, which no person's tool has.

pub use ferros_link::usb::UsbLink;

/// Read the kernel's vendor debug counter block (VENDOR_REQ_DBG G#5A): 16 LE u32 counters served over EP0, independent of the bulk path.
pub async fn read_dbg(link: &UsbLink) -> Result<[u32; 16], String> {
    use nusb::transfer::{ControlIn, ControlType, Recipient};
    let completion = link.interface().control_in(ControlIn {
        control_type: ControlType::Vendor,
        recipient: Recipient::Device,
        request: 0x5A,
        value: 0,
        index: 0,
        length: 64,
    }).await;
    completion.status.map_err(|e| format!("debug control_in failed: {e}"))?;
    let data = completion.data;
    if data.len() < 64 {
        return Err(format!("short debug block: {} bytes", data.len()));
    }
    let mut out = [0u32; 16];
    for i in 0..16 {
        out[i] = u32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    }
    Ok(out)
}

/// Answer the "Ready?" gate from the host (VENDOR_REQ_GATE G#5C, no data): 1 = ferros, 2 = Android, 3 = hold. Dev builds have USB up at the gate for this.
pub async fn gate(link: &UsbLink, value: u16) -> Result<(), String> {
    use nusb::transfer::{ControlOut, ControlType, Recipient};
    let completion = link.interface().control_out(ControlOut {
        control_type: ControlType::Vendor,
        recipient: Recipient::Device,
        request: 0x5C,
        value,
        index: 0,
        data: &[],
    }).await;
    completion.status.map_err(|e| format!("gate control_out failed: {e}"))
}

/// Read the raw event-path debug block (VENDOR_REQ_DBG2 G#5B): 12 event words + count + TRB snapshots + last DEPCMD failure.
pub async fn read_dbg2(link: &UsbLink) -> Result<[u32; 16], String> {
    use nusb::transfer::{ControlIn, ControlType, Recipient};
    let completion = link.interface().control_in(ControlIn {
        control_type: ControlType::Vendor,
        recipient: Recipient::Device,
        request: 0x5B,
        value: 0,
        index: 0,
        length: 64,
    }).await;
    completion.status.map_err(|e| format!("dbg2 control_in failed: {e}"))?;
    let data = completion.data;
    if data.len() < 64 {
        return Err(format!("short dbg2 block: {} bytes", data.len()));
    }
    let mut out = [0u32; 16];
    for i in 0..16 {
        out[i] = u32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    }
    Ok(out)
}
