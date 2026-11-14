#![no_std]
#![no_main]

fn main(_grants: u64) -> ! {
    ferros_sys::exit(0)
}
ferros_sys::entry!(main);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    ferros_sys::exit(1)
}
