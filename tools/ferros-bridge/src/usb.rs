use nusb::transfer::{Queue, RequestBuffer, TransferError};
use std::time::Duration;

const FERROS_VID: u16 = 0x1838;
const FERROS_PID: u16 = 0xFE01;
const INTERFACE: u8 = 0;
const TIMEOUT: Duration = Duration::from_secs(1);

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
            .ok_or_else(|| format!("No device found with VID={FERROS_VID:04x} PID={FERROS_PID:04x}"))?;

        let device = device.open().map_err(|e| format!("Failed to open device: {e}"))?;
        let interface = device
            .claim_interface(INTERFACE)
            .map_err(|e| format!("Failed to claim interface {INTERFACE}: {e}"))?;

        // Find bulk endpoints from the active config
        let config = device.active_configuration()
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

        eprintln!("Connected: VID={FERROS_VID:04x} PID={FERROS_PID:04x} EP_OUT=0x{ep_out:02x} EP_IN=0x{ep_in:02x}");

        Ok(UsbLink { interface, ep_out, ep_in })
    }

    /// Send data via bulk OUT.
    pub async fn send(&self, data: &[u8]) -> Result<(), TransferError> {
        let completion = self.interface.bulk_out(self.ep_out, data.to_vec()).await;
        completion.status
    }

    /// Receive data via bulk IN (up to 512 bytes).
    pub async fn recv(&self) -> Result<Vec<u8>, TransferError> {
        let completion = self.interface
            .bulk_in(self.ep_in, RequestBuffer::new(512))
            .await;
        completion.status.map(|_| completion.data)
    }

    pub fn ep_out(&self) -> u8 { self.ep_out }
    pub fn ep_in(&self) -> u8 { self.ep_in }
}
