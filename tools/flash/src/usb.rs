//! The fastboot transport on a real phone: nusb (plain Rust on Linux, macOS and Windows, no libusb), the crate the bridge already uses.
//! A bootloader in fastboot shows one interface of class G#FF, subclass G#42, protocol G#03 with a bulk endpoint each way; that triple finds it, whatever its USB ids.
//! On Windows nusb needs the WinUSB driver bound to that interface; when it is not, the guide installs Google's own USB driver package (the CLI's driver.rs).
//! `watch` is the live detector (the GUI's device silhouette): nusb's hotplug stream, a scan on each arrival or departure, never a poll.

use crate::fastboot::Transport;
use nusb::transfer::{Direction, EndpointType, RequestBuffer};
use std::sync::Arc;
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
    /// A Google device this computer cannot open (on Windows: no WinUSB driver bound to it; on Linux: no udev rule), with the system's own words.
    NoDriver { vid: u16, pid: u16, why: String },
    /// A Google device that is not in fastboot (the device in Android): its USB ids and the product its descriptor names ("Pixel 8a").
    Other { vid: u16, pid: u16, product: String },
    /// A device already running ferros (its own USB ids).
    Ferros { product: String },
}

const GOOGLE: u16 = 0x18D1;
/// ferros's own USB ids (pid.codes, docs/FLASHER.md).
pub const FERROS_VID: u16 = 0x1209;
pub const FERROS_PID: u16 = 0x4665;
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
        // A device already running ferros (its own USB ids): not in its bootloader either.
        if d.vendor_id() == FERROS_VID && d.product_id() == FERROS_PID {
            out.push(Seen::Ferros { product: d.product_string().unwrap_or("ferros").to_string() });
            continue;
        }
        if d.vendor_id() != GOOGLE {
            continue;
        }
        let (vid, pid) = (d.vendor_id(), d.product_id());
        let product = d.product_string().unwrap_or("").to_string();
        if d.interfaces().next().is_none() {
            match d.open() {
                Err(e) => out.push(Seen::NoDriver {
                    vid,
                    pid,
                    why: e.to_string(),
                }),
                Ok(dev) => match fastboot_interface(&dev) {
                    Some(n) => out.push(Seen::Fastboot(found(&d, n))),
                    None => out.push(Seen::Other { vid, pid, product }),
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
                    Ok(()) => out.push(Seen::Other { vid, pid, product }),
                }
            }
            _ => out.push(Seen::Other { vid, pid, product }),
        }
    }
    Ok(out)
}

/// A waker that unparks the thread waiting on the hotplug stream.
struct Unpark(std::thread::Thread);

impl std::task::Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// The devices on this computer as they change: `seen` is called once with what is there now, and again after every USB arrival or departure (nusb's hotplug stream: udev's netlink on Linux, IOKit on macOS, device notifications on Windows), each time with a fresh `scan`.
/// Nothing is asked on a timer: between events the thread is parked, woken by the stream's own waker.
/// It holds the thread it runs on and returns only when the stream ends, or when `seen` returns false.
pub fn watch(mut seen: impl FnMut(Result<Vec<Seen>, String>) -> bool) -> Result<(), String> {
    use std::task::{Context, Poll};
    let mut stream = Box::pin(nusb::watch_devices().map_err(|e| format!("watching USB devices: {e}"))?);
    if !seen(scan()) {
        return Ok(());
    }
    let waker = std::task::Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match futures_core::Stream::poll_next(stream.as_mut(), &mut cx) {
            Poll::Ready(Some(_)) => {
                if !seen(scan()) {
                    return Ok(());
                }
            }
            Poll::Ready(None) => return Ok(()),
            Poll::Pending => std::thread::park(),
        }
    }
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
