//! A stacked CLIP: lumis_core's CLIP at an exposure of two of the camera's frames or more (a time-lapse), every completed exposure one frame of its exact sums (the owner, 2026-11-23: "our nice bitpacked RAW frames for video that probably is a time lapse, thus has a bunch of integrations per frame"). The integrator builds each frame in a buffer set aside at RECORD (lumis_core `set_clip_recording`, `take_clip_frame`); the camera thread stamps it with its last frame's time and hands it to this clip's thread, never waiting; the thread packs the sums at the width they need, gives the buffer back (`give_clip_buffer`) and writes the frame into the person's hold (ferros_clip `packed::PackedRecorder`, an item of kind RAW video).

use ferros_clip::packed::{PackedFrame, PackedRecorder};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use vsf::types::BitPackedTensor;
use vsf::visual::clip::{FrameMeta, Levels, Sum};

/// A completed exposure for the clip's thread: the integrator's frame and the camera's facts of the last frame in it (its time, by the driver's clock, Eagle ns; its shutter, gain and focus).
pub struct Exposure {
    pub frame: lumis_core::ClipFrame,
    pub eagle_ns: i64,
    pub shutter_ns: u64,
    pub gain: u32,
    pub focus: u32,
}

/// Exposures handed over and not yet packed at most: past that one is passed over (the camera thread never waits), its buffer given back.
const QUEUE: usize = 1;

/// What the clip's thread says when the clip ends: its frames written, its bytes, and why it ended early if it did.
pub struct Ended {
    pub frames: u64,
    pub bytes: u64,
    pub error: Option<String>,
}

/// A stacked clip recording: the camera thread's side.
pub struct Stack {
    to_thread: Option<SyncSender<Exposure>>,
    /// Buffers packed, back for the integrator.
    pub back: Receiver<Vec<u64>>,
    thread: Option<JoinHandle<Ended>>,
    /// The frames written so far (the thread counts them).
    pub written: Arc<std::sync::atomic::AtomicU64>,
    /// Exposures the thread had no room for (it was still packing the one before).
    pub passed_over: u64,
    /// The camera's count of the last frame taken while it records, and its frames the driver wrote but Lumis did not take: since the last exposure handed over, and in all (the coordinator, 2026-11-23: a frame not taken is light no exposure summed, its exposure spanning a frame more).
    pub last_count: Option<u32>,
    pub skipped: u64,
    pub skipped_total: u64,
    pub name: String,
    pub cam: String,
    /// The item's handle and card as the sink fills them (kept with the clip, as a paced clip's are).
    pub _receipt: Arc<Mutex<ferros_clip::items::Receipt>>,
    pub began: std::time::Instant,
}

impl Stack {
    /// The stacked clip started on `rec` (its extent open, generation 0 written): its thread started.
    pub fn new(rec: PackedRecorder, name: String, cam: String, receipt: Arc<Mutex<ferros_clip::items::Receipt>>) -> Result<Stack, String> {
        let (to_thread, from) = sync_channel::<Exposure>(QUEUE);
        let (give, back) = std::sync::mpsc::channel::<Vec<u64>>();
        let written = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let w = written.clone();
        let thread = std::thread::Builder::new().name("lumis-stack".into()).spawn(move || pack_and_write(rec, from, give, w)).map_err(|e| format!("no thread for the clip ({e})"))?;
        Ok(Stack { to_thread: Some(to_thread), back, thread: Some(thread), written, passed_over: 0, last_count: None, skipped: 0, skipped_total: 0, name, cam, _receipt: receipt, began: std::time::Instant::now() })
    }

    /// A completed exposure to the thread, never waited for: when it is still packing the one before, this one is passed over and its buffer comes back for the integrator.
    pub fn hand(&mut self, e: Exposure) -> Option<Vec<u64>> {
        let Some(t) = self.to_thread.as_ref() else {
            self.passed_over += 1;
            return Some(e.frame.sums);
        };
        match t.try_send(e) {
            Ok(()) => None,
            Err(TrySendError::Full(e) | TrySendError::Disconnected(e)) => {
                self.passed_over += 1;
                Some(e.frame.sums)
            }
        }
    }

    /// The clip ended on its own (its thread done: its space full, or a write failed): `end` says why.
    pub fn ended(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }

    /// Every exposure handed over packed and written, the clip closed and sealed (`PackedRecorder::stop`).
    pub fn end(mut self) -> Ended {
        drop(self.to_thread.take());
        match self.thread.take().map(|t| t.join()) {
            Some(Ok(e)) => e,
            _ => Ended { frames: 0, bytes: 0, error: Some("the clip's thread ended without it".into()) },
        }
    }
}

/// The clip's thread: each exposure's sums packed at the width they need (their peak's, or the white's), its buffer given back, the frame written; at the end the clip closed and sealed.
fn pack_and_write(rec: PackedRecorder, from: Receiver<Exposure>, give: std::sync::mpsc::Sender<Vec<u64>>, written: Arc<std::sync::atomic::AtomicU64>) -> Ended {
    let mut frames = 0u64;
    for e in from {
        let f = &e.frame;
        let t0 = ferros_clip::micros();
        let peak = f.sums.iter().copied().max().unwrap_or(0).max(f.white);
        let samples = BitPackedTensor::pack((64 - peak.leading_zeros()) as u8, vec![f.height, f.width], &f.sums);
        let t1 = ferros_clip::micros();
        let meta = FrameMeta { index: frames, at: ferros_clip::eagle_osc(e.eagle_ns as u64), exposure_s: e.shutter_ns as f32 / 1e9, gain: e.gain as f32 / 1024., focus: e.focus };
        let sum = Sum { exposures: f.exposures as u32, levels: Levels { black: vec![f.black as f32; 3], white: vec![f.white as f32; 3] }, motion: None };
        let _ = give.send(e.frame.sums);
        if !rec.frame(PackedFrame { meta, sum, samples, motion: None }) {
            break;
        }
        // Its packing and its hand-over to the writer on the process's clock (ferros_clip `micros`), beside the camera's turns and the writer's writes.
        println!("lumis-stack: frame {frames} packed from {t0} us to {t1} us, handed to the writer at {} us", ferros_clip::micros());
        frames += 1;
        written.store(frames, std::sync::atomic::Ordering::Relaxed);
    }
    let info = rec.stop();
    Ended { frames, bytes: info.len, error: info.error }
}
