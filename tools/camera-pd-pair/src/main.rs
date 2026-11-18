//! camera-pd-pair: a frame of the main camera and its phase-detect rows, as payloads/camera's SNAP held them and `ferros-bridge read` pulled them (tools/camera-pd-pair.sh), into one VSF visual (owner, 2026-11-17: phase detect as data, for split-image and microprism looks, lens rendering and a focus map from its left and right halves; docs/CAMERA.md "Phase detect as data").
//!
//! Both are MIPI RAW10 as the front end wrote them (four samples in five bytes, each line on its stride): the image 4080 x 3072 at G#1400 a line, the PD rows 4080 x 768 at the same stride (loch0's PD path, the link's channel 1), in the same ring buffer, so of the same frame.
//! The visual: the image as the usual sensor plane (`plane.b0.s0.sensor`, its mosaic G B / R G, black 64, white 1023, EXIF orientation 6), and the PD rows as one plane of role `phase` (vsf 0.11.0 at 1255a07, `PlaneRole::Phase`, lossless only) named `phase`: raw counts in the sensor's own order, not split into pupil halves, since which samples are which half is not decoded yet; the decode works from this file.
//! The capture: exposure, gain (ISO 100 at 1024), the instant the frame ended (Eagle time, from the driver's record), mode `single`, the sensor module named.
//!
//! Usage: camera-pd-pair <image.raw10> <pd.raw10> <out.vsf> <count> <exposure_ns> <gain> <focus> <eagle_ns> [width rows stride pd_rows] (the main's 4080 3072 5120 768 by default).

use vsf::spectral_image::{PlaneLayout, SpectralChannel};
use vsf::visual::{Capture, Plane, PlaneRole, Visual};
use vsf::{BitPackedTensor, OSCILLATIONS_PER_SECOND, Tensor};

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

fn ch(name: &str) -> SpectralChannel {
    SpectralChannel { name: name.into(), curve: None }
}

/// The visual: the sensor plane and the phase plane beside it.
fn visual(ipx: &[u16], ppx: &[u16], w: usize, h: usize, pd_h: usize, exposure_ns: u64, gain: u64, focus: u64, at: i64, count: u64) -> Visual {
    // The main's mosaic, G B / R G, as channel indices R 0, G 1, B 2.
    let cfa = Tensor::new(vec![2, 2], vec![1u8, 2, 0, 1]);
    let sensor = Plane { orientation: 6, at: Some(at), ..Plane::sensor("b0", "s0", w, h, vec![ch("R"), ch("G"), ch("B")], PlaneLayout::Mosaic { cfa }, vec![64.; 3], vec![1023.; 3], BitPackedTensor::pack(10, vec![h, w], ipx)) };
    let capture = Capture { exposure_s: Some(exposure_ns as f32 / 1e9), gain: Some(gain as f32 / 1024.), iso: Some(gain as f32 * 100. / 1024.), at: Some(at), mode: "single".into(), sensor_module: "main (Samsung G#08EF)".into(), ..Capture::default() };
    let mut v = Visual::still(sensor, Some(capture), None, &[]);
    let mut phase = Plane::sensor("b0", "s0", w, pd_h, vec![ch("PD")], PlaneLayout::Planar, vec![64.], vec![1023.], BitPackedTensor::pack(10, vec![1, pd_h, w], ppx));
    phase.name = "phase".into();
    phase.role = PlaneRole::Phase;
    phase.at = Some(at);
    v.planes.push(phase);
    v.text = format!("ferros payloads/camera SNAP, frame {count}: the main's binned frame and its phase-detect rows (loch0's PD path, channel 1) from the same ring buffer, raw counts in the sensor's order; lens actuator at {focus:#06X}");
    v
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
    // The image's mean and the PD rows' mean, said, so a pair whose rows are the open's pattern or zeros shows at once.
    let mean = |p: &[u16]| p.iter().map(|&s| s as u64).sum::<u64>() / p.len() as u64;
    eprintln!("image {w} x {h}, mean {}; PD {w} x {pd_h}, mean {}", mean(&ipx), mean(&ppx));
    // Eagle time in hydrogen-line oscillations, from the frame's end in Eagle ns.
    let at = (eagle_ns as u128 * OSCILLATIONS_PER_SECOND as u128 / 1_000_000_000) as i64;
    let bytes = visual(&ipx, &ppx, w, h, pd_h, exposure, gain, focus, at, count).write().unwrap_or_else(|e| panic!("the VSF would not build: {e}"));
    std::fs::write(&a[3], &bytes).unwrap_or_else(|e| panic!("{}: {e}", a[3]));
    eprintln!("{}: {} bytes", a[3], bytes.len());
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

    /// The visual reads back with its sensor plane first and the phase plane beside it, both lossless, the samples exact.
    #[test]
    fn the_pair_round_trips() {
        let (w, h, pd_h) = (16usize, 8usize, 2usize);
        let ipx: Vec<u16> = (0..w * h).map(|i| (i * 7 % 1024) as u16).collect();
        let ppx: Vec<u16> = (0..w * pd_h).map(|i| (i * 13 % 1024) as u16).collect();
        let v = visual(&ipx, &ppx, w, h, pd_h, 67_000_000, 1024, 0x3800, 1 << 40, 9);
        let back = Visual::read(&v.write().unwrap()).unwrap();
        assert_eq!(back, v);
        assert_eq!(back.primary_plane().map(|p| p.role), Some(PlaneRole::Sensor));
        let phase = back.planes.iter().find(|p| p.role == PlaneRole::Phase).unwrap();
        assert_eq!((phase.name.as_str(), phase.width, phase.height), ("phase", w, pd_h));
    }
}
