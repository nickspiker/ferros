//! The USB link to a ferros device (docs/USB.md, docs/BRIDGE.md): found by its IDs (`FERROS_BUS` picks one of several), its bulk endpoints claimed, packets and queued runs of them moved. The development vendor requests (the debug blocks, the gate) are the bridge's own (tools/ferros-bridge usb.rs), not here.

use nusb::transfer::{RequestBuffer, TransferError};

// VID G#1209 (pid.codes), PID G#4665 (ferros, pid.codes PR #1208, merged 2026-09-28). Same PID used by every ferros-shipped USB device; the VSF document's "PIPE message" section disambiguates protocol/role, so PID-level multiplexing isn't needed.
const FERROS_VID: u16 = 0x1209;
const FERROS_PID: u16 = 0x4665;
const INTERFACE: u8 = 0;
/// The device's vendor session request (ferros_hal usb.rs `VENDOR_REQ_SESSION`, bmRequestType G#40, no data).
const VENDOR_REQ_SESSION: u8 = 0x5D;

/// Bulk transfers kept in flight by the queued send and receive, and each one's size (a multiple of 512).
const QUEUE_DEPTH: usize = 4;
const QUEUE_PIECE: usize = 1 << 17;

pub struct UsbLink {
    interface: nusb::Interface,
    ep_out: u8,
    ep_in: u8,
}

impl UsbLink {
    /// Find and open the ferros device, and start a session on it (`session`). Returns None if not found.
    /// With two ferros devices on the host (the Pro and the 8a, 2026-11-10) the first found was taken, whichever it was: `FERROS_BUS=<bus number>` (lsusb's) picks one.
    pub async fn open() -> Result<Self, String> {
        let link = Self::find()?;
        link.session().await;
        Ok(link)
    }

    /// A new session (the device's VENDOR_REQ_SESSION, ferros_hal usb.rs): whatever an earlier host process left half done is dropped on the device before this one sends anything (a command half sent, a reply half read, a command still with the kernel, whose reply the device then drops). One host process holds the interface at a time (the claim is exclusive), so nothing live is cut. Until 2026-11-12 a `ferros send` killed in a held read left its reply in the device's IN endpoint, the next session read it as its SPEC ACK, and the port had to be kicked. A device from before stalls the request: passed over.
    async fn session(&self) {
        use nusb::transfer::{ControlOut, ControlType, Recipient};
        let c = self.interface.control_out(ControlOut { control_type: ControlType::Vendor, recipient: Recipient::Device, request: VENDOR_REQ_SESSION, value: 0, index: 0, data: &[] }).await;
        if crate::traced() {
            eprintln!("session: {}", if c.status.is_ok() { "started (the device dropped what an earlier session left)" } else { "not known to this device (a driver from before)" });
        }
    }

    fn find() -> Result<Self, String> {
        let bus: Option<u8> = std::env::var("FERROS_BUS").ok().and_then(|v| v.parse().ok());
        let device = nusb::list_devices()
            .map_err(|e| format!("USB enumeration failed: {e}"))?
            .find(|d| d.vendor_id() == FERROS_VID && d.product_id() == FERROS_PID && bus.is_none_or(|b| d.bus_number() == b))
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

        if crate::traced() {
            eprintln!("Connected: VID=G#{FERROS_VID:04X} PID=G#{FERROS_PID:04X} EP_OUT=G#{ep_out:02X} EP_IN=G#{ep_in:02X}");
        }

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

    /// Send whole 512-byte packets (`data`, a multiple of 512) with `QUEUE_DEPTH` bulk transfers of `QUEUE_PIECE` bytes in flight at once, so the host controller always has the next one when a transfer ends instead of waiting on this program.
    pub async fn send_queued(&self, data: &[u8]) -> Result<(), TransferError> {
        let mut q = self.interface.bulk_out_queue(self.ep_out);
        let mut pieces = data.chunks(QUEUE_PIECE);
        let mut pending = 0;
        loop {
            while pending < QUEUE_DEPTH {
                let Some(p) = pieces.next() else { break };
                q.submit(p.to_vec());
                pending += 1;
            }
            if pending == 0 {
                return Ok(());
            }
            q.next_complete().await.status?;
            pending -= 1;
        }
    }

    /// Receive exactly `len` bytes (a multiple of 512) with `QUEUE_DEPTH` bulk transfers in flight, each an exact run of whole packets: the queue completes in order, so the pieces join in order.
    pub async fn recv_queued(&self, len: usize) -> Result<Vec<u8>, TransferError> {
        let mut q = self.interface.bulk_in_queue(self.ep_in);
        let mut out = Vec::with_capacity(len);
        let (mut asked, mut pending) = (0usize, 0usize);
        while out.len() < len {
            while pending < QUEUE_DEPTH && asked < len {
                let n = (len - asked).min(QUEUE_PIECE);
                q.submit(RequestBuffer::new(n));
                asked += n;
                pending += 1;
            }
            let c = q.next_complete().await;
            pending -= 1;
            c.status?;
            out.extend_from_slice(&c.data);
        }
        Ok(out)
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

    /// The claimed interface, for a tool's own control transfers on it (the bridge's development vendor requests).
    pub fn interface(&self) -> &nusb::Interface {
        &self.interface
    }
}
