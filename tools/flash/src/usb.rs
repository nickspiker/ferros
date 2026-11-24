//! The fastboot transport on a real phone: nusb (plain Rust on Linux, macOS and Windows, no libusb), the crate the bridge already uses.
//! A bootloader in fastboot shows one interface of class G#FF, subclass G#42, protocol G#03 with a bulk endpoint each way; that triple finds it, whatever its USB ids.
//! On Windows nusb needs the WinUSB driver bound to that interface; whether the Pixel's bootloader asks Windows for it by itself (Microsoft OS descriptors) is docs/FLASHER.md's open measurement.

use crate::fastboot::Transport;
use nusb::transfer::{Direction, EndpointType, RequestBuffer};
use std::time::Duration;

const CLASS: u8 = 0xFF;
const SUBCLASS: u8 = 0x42;
const PROTOCOL: u8 = 0x03;

/// A phone in fastboot as the computer lists it, before it is opened.
#[derive(Clone, Debug)]
pub struct Found {
    pub vid: u16,
    pub pid: u16,
    pub serial: String,
    pub bus: u8,
    pub interface: u8,
    pub info: nusb::DeviceInfo,
}

/// Every phone in fastboot on this computer.
pub fn list() -> Result<Vec<Found>, String> {
    let all = nusb::list_devices().map_err(|e| format!("listing USB devices: {e}"))?;
    Ok(all
        .filter_map(|d| {
            let i = d.interfaces().find(|i| {
                i.class() == CLASS && i.subclass() == SUBCLASS && i.protocol() == PROTOCOL
            })?;
            Some(Found {
                vid: d.vendor_id(),
                pid: d.product_id(),
                serial: d.serial_number().unwrap_or("").to_string(),
                bus: d.bus_number(),
                interface: i.interface_number(),
                info: d.clone(),
            })
        })
        .collect())
}

/// An opened phone: its claimed interface and bulk endpoints.
pub struct Usb {
    rt: tokio::runtime::Runtime,
    interface: nusb::Interface,
    ep_out: u8,
    ep_in: u8,
}

impl Usb {
    pub fn open(f: &Found) -> Result<Usb, String> {
        let device = f
            .info
            .open()
            .map_err(|e| format!("opening the phone: {e}"))?;
        let interface = device
            .claim_interface(f.interface)
            .map_err(|e| format!("claiming its fastboot interface: {e}"))?;
        let config = device
            .active_configuration()
            .map_err(|e| format!("reading its configuration: {e}"))?;
        let (mut ep_out, mut ep_in) = (None, None);
        for i in config.interfaces() {
            for alt in i
                .alt_settings()
                .filter(|a| a.interface_number() == f.interface)
            {
                for ep in alt.endpoints() {
                    match (ep.direction(), ep.transfer_type()) {
                        (Direction::Out, EndpointType::Bulk) => ep_out = Some(ep.address()),
                        (Direction::In, EndpointType::Bulk) => ep_in = Some(ep.address()),
                        _ => {}
                    }
                }
            }
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Usb {
            rt,
            interface,
            ep_out: ep_out.ok_or("no bulk OUT endpoint")?,
            ep_in: ep_in.ok_or("no bulk IN endpoint")?,
        })
    }
}

impl Transport for Usb {
    fn write(&mut self, data: &[u8]) -> Result<(), String> {
        let (i, ep) = (&self.interface, self.ep_out);
        self.rt.block_on(async {
            tokio::select! {
                c = i.bulk_out(ep, data.to_vec()) => c.status.map(|_| ()).map_err(|e| e.to_string()),
                _ = tokio::time::sleep(crate::fastboot::WAIT) => Err("the phone took no data for A#16 s".into()),
            }
        })
    }

    fn read(&mut self, max: usize, wait: Duration) -> Result<Vec<u8>, String> {
        let (i, ep) = (&self.interface, self.ep_in);
        self.rt.block_on(async {
            tokio::select! {
                c = i.bulk_in(ep, RequestBuffer::new(max)) => c.status.map(|_| c.data).map_err(|e| e.to_string()),
                _ = tokio::time::sleep(wait) => Err(format!("no answer in A#{} s", wait.as_secs())),
            }
        })
    }
}
