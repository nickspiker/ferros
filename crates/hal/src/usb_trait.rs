/// USB event returned by [`UsbBulk::poll_event`]. Hardware-independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbEvent {
    None,
    Reset,
    ConnectDone { speed: u32 },
    Disconnect,
    Ep0Setup { request: [u8; 8] },
    TransferComplete { ep: u8 },
    TransferNotReady { ep: u8 },
}

/// Minimal USB bulk device interface required by the PT event loop.
///
/// Implemented by each platform HAL. All methods operate on the controller's internal static DMA buffers — no external allocation.
pub trait UsbBulk {
    /// Poll for the next hardware event. Non-blocking; returns `None` if idle.
    fn poll_event(&mut self) -> UsbEvent;

    /// Arm the bulk OUT endpoint to receive one packet (≤512 bytes).
    fn bulk_out_arm(&mut self);

    /// Consume bulk OUT data received since the last `bulk_out_arm`. Returns `None` if no data is ready. Caller must call `bulk_out_arm` again after consuming.
    fn bulk_out_read(&mut self) -> Option<&[u8]>;

    /// Start a bulk IN transfer. Returns `true` if successfully armed.
    fn bulk_in_send(&mut self, data: &[u8]) -> bool;

    /// Returns `true` if the bulk IN endpoint is idle (ready for next send).
    fn bulk_in_is_idle(&self) -> bool;

    /// Handle a USB bus reset.
    fn handle_reset(&mut self);

    /// Handle connect-done (speed negotiated after reset).
    fn handle_connect_done(&mut self);

    /// Handle cable disconnect.
    fn handle_disconnect(&mut self);

    /// Handle an EP0 SETUP packet. Returns `false` if the request should be stalled.
    fn handle_setup(&mut self, request: &[u8; 8]) -> bool;

    /// Re-arm EP0 to receive the next SETUP packet.
    fn ep0_start_setup(&mut self);

    /// Stall EP0.
    fn ep0_stall(&mut self);

    /// Cancel any in-flight bulk IN transfer (called before hot-reload jump).
    fn cancel_bulk_in(&mut self);

    /// Shut down the controller cleanly before a kernel jump or reboot.
    fn shutdown(&mut self);
}
