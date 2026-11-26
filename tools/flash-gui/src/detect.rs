//! The live detector behind the device silhouette: one thread on `ferros_flash::usb::watch` (nusb's hotplug stream, a scan on each arrival or departure, never a poll), each change sent to the window as one `Device`.
//! Detection is read only: a bootloader is opened just long enough to ask its product, so the silhouette can say "Pixel 8a" rather than a USB id.

use ferros_flash::board::Board;
use ferros_flash::fastboot::Fastboot;
use ferros_flash::usb::{self, Seen, Usb};

/// What the silhouette shows (docs/FLASHER.md, screen 0).
#[derive(Clone, Debug, PartialEq)]
pub enum Device {
    /// Nothing seen: grey, a question mark.
    None,
    /// Seen, but this computer cannot open it (Windows: no WinUSB driver; Linux: no udev rule): amber.
    NoAccess { why: String },
    /// A Pixel in Android, named by its descriptor: warm.
    Android { model: String },
    /// A Pixel in its bootloader, named by the product it answers: warm.
    Bootloader { model: String },
    /// A device already running ferros: orange.
    Ferros,
    /// The USB list itself could not be read.
    Failed { why: String },
}

impl Device {
    /// The word inside the silhouette.
    pub fn word(&self) -> &str {
        match self {
            Device::None => "?",
            Device::NoAccess { .. } => "no access",
            Device::Android { model } | Device::Bootloader { model } => model,
            Device::Ferros => "ferros",
            Device::Failed { .. } => "?",
        }
    }

    /// The smaller line under the word, when there is one.
    pub fn under(&self) -> &str {
        match self {
            Device::NoAccess { .. } if cfg!(windows) => "driver needed",
            Device::NoAccess { .. } => "no permission to open it",
            Device::Android { .. } => "in Android",
            Device::Bootloader { .. } => "in its bootloader",
            Device::Ferros => "already running ferros",
            _ => "",
        }
    }

    /// Whether the install button wakes under the silhouette.
    pub fn seen(&self) -> bool {
        matches!(self, Device::Android { .. } | Device::Bootloader { .. } | Device::NoAccess { .. })
    }
}

/// The bootloader's model from the product it answers (`akita` is the Pixel 8a), else the product word itself.
fn bootloader_model(f: &usb::Found) -> String {
    let product = Usb::open(f)
        .ok()
        .and_then(|u| Fastboot::new(u, Board::akita()).getvar("product").ok())
        .unwrap_or_default();
    let product = product.trim();
    match Board::for_product(product) {
        Some(b) => b.model,
        None if product.is_empty() => "a bootloader".into(),
        None => product.to_string(),
    }
}

/// One `Device` from everything seen: a bootloader first, then a device in Android, then ferros, then one that cannot be opened.
pub fn pick(seen: Result<Vec<Seen>, String>) -> Device {
    let seen = match seen {
        Ok(s) => s,
        Err(why) => return Device::Failed { why },
    };
    if let Some(Seen::Fastboot(f)) = seen.iter().find(|s| matches!(s, Seen::Fastboot(_))) {
        return Device::Bootloader { model: bootloader_model(f) };
    }
    if let Some(Seen::Other { product, .. }) = seen.iter().find(|s| matches!(s, Seen::Other { .. })) {
        let model = if product.is_empty() { "a Pixel".to_string() } else { product.clone() };
        return Device::Android { model };
    }
    if seen.iter().any(|s| matches!(s, Seen::Ferros { .. })) {
        return Device::Ferros;
    }
    if let Some(Seen::NoDriver { why, .. }) = seen.iter().find(|s| matches!(s, Seen::NoDriver { .. })) {
        return Device::NoAccess { why: why.clone() };
    }
    Device::None
}

/// The detector thread: `deliver` gets each change (the first right away); it returns false when the window is gone, and the thread ends.
pub fn start(deliver: impl FnMut(Device) -> bool + Send + 'static) {
    std::thread::Builder::new()
        .name("usb watch".into())
        .spawn(move || {
            let mut deliver = deliver;
            let mut last: Option<Device> = None;
            let r = usb::watch(|seen| {
                let d = pick(seen);
                // A bus event that changed nothing for us (a mouse replugged) is not a change.
                if last.as_ref() == Some(&d) {
                    return true;
                }
                last = Some(d.clone());
                deliver(d)
            });
            if let Err(why) = r {
                deliver(Device::Failed { why });
            }
        })
        .expect("the usb watch thread");
}
