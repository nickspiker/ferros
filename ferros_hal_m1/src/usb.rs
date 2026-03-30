//! M1 USB stub.
//!
//! Apple's USB controller requires reverse-engineering from m1n1/Asahi sources.
//! Until implemented, the kernel runs without USB — output goes to framebuffer.

use ferros_hal::{UsbBulk, UsbEvent};

pub struct M1UsbBulk;

impl M1UsbBulk {
    /// Returns `None` — USB not yet implemented on M1.
    pub fn init() -> Option<Self> {
        None
    }
}

impl UsbBulk for M1UsbBulk {
    fn poll_event(&mut self) -> UsbEvent { UsbEvent::None }
    fn bulk_out_arm(&mut self) {}
    fn bulk_out_read(&mut self) -> Option<&[u8]> { None }
    fn bulk_in_send(&mut self, _data: &[u8]) -> bool { false }
    fn bulk_in_is_idle(&self) -> bool { true }
    fn handle_reset(&mut self) {}
    fn handle_connect_done(&mut self) {}
    fn handle_disconnect(&mut self) {}
    fn handle_setup(&mut self, _request: &[u8; 8]) -> bool { false }
    fn ep0_start_setup(&mut self) {}
    fn ep0_stall(&mut self) {}
    fn cancel_bulk_in(&mut self) {}
    fn shutdown(&mut self) {}
}
