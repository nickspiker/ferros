//! The icon decoder at stow (docs/KISTS.md "Icons"; coordinator, 2026-11-20).
//!
//! A kist's icon is an AV1 picture in its signed capsule. The display decodes nothing, and the stower is no_std, so this std process decodes each icon once, at stow: the stower hands it the AV1 in pieces (ferros_kept::icon `dec::OP_AV1`), it decodes the picture (vsf::visual::av1, rav1d) and makes what the display draws (ferros_kept::icon `texels`: straight `g2t` RGBA, 256 x 256, the circle's coverage in alpha), and hands that back in pieces (`dec::OP_TEXELS`). The stower stores it in `kists.` under the icon's `hp`.
//!
//! Its one grant is its endpoint, `icons` (slot 1; slot 0 its console): no vault, no network, no device. rav1d is a large decoder fed a stranger's bytes; here it has nothing to misuse. The kernel starts it when the stower calls and none runs (process.rs `ON_DEMAND`), and it ends itself once it has handed an icon back: one icon, one process.

use ferros_kept::icon::{dec, texels, SIDE, TEXELS};

/// An icon's AV1 decoded into what the display draws; None for anything that is not a 256 x 256, 8-bit, 4:2:0 picture (an icon is exactly that, KISTS.md "Icons").
fn decode(av1: &[u8], gamma2: bool) -> Option<Vec<u8>> {
    let planes = vsf::visual::av1::decode(av1, 3, SIDE, SIDE, 8).ok()?;
    texels(&planes, gamma2)
}

#[cfg(target_os = "ferros")]
mod sys {
    use core::arch::asm;
    const RECV: u64 = 2;
    const REPLY: u64 = 3;

    #[inline(always)]
    fn svc(n: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
        let r: u64;
        unsafe { asm!("svc #0", inlateout("x0") a0 => r, in("x1") a1, in("x2") a2, in("x3") a3, in("x4") a4, in("x8") n, options(nostack)) };
        r
    }
    #[inline(always)]
    fn arc_base() -> u64 {
        let b: u64;
        unsafe { asm!("mov {}, x28", out(reg) b, options(nomem, nostack, preserves_flags)) };
        b
    }
    #[inline(always)]
    fn off<T: ?Sized>(p: *const T) -> u64 {
        (p as *const u8 as u64).wrapping_sub(arc_base())
    }
    fn cap(slot: u64) -> u64 {
        0x204 + 32 * slot
    }
    pub fn recv(slot: u64, buf: &mut [u8], token: &mut [u8; 32]) -> u64 {
        svc(RECV, cap(slot), off(buf.as_mut_ptr()), buf.len() as u64, off(token.as_mut_ptr()), 0)
    }
    pub fn reply(token: &[u8; 32], msg: &[u8]) -> u64 {
        svc(REPLY, off(token.as_ptr()), off(msg.as_ptr()), msg.len() as u64, 0, 0)
    }
}

/// The endpoint's slot.
#[cfg(target_os = "ferros")]
const SERVE: u64 = 1;

#[cfg(target_os = "ferros")]
fn main() {
    let mut buf = vec![0u8; 1 << 12];
    let mut token = [0u8; 32];
    let mut av1: Vec<u8> = Vec::new();
    let mut out: Option<Vec<u8>> = None;
    loop {
        let mut handed = false;
        let n = sys::recv(SERVE, &mut buf, &mut token) as usize;
        // WHY the min: the kernel copies at most the buffer and says the message's length.
        let m = &buf[..n.min(buf.len())];
        let answer: Vec<u8> = match m.first().copied() {
            Some(dec::OP_AV1) if m.len() >= 7 => {
                let at = u32::from_le_bytes([m[2], m[3], m[4], m[5]]) as usize;
                let piece = &m[7..];
                // In order from 0, and no more than an icon is: anything else starts nothing.
                if at != av1.len() || at + piece.len() > dec::AV1_MAX {
                    av1.clear();
                    vec![1]
                } else {
                    av1.extend_from_slice(piece);
                    if m[6] == 0 {
                        vec![0]
                    } else {
                        out = decode(&av1, m[1] == dec::GAMMA2);
                        av1.clear();
                        vec![if out.is_some() { 0 } else { 1 }]
                    }
                }
            }
            Some(dec::OP_TEXELS) if m.len() >= 5 => {
                let at = u32::from_le_bytes([m[1], m[2], m[3], m[4]]) as usize;
                match &out {
                    Some(t) if at < TEXELS => {
                        // WHY the min: the last piece is what is left of the texels.
                        let end = (at + dec::PIECE).min(TEXELS);
                        handed = end == TEXELS;
                        t[at..end].to_vec()
                    }
                    _ => vec![],
                }
            }
            _ => vec![],
        };
        sys::reply(&token, &answer);
        // One icon, one process: handed back whole, it ends (the next stow's call starts another).
        if handed {
            std::process::exit(0);
        }
    }
}

