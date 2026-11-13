//! camreplay: a development stand-in for the camera driver (docs/CAMERA.md "The endpoint"), serving one recorded frame of the Pro's main camera as the driver will serve live ones.
//! It exists to check Lumis's preview, its controls and the endpoint on a device before the driver does: the list names the camera as a recording, every frame is the same frame, and the controls are taken and change nothing (a recording has no sensor to set).
//! Spawned by hand only: `ferros-bridge spawn camreplay.bin --name camreplay --serve camera` (grant slot 1). Built with CAMREPLAY_FRAME naming a frame pulled from the Pro (tools/camera-pull.sh); the frame never enters the repository.

use std::time::{Duration, Instant};

include!(concat!(env!("OUT_DIR"), "/idt.rs"));

/// The recording: 2040 x 1536 samples of 16 bits, the main camera's 4080 x 3072 readout binned 2 x 2 per colour (build.rs).
static FRAME: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/frame.bin"));
const W: u16 = 2040;
const H: u16 = 1536;
/// G B / R G, two bits a sample, top left first (0 R, 1 G, 2 B).
const CFA: u8 = 1 | 2 << 2 | 0 << 4 | 1 << 6;
/// The main camera's frame period and integration as measured (docs/oracle/CAMERA.md "Many runs in one boot": 67.8 ms a frame, 68.8 ms integration).
const PERIOD: Duration = Duration::from_micros(67_800);
const EXPOSURE_NS: u64 = 68_800_000;
const GAIN: u64 = 1 << 10;
const FOCUS: u64 = 0x3800;

const SERVE: u64 = 1;

mod sys {
    use core::arch::asm;
    pub const ERR: u64 = u64::MAX;
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

/// The list: one camera, named as what it is.
fn list() -> Vec<u8> {
    use vsf::VsfType;
    let mut s = vsf::file_format::VsfSection::new("camera");
    let mut put = |name: &str, values: Vec<VsfType>| {
        let mut f = vsf::file_format::VsfField::new(name);
        f.values = values;
        s.fields.push(f);
    };
    let u = |v: u64| vec![VsfType::u(v as usize, false)];
    put("id", u(0));
    put("name", vec![VsfType::a(vsf::Ascii::lit("a recorded frame of the Pro's main camera (development)"))]);
    put("facing", u(0));
    put("width", u(4080));
    put("height", u(3072));
    put("cfa", u(CFA as u64));
    put("black", u(64));
    put("white", u(1023));
    put("rotation", u(1));
    put("idt", IDT.iter().map(|v| VsfType::f5(*v)).collect());
    put("sensitivity", SENSITIVITY.iter().map(|v| VsfType::f5(*v)).collect());
    put("exposure_min", u(EXPOSURE_NS));
    put("exposure_max", u(EXPOSURE_NS));
    put("gain_min", u(GAIN));
    put("gain_max", u(GAIN));
    put("focus_min", u(FOCUS));
    put("focus_max", u(FOCUS));
    vsf::VsfBuilder::new().add_section_direct(s).build().expect("the list builds")
}

fn main() {
    println!("camreplay: serving `camera` with one recorded frame ({W} x {H} samples, {} bytes), a development stand-in for the driver", FRAME.len());
    let doc = list();
    let mut frame = vec![0u8; 64 + FRAME.len()];
    frame[64..].copy_from_slice(FRAME);
    frame[4..6].copy_from_slice(&W.to_le_bytes());
    frame[6..8].copy_from_slice(&H.to_le_bytes());
    frame[8] = 2;
    frame[9] = CFA;
    frame[10..12].copy_from_slice(&64u16.to_le_bytes());
    frame[12..14].copy_from_slice(&1023u16.to_le_bytes());
    frame[16..24].copy_from_slice(&EXPOSURE_NS.to_le_bytes());
    frame[24..28].copy_from_slice(&(GAIN as u32).to_le_bytes());
    frame[28..32].copy_from_slice(&(FOCUS as u32).to_le_bytes());
    let mut buf = [0u8; 64];
    let mut token = [0u8; 32];
    let mut count = 0u32;
    let mut next = Instant::now();
    let (mut served, mut since) = (0u32, Instant::now());
    loop {
        let n = sys::recv(SERVE, &mut buf, &mut token);
        if n == sys::ERR || n == 0 {
            println!("camreplay: the receive failed; ending");
            return;
        }
        let r = match buf[0] {
            0 => sys::reply(&token, &doc),
            1 => sys::reply(&token, &[if buf[1] == 0 { 0 } else { 1 }]),
            2 => {
                // One frame a measured period, as the sensor sends them.
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                }
                next = Instant::now() + PERIOD;
                count += 1;
                frame[0..4].copy_from_slice(&count.to_le_bytes());
                served += 1;
                if since.elapsed() >= Duration::from_secs(8) {
                    println!("camreplay: {served} frames in {} ms", since.elapsed().as_millis());
                    (served, since) = (0, Instant::now());
                }
                sys::reply(&token, &frame)
            }
            3 | 4 => sys::reply(&token, &[0]),
            op => {
                println!("camreplay: op {op} is not one of the endpoint's");
                sys::reply(&token, &[1])
            }
        };
        if r == sys::ERR {
            println!("camreplay: a reply was refused (its caller ended?)");
        }
    }
}
