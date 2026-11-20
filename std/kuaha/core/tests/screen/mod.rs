//! Kūaha's frames on the host: drawn headless into a 16-bit fluor buffer the size of the Pro's glass, written as a VSF visual (one Display plane, gamma2tanh at 16 bits, the buffer's own primaries) for the owner, and as colourless ASCII for a reader with no eyes on the colours.

#![allow(dead_code)]

use fluor::canvas::Damage;
use fluor::pixel::G2t16;
use fluor::text::TextRenderer;
use kuaha_core::{HoldSource, Kuaha, Locale};
use vsf::types::BitPackedTensor;
use vsf::visual::{Encoding, Plane, Primaries, Visual};

/// The Pro's glass (husky, 1344 x 2992).
pub const W: usize = 1344;
pub const H: usize = 2992;

pub struct Screen {
    pub px: Vec<G2t16>,
    pub text: TextRenderer,
}

impl Screen {
    pub fn new() -> Screen {
        Screen { px: vec![0; W * H], text: TextRenderer::new() }
    }

    pub fn draw(&mut self, k: &mut Kuaha, loc: &Locale) {
        self.px.fill(0);
        let mut damage = Damage::new();
        k.draw(&mut self.px, W, H, &mut self.text, loc, &mut damage);
    }

    /// A pixel's light code per channel: what is still see-through is over black, so the light is α less the premultiplied darkness.
    fn codes(p: G2t16) -> [u16; 3] {
        let a = (p >> 48) as u16;
        [a - (p >> 32) as u16, a - (p >> 16) as u16, a - p as u16]
    }

    /// The frame as a VSF visual in the build's scratch (`CARGO_TARGET_TMPDIR`): its path.
    pub fn write(&self, name: &str) -> String {
        let mut samples = vec![0u16; 3 * W * H];
        for (i, &p) in self.px.iter().enumerate() {
            let c = Self::codes(p);
            for ch in 0..3 {
                samples[ch * W * H + i] = c[ch];
            }
        }
        // The host build's buffer holds Rec.2020 codes (fluor theme `vsf`: VSF RGB to the working primaries everywhere but macOS); the plane says so, and a reader converts back.
        let mut plane = Plane::encoded("display", "screen", W, H, Primaries::Rec2020, Encoding::Gamma2Tanh, BitPackedTensor::pack(16, vec![3, H, W], &samples));
        plane.name = "kuaha".into();
        let v = Visual { title: format!("Kuaha, {name}"), text: "The Kuaha frame drawn on the host at the Pro's size, fluor's 16-bit buffer as it would reach the display.".into(), planes: vec![plane], ..Default::default() };
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("kuaha-screens");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.vsf"));
        std::fs::write(&path, v.write().unwrap()).unwrap();
        std::fs::write(dir.join(format!("{name}.txt")), self.ascii(112)).unwrap();
        path.display().to_string()
    }

    /// The frame as text, `cols` across: each cell the mean light of its pixels on a ramp, two rows of pixels a cell for every one across (a character is about twice as tall as wide).
    pub fn ascii(&self, cols: usize) -> String {
        const RAMP: &[u8] = b" .:-=+*#%@";
        let cw = W / cols;
        let ch = cw * 2;
        let mut out = String::new();
        for cy in 0..H / ch {
            for cx in 0..cols {
                let mut sum = 0u64;
                for y in cy * ch..cy * ch + ch {
                    for x in cx * cw..cx * cw + cw {
                        let c = Self::codes(self.px[y * W + x]);
                        sum += (c[0] as u64 + 2 * c[1] as u64 + c[2] as u64) >> 2;
                    }
                }
                let mean = sum / (cw * ch) as u64;
                // The ramp over the codes up to diffuse white (g2t's white is about 0xC2 of 0xFF at 8 bits); WHY the min: light above white (g2t's headroom) is the ramp's top, not past its end.
                let i = (mean * RAMP.len() as u64 / 0xC400).min(RAMP.len() as u64 - 1);
                out.push(RAMP[i as usize] as char);
            }
            out.push('\n');
        }
        out
    }
}

/// Every ask Kūaha queued, run on `source` until none is left.
pub fn pump(k: &mut Kuaha, source: &mut dyn HoldSource) {
    loop {
        let asks = k.take_asks();
        if asks.is_empty() {
            return;
        }
        for a in asks {
            let answer = kuaha_core::serve(source, a);
            k.answer(answer);
        }
    }
}
