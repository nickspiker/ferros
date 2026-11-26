//! A VSF capture of the window's own frame (`--snap <out.vsf>`): the finished frame in fluor's α + premultiplied darkness, read as the light over black (α − darkness, "Claude memories/feedback_darkness_space.md"), written as tools/screencap writes the device's screen: one Display plane, VSF RGB, gamma2tanh at 16 bits.
//! tools/vsf2web turns it into an AV1 picture for a browser.

use fluor::pixel::{G2t16, Pixel};
use vsf::types::BitPackedTensor;
use vsf::visual::{Encoding, Plane, Primaries, Visual};

pub fn write(path: &str, frame: &[G2t16], w: usize, h: usize) -> Result<(), String> {
    let mut samples = vec![0u16; 3 * w * h];
    for (i, px) in frame[..w * h].iter().enumerate() {
        let a = px.alpha();
        let [r, g, b] = px.channels();
        // The accumulator's darkness is premultiplied, so the code shown over black is α − d; a transparent corner (α = 0, d = 0) is black.
        for (c, d) in [r, g, b].into_iter().enumerate() {
            samples[c * w * h + i] = (a - d) as u16;
        }
    }
    let mut plane = Plane::encoded("display", "screen", w, h, Primaries::Vsf, Encoding::Gamma2Tanh, BitPackedTensor::pack(16, vec![3, h, w], &samples));
    plane.name = "window".into();
    let v = Visual {
        title: "ferros installer".into(),
        text: "The installer window's frame as fluor composed it, before the present's dither.".into(),
        planes: vec![plane],
        ..Default::default()
    };
    let file = v.write()?;
    std::fs::write(path, &file).map_err(|e| format!("{path}: {e}"))
}
