//! The host's link to a ferros device over USB (docs/USB.md): the device found and its interface claimed (`usb`), and Photon Transport commands sent and answered over it (`pt`). One library for every host tool that speaks to a device: ferros-bridge (development) and ferros-send (stowing a kist, the person's tool). Nothing here is a development command; a tool names the capabilities it uses itself.
//!
//! Written for Linux first and kept portable: nusb is plain Rust on Linux, macOS and Windows, and nothing here touches a platform's own API.

pub mod pt;
pub mod usb;

use std::sync::atomic::{AtomicBool, Ordering};

static TRACE: AtomicBool = AtomicBool::new(false);

/// Say each transfer's steps on stderr (the bridge's development output); off by default, so a person's tool prints only what it means to.
pub fn trace(on: bool) {
    TRACE.store(on, Ordering::Relaxed);
}

fn traced() -> bool {
    TRACE.load(Ordering::Relaxed)
}
