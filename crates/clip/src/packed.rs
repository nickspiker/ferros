//! Stacked clips: every completed exposure one frame of its exact sums, bit-packed at the width it needs (vsf `ClipCodec::Packed`, vsf 3bf262a), with how many exposures it sums and its levels; in a Motion clip a motion plane beside them. Lumis's CLIP records these where an exposure stacks more than one frame: a time-lapse, and Difference and Motion (the owner, 2026-11-23: "our nice bitpacked RAW frames for video that probably is a time lapse, thus has a bunch of integrations per frame so the bit depth would be higher but you also wouldn't be shooting at 60fps"). The frames arrive packed, so nothing is coded here: a writer thread of its own puts each in the sink as it comes, as `write_clip` does a coded frame.

use super::{micros, seal_clip, Camera, ClipInfo, Sink, Stage, BLOCK, WRITE_MAX};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use vsf::types::{BitPackedTensor, Tensor};
use vsf::visual::clip::{self, ClipCodec, ClipSpec, FrameMeta, Sum};

/// One frame of a stacked clip: when and how it was taken, how many exposures its sums hold and their levels, the sums packed (`[height, width]`), and the motion plane in a Motion clip.
pub struct PackedFrame {
    pub meta: FrameMeta,
    pub sum: Sum,
    pub samples: BitPackedTensor,
    pub motion: Option<BitPackedTensor>,
}

/// What a stacked clip is: its name; the time a frame stands for (its exposure, ns); whether every frame carries a motion plane; when it started, Eagle oscillations; the extent asked for, blocks.
#[derive(Debug, Clone)]
pub struct PackedMode {
    pub title: String,
    pub period_ns: u64,
    pub motion: bool,
    pub at: i64,
    pub blocks: u64,
}

/// Frames handed over and not yet written at most: the caller waits past this, never the camera (Lumis hands frames over from a thread of its own).
const QUEUE: usize = 1 << 1;

/// A stacked clip being recorded: start, hand frames, stop.
pub struct PackedRecorder {
    to_writer: Option<SyncSender<PackedFrame>>,
    writer: Option<JoinHandle<ClipInfo>>,
    written: Arc<AtomicU64>,
    ended: Arc<AtomicBool>,
}

impl PackedRecorder {
    /// Start a stacked clip of `cam`: the extent opened, generation 0 written, the writer started.
    pub fn start(cam: &Camera, mode: &PackedMode, mut sink: Box<dyn Sink>) -> Result<PackedRecorder, String> {
        let p = match cam.cfa.len() {
            4 => 2,
            16 => 4,
            n => return Err(format!("a mosaic of {n} cells: 4 (Bayer) or 16 (quad)")),
        };
        // The rate a frame each `period_ns`, as a reduced fraction: frames a second = 10^9 / period_ns. A period of 0 is the caller's mistake, said, not guessed past.
        if mode.period_ns == 0 {
            return Err("a stacked clip's frame period of 0 ns".into());
        }
        let g = gcd(1_000_000_000, mode.period_ns);
        let (num, den) = (1_000_000_000 / g, mode.period_ns / g);
        let (rate_num, rate_den) = (u32::try_from(num).map_err(|_| "a frame rate past 2^32")?, u32::try_from(den).map_err(|_| format!("a frame each {} ns: its rate's denominator past 2^32", mode.period_ns))?);
        let capture = vsf::visual::Capture { mode: "stacked".into(), make: cam.make.clone(), model: cam.model.clone(), sensor_module: cam.sensor_module.clone(), lens: cam.lens.clone(), f_number: cam.f_number, focal_mm: cam.focal_mm, ..Default::default() };
        let spec = ClipSpec {
            title: mode.title.clone(),
            rate_num,
            rate_den,
            period_ns: mode.period_ns,
            width: cam.width,
            height: cam.height,
            cfa: Tensor::new(vec![p, p], cam.cfa.clone()),
            codec: ClipCodec::Packed { motion: mode.motion },
            channel_names: vec!["R".into(), "G".into(), "B".into()],
            // One exposure's levels and its bits (a frame's own are in its section): the sensor's.
            black: vec![cam.black as f32; 3],
            white: vec![cam.white as f32; 3],
            depth: (16 - cam.white.leading_zeros()) as u8,
            orientation: cam.orientation,
            capture,
            characterization: cam.characterization.clone(),
            observer: cam.observer.clone(),
            at: Some(mode.at),
        };
        let (cw, head) = clip::Writer::new(&spec).map_err(|e| e.to_string())?;
        let got = sink.open(mode.blocks)?;
        let (to_writer, from) = sync_channel::<PackedFrame>(QUEUE);
        let written = Arc::new(AtomicU64::new(0));
        let ended = Arc::new(AtomicBool::new(false));
        let (w, e) = (written.clone(), ended.clone());
        let writer = std::thread::Builder::new().name("clip-packed".into()).spawn(move || write_packed(cw, head, sink, got, from, w, e)).map_err(|e| format!("the writer thread: {e}"))?;
        Ok(PackedRecorder { to_writer: Some(to_writer), writer: Some(writer), written, ended })
    }

