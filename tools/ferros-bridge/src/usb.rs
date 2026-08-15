use nusb::transfer::{RequestBuffer, TransferError};

// VID 0x1209 (pid.codes), PID 0x4665 (ferros, requested via pid.codes PR #1208 2026-05-13, pending merge). Same PID used by every ferros-shipped USB device; the VSF document's "PIPE message" section disambiguates protocol/role, so PID-level multiplexing isn't needed.
const FERROS_VID: u16 = 0x1209;
const FERROS_PID: u16 = 0x4665;
const INTERFACE: u8 = 0;

pub struct UsbLink {
    interface: nusb::Interface,
    ep_out: u8,
    ep_in: u8,
}

impl UsbLink {
    /// Find and open the ferros device. Returns None if not found.
    pub fn open() -> Result<Self, String> {
        let device = nusb::list_devices()
            .map_err(|e| format!("USB enumeration failed: {e}"))?
            .find(|d| d.vendor_id() == FERROS_VID && d.product_id() == FERROS_PID)
            .ok_or_else(|| {
                format!("No device found with VID={FERROS_VID:04x} PID={FERROS_PID:04x}")
            })?;

        let device = device
            .open()
            .map_err(|e| format!("Failed to open device: {e}"))?;
        let interface = device
            .claim_interface(INTERFACE)
            .map_err(|e| format!("Failed to claim interface {INTERFACE}: {e}"))?;

        // Find bulk endpoints from the active config
        let config = device
            .active_configuration()
            .map_err(|e| format!("Failed to read config: {e}"))?;

        let mut ep_out = None;
        let mut ep_in = None;

        for iface in config.interfaces() {
            for alt in iface.alt_settings() {
                if alt.interface_number() == INTERFACE {
                    for ep in alt.endpoints() {
                        use nusb::transfer::Direction;
                        match (ep.direction(), ep.transfer_type()) {
                            (Direction::Out, nusb::transfer::EndpointType::Bulk) => {
                                ep_out = Some(ep.address());
                            }
                            (Direction::In, nusb::transfer::EndpointType::Bulk) => {
                                ep_in = Some(ep.address());
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        let ep_out = ep_out.ok_or("No bulk OUT endpoint found")?;
        let ep_in = ep_in.ok_or("No bulk IN endpoint found")?;

        eprintln!(
            "Connected: VID={FERROS_VID:04x} PID={FERROS_PID:04x} EP_OUT=0x{ep_out:02x} EP_IN=0x{ep_in:02x}"
        );

        Ok(UsbLink {
            interface,
            ep_out,
            ep_in,
        })
    }

    /// Send data via bulk OUT. Always pads to 512 bytes (USB HS bulk MPS) so DWC3's TRB ring never sees short packets that break the chain.
    pub async fn send(&self, data: &[u8]) -> Result<(), TransferError> {
        let mut padded = vec![0u8; 512];
        let len = data.len().min(512);
        padded[..len].copy_from_slice(&data[..len]);
        let completion = self.interface.bulk_out(self.ep_out, padded).await;
        completion.status
    }

    /// Receive data via bulk IN (up to 512 bytes).
    pub async fn recv(&self) -> Result<Vec<u8>, TransferError> {
        let completion = self
            .interface
            .bulk_in(self.ep_in, RequestBuffer::new(512))
            .await;
        completion.status.map(|_| completion.data)
    }

    /// Receive data via bulk IN with a timeout.
    pub async fn recv_timeout(&self, timeout: std::time::Duration) -> Result<Vec<u8>, String> {
        tokio::select! {
            completion = self.interface.bulk_in(self.ep_in, RequestBuffer::new(512)) => {
                completion.status
                    .map(|_| completion.data)
                    .map_err(|e| format!("USB read error: {e}"))
            }
            _ = tokio::time::sleep(timeout) => {
                Err("timeout".to_string())
            }
        }
    }

    /// Read the kernel's vendor debug counter block (VENDOR_REQ_DBG G#5A): 16 LE u32 counters served over EP0, independent of the bulk path.
    pub async fn read_dbg(&self) -> Result<[u32; 16], String> {
        use nusb::transfer::{ControlIn, ControlType, Recipient};
        let completion = self.interface.control_in(ControlIn {
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

    /// Read the raw event-path debug block (VENDOR_REQ_DBG2 G#5B): 12 event words + count + TRB snapshots + last DEPCMD failure.
    pub async fn read_dbg2(&self) -> Result<[u32; 16], String> {
        use nusb::transfer::{ControlIn, ControlType, Recipient};
        let completion = self.interface.control_in(ControlIn {
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

    #[allow(dead_code)]
    pub fn ep_out(&self) -> u8 { self.ep_out }
    #[allow(dead_code)]
    pub fn ep_in(&self) -> u8 { self.ep_in }
}
