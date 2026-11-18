//! camera-pd-pair: a frame of the main camera and its phase-detect rows, as payloads/camera's SNAP kept them and `ferros-bridge read` pulled them (tools/camera-pd-pair.sh), into one VSF (owner, 2026-11-17: phase detect as data, for split-image and microprism looks and lens rendering from its left and right halves; docs/CAMERA.md "Phase detect as data").
//!
//! Both are MIPI RAW10 as the front end wrote them (four samples in five bytes, each line on its stride): the image 4080 x 3072 at G#1400 a line, the PD rows 4080 x 768 at the same stride (loch0's PD path, the link's channel 1).
//! The PD rows' layout (which samples are left- and right-masked pixels, where in the array) is not decoded yet: they are kept exactly as the sensor sent them, unpacked to 10-bit samples, so the decode works from the file.
//!
//! Usage: camera-pd-pair <image.raw10> <pd.raw10> <out.vsf> <the SNAP line's fields: count exposure_ns gain focus eagle_ns> [width rows stride pd_rows]
//! (the main's 4080 3072 5120 768 by default). Output: sections `image` and `pd`, each width, height, the samples as a 10-bit packed tensor (rows x width); `image` also its mosaic (cfa: 2 bits a site, top left first, 0 R 1 G 2 B: the main's G B / R G is G#49), black, white, exposure (ns), gain (1024 one), focus (the actuator's code), the frame's count; the creation time is the frame's own (Eagle time, from the driver's record of when it ended).

use vsf::{BitPackedTensor, OSCILLATIONS_PER_SECOND};
use vsf::{VsfBuilder, VsfType};

/// RAW10 lines into 10-bit samples: per line of `stride` bytes, `w` samples, each group of four the high eight bits of each and then their low two bits in one byte (sample k at bits 2k).
fn unpack(raw: &[u8], w: usize, h: usize, stride: usize) -> Vec<u16> {
    assert!(raw.len() >= h * stride, "short: {} bytes for {h} lines of {stride}", raw.len());
    assert_eq!(w % 4, 0, "a RAW10 line is whole groups of four");
    let mut px = vec![0u16; w * h];
    for y in 0..h {
        let line = &raw[y * stride..y * stride + w * 5 / 4];
        for (g, b) in line.chunks_exact(5).enumerate() {
            for k in 0..4 {
                px[y * w + g * 4 + k] = (b[k] as u16) << 2 | (b[4] as u16 >> (2 * k)) & 3;
            }
        }
    }
    px
}

fn u(v: u64) -> VsfType {
    VsfType::u(v as usize, false)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 9 {
        eprintln!("usage: camera-pd-pair <image.raw10> <pd.raw10> <out.vsf> <count> <exposure_ns> <gain> <focus> <eagle_ns> [width rows stride pd_rows]");
        std::process::exit(2);
    }
    let n = |k: usize, d: u64| a.get(k).map_or(d, |s| s.parse().unwrap_or_else(|_| panic!("argument {k} is not a number: {s}")));
    let (count, exposure, gain, focus, eagle_ns) = (n(4, 0), n(5, 0), n(6, 0), n(7, 0), n(8, 0));
    let (w, h, stride, pd_h) = (n(9, 4080) as usize, n(10, 3072) as usize, n(11, 5120) as usize, n(12, 768) as usize);
    let img = std::fs::read(&a[1]).unwrap_or_else(|e| panic!("{}: {e}", a[1]));
    let pd = std::fs::read(&a[2]).unwrap_or_else(|e| panic!("{}: {e}", a[2]));
    let ipx = unpack(&img, w, h, stride);
    let ppx = unpack(&pd, w, pd_h, stride);
    // The image's mean and the PD rows' mean, said, so a pair whose rows are the pattern or zeros shows at once.
    let mean = |p: &[u16]| p.iter().map(|&s| s as u64).sum::<u64>() / p.len() as u64;
    eprintln!("image {w} x {h}, mean {}; PD {w} x {pd_h}, mean {}", mean(&ipx), mean(&ppx));
    // Eagle time in hydrogen-line oscillations (1420407826 a second), from the frame's end in Eagle ns.
    let osc = (eagle_ns as u128 * OSCILLATIONS_PER_SECOND as u128 / 1_000_000_000) as i64;
    let bytes = VsfBuilder::new()
        .creation_time_oscillations(osc)
        .provenance_only()
        .add_section(
            "image",
            vec![
                ("width".to_string(), u(w as u64)),
                ("height".to_string(), u(h as u64)),
                ("cfa".to_string(), u(0x49)),
                ("black".to_string(), u(64)),
                ("white".to_string(), u(1023)),
                ("exposure_ns".to_string(), u(exposure)),
                ("gain".to_string(), u(gain)),
                ("focus".to_string(), u(focus)),
                ("count".to_string(), u(count)),
                ("samples".to_string(), VsfType::p(BitPackedTensor::pack(10, vec![h, w], &ipx))),
            ],
        )
        .add_section(
            "pd",
            vec![
                ("width".to_string(), u(w as u64)),
                ("height".to_string(), u(pd_h as u64)),
                ("samples".to_string(), VsfType::p(BitPackedTensor::pack(10, vec![pd_h, w], &ppx))),
            ],
        )
        .build()
        .unwrap_or_else(|e| panic!("the VSF would not build: {e}"));
    std::fs::write(&a[3], &bytes).unwrap_or_else(|e| panic!("{}: {e}", a[3]));
    eprintln!("{}: {} bytes, Eagle {} oscillations", a[3], bytes.len(), osc);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One RAW10 group of four: the high bytes, then the low bits of each at bits 2k; the line's padding past the samples ignored.
    #[test]
    fn a_group_unpacks() {
        let mut raw = vec![0u8; 16];
        raw[..5].copy_from_slice(&[0xFF, 0x00, 0x80, 0x01, 0b11_10_01_00]);
        assert_eq!(unpack(&raw, 4, 1, 16), vec![1020, 1, 514, 7]);
    }
}