    /// Hand a frame to the writer (waiting while `QUEUE` are behind): false once the clip has ended (its extent full, or a write failed), and the frame is not kept.
    pub fn frame(&self, f: PackedFrame) -> bool {
        !self.ended() && self.to_writer.as_ref().is_some_and(|t| t.send(f).is_ok())
    }

    /// Bytes written so far.
    pub fn written(&self) -> u64 {
        self.written.load(Ordering::Relaxed)
    }

    /// The clip ended on its own: its extent full, or a write failed ([`ClipInfo::error`] at `stop` says which).
    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed)
    }

    /// Every frame handed over written, the clip closed and sealed: its length, seal and why it ended early, if it did.
    pub fn stop(mut self) -> ClipInfo {
        drop(self.to_writer.take());
        match self.writer.take().map(|w| w.join()) {
            Some(Ok(info)) => info,
            _ => ClipInfo { stats: Default::default(), len: 0, hash: [0; 32], write_us: 0, error: Some("the writer thread ended without the clip".into()) },
        }
    }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// The writer: generation 0, then each frame as it comes (its head, its samples, the piece after, and in a Motion clip its motion plane and the last piece), then the closing generation and the seal (`seal_clip`). A frame that would not leave room for the close ends the clip there (`ended`).
fn write_packed(mut cw: clip::Writer, head: Vec<u8>, mut sink: Box<dyn Sink>, blocks: u64, from: Receiver<PackedFrame>, written: Arc<AtomicU64>, ended: Arc<AtomicBool>) -> ClipInfo {
    let mut st = Stage { buf: Vec::with_capacity(WRITE_MAX << 1), block: 0, us: 0, longest: 0 };
    let mut failed: Option<String> = st.put(&mut *sink, &head).err();
    let mut full = false;
    for f in from {
        if failed.is_some() || full {
            continue;
        }
        // This frame (its fields a few hundred bytes past its planes) and the closing generation (about 2^7 bytes a frame of index, a header) must fit what is left.
        let planes = f.samples.data.len() as u64 + f.motion.as_ref().map_or(0, |m| m.data.len() as u64);
        let need = planes + (1 << 12) + (cw.frames().len() as u64 + 1) * (1 << 7) + (1 << 14);
        if cw.end() + need > blocks * BLOCK as u64 {
            full = true;
        } else {
            let (t0, us0) = (micros(), st.us);
            st.longest = 0;
            match cw.packed_frame(&f.meta, &f.sum, &f.samples, f.motion.as_ref()) {
                Ok(pieces) => {
                    // The pieces around the planes: [0], the samples, [1], and in a Motion clip the motion plane and [2].
                    let mut r = Ok(());
                    for (k, piece) in pieces.iter().enumerate() {
                        r = r.and_then(|()| st.put(&mut *sink, piece));
                        match (k, &f.motion) {
                            (0, _) => r = r.and_then(|()| st.put(&mut *sink, &f.samples.data)),
                            (1, Some(m)) => r = r.and_then(|()| st.put(&mut *sink, &m.data)),
                            _ => {}
                        }
                    }
                    failed = r.err();
                }
                Err(e) => failed = Some(e.to_string()),
            }
            // Each frame's write on the process's clock (`micros`), its time in the sink and its longest single write: a camera frame Lumis did not take lines up against these (the coordinator, 2026-11-23: a vault commit may hold the kernel lock).
            println!("clip-packed: {} frames in, the last written from {} us to {} us: {} us in the sink, its longest write {} us", cw.frames().len(), t0, micros(), st.us - us0, st.longest);
        }
        written.store(st.block * BLOCK as u64, Ordering::Relaxed);
        if failed.is_some() || full {
            ended.store(true, Ordering::Relaxed);
        }
    }
    seal_clip(cw, st, &mut *sink, failed, full, &written)
}
