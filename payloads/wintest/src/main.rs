//! A window to exercise the display's layer stack (WINDOWS.md, "The layer stack"): a fluor-format (`G2T16`) surface with a half-transparent red block in its middle and nothing elsewhere, shown at the level and with the mode byte the input names, for a minute, then done.
//! Input: byte 0 the level (0 an app, 2 the system), byte 1 the mode byte (bits 0-2 the blend mode in fluor's order, bit 3 opaque).
//! Spawn: ferros-bridge spawn wintest.bin 0001 --name wintest --use display --use touch --dma surface   (level 0, Multiply)
//! Grant slots as an app's: 0 console, 1 display (use), 2 touch (use), 3 its surface.
#![no_std]
#![no_main]

use ferros_sys::{call, cap, console, exit, freq, now, sleep_until, window};

const S_DISPLAY: u64 = 1;
const S_SURF: u64 = 3;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _entry(grants: u64, input: u64, input_len: u64) -> ! {
    unsafe { ferros_sys::relocate() };
    let display = cap(grants, S_DISPLAY);
    // The input is at an offset from the arc base (ferros_rt start.rs).
    let inp = unsafe { core::slice::from_raw_parts((ferros_sys::arc_base() + input) as *const u8, input_len as usize) };
    let (level, mode) = (if inp.is_empty() { 0 } else { inp[0] }, if inp.len() < 2 { 0 } else { inp[1] });
    let mut r = [0u8; 256];
    if call(display, &[5], &mut r) < 4 {
        console(grants, b"wintest: no display server\n");
        exit(1);
    }
    let (w, h) = (u16::from_le_bytes([r[0], r[1]]) as usize, u16::from_le_bytes([r[2], r[3]]) as usize);
    let base = ferros_sys::dma(cap(grants, S_SURF));
    if base == 0 || base == u64::MAX || w * h * 8 > 32 << 20 {
        console(grants, b"wintest: no app surface\n");
        exit(2);
    }
    let surf = unsafe { core::slice::from_raw_parts_mut(base as *mut u64, w * h) };
    surf.fill(0);
    // A fluor 16-bit pixel: alpha in bits 48-63, then each channel's darkness scaled by alpha (R 32-47, G 16-31, B 0-15). Half-transparent red: alpha G#8000, red no darkness, green and blue full darkness at that alpha.
    let red = (0x8000u64) << 48 | 0x8000 << 16 | 0x8000;
    for y in h / 4..h {
        for x in w / 6..w * 5 / 6 {
            surf[y * w + x] = red;
        }
    }
    if !window::show_full(display, base, 0, 0, w as u16, h as u16, level, window::G2T16, (0, 0, w as u16, h as u16), mode) {
        console(grants, b"wintest: the display refused the window\n");
        exit(3);
    }
    console(grants, b"wintest: shown\n");
    let end = now() + 60 * freq();
    while now() < end {
        sleep_until(now() + freq());
    }
    window::done(display, base);
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::panic_exit()
}
