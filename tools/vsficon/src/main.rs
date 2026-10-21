//! Decode a VSF image into raw RGBA with fluor's own decoder, so a build step without VSF support can take an app icon that only exists as VSF (Opsin's orb).
//!
//! Usage: vsficon <in.vsf> <out.rgba>
//! Output: width and height as little-endian u32, then width x height RGBA bytes, square and opaque. The circle is cut later, by tools/app-icons.py, once for every icon whatever format it came in.
use std::{env, fs, process};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: vsficon <in.vsf> <out.rgba>");
        process::exit(2);
    }
    let bytes = fs::read(&args[1]).unwrap_or_else(|e| {
        eprintln!("vsficon: {}: {e}", args[1]);
        process::exit(1);
    });
    let icon = fluor::host::icon::Icon::from_vsf_bytes(&bytes).unwrap_or_else(|e| {
        eprintln!("vsficon: {}: not a VSF image ({e:?})", args[1]);
        process::exit(1);
    });
    // fluor packs a pixel as alpha over darkness — the RGB bytes are 255 minus the visible colour — so one XOR turns it back. (Icon::to_rgba_circular does this too, but it is compiled only with fluor's winit host.)
    let (w, h) = (icon.width, icon.height);
    let mut rgba = Vec::with_capacity(icon.pixels.len() * 4);
    for &p in icon.pixels.iter() {
        let v = p ^ 0x00FF_FFFF;
        rgba.extend_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8, 0xFF]);
    }
    let mut out = Vec::with_capacity(8 + rgba.len());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&rgba);
    fs::write(&args[2], out).unwrap_or_else(|e| {
        eprintln!("vsficon: {}: {e}", args[2]);
        process::exit(1);
    });
    println!("{} -> {} ({w} x {h})", args[1], args[2]);
}