/// On the host, the seed (coordinator, 2026-11-20: the bench and a first deploy need no fresh stow): `icondec <capsule or icon document> <out.vsf>` decodes the icon exactly as the device does and writes its decoded document (ferros_kept::icon `decoded`), named by the `hp` the device knows it by (ferros_kist `icon_hp`); `ferros-bridge icon seed <out.vsf>` stores it in the device's `kists.` under that `hp`.
#[cfg(not(target_os = "ferros"))]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [src, out] = &args[..] else {
        eprintln!("usage: icondec <capsule.vsf | icon.vsf> <out.vsf>\n  the device's icon decoder at stow (tools/std-build.sh std/icondec); on the host, the decoded document to seed with `ferros-bridge icon seed`");
        std::process::exit(2);
    };
    let fail = |e: &str| -> ! {
        eprintln!("icondec: {src}: {e}");
        std::process::exit(1)
    };
    let bytes = std::fs::read(src).unwrap_or_else(|e| fail(&e.to_string()));
    // A capsule carries its icon in a section `icon` (its `hp` is that section's BLAKE3); an icon document's `image` section holds the same two fields, and in a capsule they make the same section.
    let (hp, icon) = match ferros_kist::icon_of(&bytes) {
        Ok(Some(icon)) => (ferros_kist::icon_hp(&bytes).ok().flatten().unwrap_or_else(|| fail("an icon without its identity")), icon),
        _ => {
            let icon = ferros_kist::icon_document(&bytes).unwrap_or_else(|e| fail(e));
            (ferros_thing::icon(&ferros_kist::icon_section(&icon).encode()), icon)
        }
    };
    let texels = decode(&icon.av1, icon.encoding == vsf::visual::Encoding::Gamma2).unwrap_or_else(|| fail("not a 256 x 256 AV1 icon"));
    let doc = ferros_kept::icon::decoded(&hp, &texels, vsf::eagle_time_oscillations()).unwrap_or_else(|| fail("the decoded document did not build"));
    std::fs::write(out, &doc).unwrap_or_else(|e| fail(&e.to_string()));
    println!("icondec: {src}: icon {} decoded, {} bytes -> {out}", hp.as_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>(), doc.len());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture made the way an icon is (VSF RGB codes, 256 square, 4:2:0, AV1 by vsf's encoder) decodes into what the display draws: its codes back near where they were (AV1 is lossy), opaque inside the circle, clear outside.
    #[test]
    fn an_icon_decodes_into_what_the_display_draws() {
        let n = SIDE * SIDE;
        let mut planes = vec![0u16; 3 * n];
        for y in 0..SIDE {
            for x in 0..SIDE {
                let i = y * SIDE + x;
                planes[i] = (x as u16) & 0xFF;
                planes[n + i] = (y as u16) & 0xFF;
                planes[2 * n + i] = 128;
            }
        }
        let av1 = vsf::visual::av1::encode(&planes, 3, SIDE, SIDE, 8, &vsf::visual::av1::Av1Settings::default()).expect("vsf's encoder");
        assert!(av1.len() <= dec::AV1_MAX, "an icon's AV1 fits the endpoint: {} bytes", av1.len());
        let t = decode(&av1, false).expect("an icon");
        assert_eq!(t.len(), TEXELS);
        let at = |x: usize, y: usize| &t[(y * SIDE + x) * 4..(y * SIDE + x) * 4 + 4];
        let c = at(100, 140);
        assert!((c[0] as i32 - 100).abs() <= 8 && (c[1] as i32 - 140).abs() <= 8 && (c[2] as i32 - 128).abs() <= 8, "near the codes: {c:?}");
        assert_eq!(c[3], 255);
        assert_eq!(at(0, 0)[3], 0);
        // Not an icon: bytes that are no AV1, and a picture of another size.
        assert_eq!(decode(&[1, 2, 3], false), None);
        let small = vsf::visual::av1::encode(&planes[..3 * 64 * 64], 3, 64, 64, 8, &vsf::visual::av1::Av1Settings::default()).unwrap();
        assert_eq!(decode(&small, false), None);
    }
}
