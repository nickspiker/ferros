//! USB PHY/link register probe — snapshots the eUSB2 PHY, USBCON link block, and DWC3 state.
//!
//! Run against a WORKING (enumerated) link to capture the known-good baseline, especially LTSTATE_HIS (link training state history) and LINK_DEBUG — the candidates for a deterministic "PHY is actually up" poll to replace the blind udelays in the kernel's init.
//!
//! Register names from gs-google kernel: eusb-con-reg.h, exynos-usb-blkcon-sfr.h.

#![no_std]
#![no_main]

const USBCON: usize = 0x1110_0000;
const EUSB_PHY: usize = 0x1111_0000;
const DWC3: usize = 0x1121_0000;

struct Out {
    ptr: *mut u8,
    cap: usize,
    n: usize,
}

impl Out {
    fn put(&mut self, b: u8) {
        if self.n < self.cap {
            unsafe { *self.ptr.add(self.n) = b; }
            self.n += 1;
        }
    }
    fn label_hex(&mut self, label: &[u8], val: u32) {
        for &b in label {
            self.put(b);
        }
        self.put(b'=');
        let hex = b"0123456789ABCDEF";
        for i in (0..8).rev() {
            self.put(hex[((val >> (i * 4)) & 0xF) as usize]);
        }
        self.put(b'\n');
    }
}

fn rd(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

/// Entry — pinned at blob offset 0 by payload.ld.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(_inp: *const u8, _in_len: usize, out: *mut u8, out_cap: usize) -> u64 {
    let mut o = Out { ptr: out, cap: out_cap, n: 0 };

    // USBCON link block
    o.label_hex(b"USBCON_VERSION", rd(USBCON + 0x00));
    o.label_hex(b"LINKCTRL", rd(USBCON + 0x04));
    o.label_hex(b"LINKPORT", rd(USBCON + 0x08));
    o.label_hex(b"LINK_CLKRST", rd(USBCON + 0x0C));
    o.label_hex(b"UTMI_CTRL", rd(USBCON + 0x10));
    o.label_hex(b"LTSTATE_HIS", rd(USBCON + 0x80));
    o.label_hex(b"LINK_DEBUG_L", rd(USBCON + 0x84));
    o.label_hex(b"LINK_DEBUG_H", rd(USBCON + 0x88));

    // eUSB2 PHY control file
    o.label_hex(b"PHY_RST_CTRL", rd(EUSB_PHY + 0x00));
    o.label_hex(b"PHY_CMN_CTRL", rd(EUSB_PHY + 0x04));
    o.label_hex(b"PHY_PLLCFG0", rd(EUSB_PHY + 0x08));
    o.label_hex(b"PHY_PLLCFG1", rd(EUSB_PHY + 0x0C));
    o.label_hex(b"PHY_RCAL", rd(EUSB_PHY + 0x10));
    o.label_hex(b"PHY_TXTUNE", rd(EUSB_PHY + 0x14));
    o.label_hex(b"PHY_RXTUNE", rd(EUSB_PHY + 0x18));
    o.label_hex(b"PHY_UTMI", rd(EUSB_PHY + 0x1C));
    o.label_hex(b"PHY_TESTSE", rd(EUSB_PHY + 0x20));

    // DWC3 controller state
    o.label_hex(b"DWC3_GCTL", rd(DWC3 + 0xC110));
    o.label_hex(b"DWC3_GSTS", rd(DWC3 + 0xC118));
    o.label_hex(b"DWC3_DCFG", rd(DWC3 + 0xC700));
    o.label_hex(b"DWC3_DCTL", rd(DWC3 + 0xC704));
    o.label_hex(b"DWC3_DSTS", rd(DWC3 + 0xC70C));

    o.n as u64
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
