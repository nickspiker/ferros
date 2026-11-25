//! The fastboot transport on a real phone: nusb (plain Rust on Linux, macOS and Windows, no libusb), the crate the bridge already uses.
//! A bootloader in fastboot shows one interface of class G#FF, subclass G#42, protocol G#03 with a bulk endpoint each way; that triple finds it, whatever its USB ids.
//! On Windows nusb needs the WinUSB driver bound to that interface; when it is not, the guide installs Google's own USB driver package (the CLI's driver.rs).

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

/// A Google device as this computer sees it.
#[derive(Clone, Debug)]
pub enum Seen {
    /// A bootloader in fastboot, its interface found.
    Fastboot(Found),
    /// A Google device this computer cannot open (on Windows: no WinUSB driver bound to it), with the system's own words.
    NoDriver { vid: u16, pid: u16, why: String },
    /// A Google device that is not in fastboot (the phone in Android): its USB ids.
    Other { vid: u16, pid: u16 },
}

const GOOGLE: u16 = 0x18D1;
/// The ADB interface (G#FF, G#42, G#01) of a phone in Android, checked on Windows so the driver goes on before the phone is even in its bootloader.
const ADB_PROTOCOL: u8 = 0x01;

fn found(d: &nusb::DeviceInfo, interface: u8) -> Found {
    Found {
        vid: d.vendor_id(),
        pid: d.product_id(),
        serial: d.serial_number().unwrap_or("").to_string(),
        bus: d.bus_number(),
        interface,
        info: d.clone(),
    }
}

/// The fastboot interface's number in an opened device's active configuration.
fn fastboot_interface(dev: &nusb::Device) -> Option<u8> {
    let config = dev.active_configuration().ok()?;
    for i in config.interfaces() {
        for a in i.alt_settings() {
            if a.class() == CLASS && a.subclass() == SUBCLASS && a.protocol() == PROTOCOL {
                return Some(a.interface_number());
            }
        }
    }
    None
}

/// Every phone in fastboot on this computer, and every other Google device with why it is not one.
/// Windows lists no interfaces for a device with one interface (nusb reads them only from a composite device's children), so a Google device listed with none is opened and its configuration read: a bootloader without the WinUSB driver fails that open, which is how a missing driver shows.
pub fn scan() -> Result<Vec<Seen>, String> {
    let all = nusb::list_devices().map_err(|e| format!("listing USB devices: {e}"))?;
    let mut out = Vec::new();
    for d in all {
        if let Some(i) = d
            .interfaces()
            .find(|i| i.class() == CLASS && i.subclass() == SUBCLASS && i.protocol() == PROTOCOL)
        {
            out.push(Seen::Fastboot(found(&d, i.interface_number())));
            continue;
        }
        // A phone already running ferros (its own USB ids): not in its bootloader either.
        if d.vendor_id() == 0x1209 && d.product_id() == 0x4665 {
            out.push(Seen::Other { vid: d.vendor_id(), pid: d.product_id() });
            continue;
        }
        if d.vendor_id() != GOOGLE {
            continue;
        }
        let (vid, pid) = (d.vendor_id(), d.product_id());
        if d.interfaces().next().is_none() {
            match d.open() {
                Err(e) => out.push(Seen::NoDriver {
                    vid,
                    pid,
                    why: e.to_string(),
                }),
                Ok(dev) => match fastboot_interface(&dev) {
                    Some(n) => out.push(Seen::Fastboot(found(&d, n))),
                    None => out.push(Seen::Other { vid, pid }),
                },
            }
            continue;
        }
        // A phone in Android: on Windows, whether its ADB interface has a driver (the same package covers it and the bootloader).
        let adb = d
            .interfaces()
            .find(|i| {
                i.class() == CLASS && i.subclass() == SUBCLASS && i.protocol() == ADB_PROTOCOL
            })
            .map(|i| i.interface_number());
        match adb {
            Some(n) if cfg!(windows) => {
                match d.open().and_then(|dev| dev.claim_interface(n).map(|_| ())) {
                    Err(e) => out.push(Seen::NoDriver {
                        vid,
                        pid,
                        why: e.to_string(),
                    }),
                    Ok(()) => out.push(Seen::Other { vid, pid }),
                }
            }
            _ => out.push(Seen::Other { vid, pid }),
        }
    }
    Ok(out)
}

/// Every phone in fastboot on this computer.
pub fn list() -> Result<Vec<Found>, String> {
    Ok(scan()?
        .into_iter()
        .filter_map(|s| {
            if let Seen::Fastboot(f) = s {
                Some(f)
            } else {
                None
            }
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
