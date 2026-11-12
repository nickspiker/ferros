//! The bake (COMPOSITOR.md "Baked"; owner, 2026-11-12: "if the GPU touches it last and we precompute the tanh highlight colours for all elements then it's a straight thru render and a sqrt to get the gamma close (we'd bake the diff in the precomputed colours but that makes the last step a sqrt instead)").
//!
//! Every pixel the display composes is in the panel's own codes: VSF RGB working values (gamma 2, white at Q14) thru the roll-off (tanh in root space, white at its share of the peak), the ODT to the panel's primaries (E's gains in its row sums) and the panel's 1/2.19, as a fraction of the panel's top code in Q16 (65535 is code 255.996 of the 8-bit link, `b >> 8` the code, its low byte the fraction the dither spends). The owner's last step, the root, is in the bake too: the canvas has always held roots (gamma 2 working values, fluor's g2t codes), so compositing stays where it was, in root space after the roll-off, and the present is only the dither and the pack (neon.rs `present_pack`).
//!
//! What it costs: once per colour (`px`, `rgb8`: the theme's palette, the inks, the glows, at each draw op), once per image (`working_row`, `g2t_row`, `codes_row`: the icons as they are built, the wallpaper as it loads, the console's rows as they decode), per frame only for a window whose surface is not baked (format 0 or 1). A brightness change bakes everything again (the render thread's `rebake`, the windows' `EV_BAKE`); the owner, 2026-11-12: "I don't particularly plan on putting quick brightness changing in the UI".
//!
//! The parameters live in two slots, the one in use and the one the next change is built in, with a generation: a reader takes the slot in use for one draw or one row and never holds it across a change (`now`).

use crate::{neon, odt};
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// One brightness's bake.
pub struct Bake {
    /// The ODT's NEON tables (neon.rs `Odt`) for this white: the images' and the legacy surfaces' path.
    pub odt: neon::Odt,
    /// The roll-off's gain on a working value (white 1): atanh(√(white / peak)).
    pub k: f32,
    /// The ODT, panel rows by VSF columns (odt.rs `odt`).
    pub m: [[f32; 3]; 3],
    /// One over the panel's gamma.
    pub g: f32,
    /// Each 8-bit gamma 2 code's light (c / 255 the working value): the palette's (`rgb8`).
    l8: *const f32,
    /// √p (0 to 1 in `CODE_STEPS` steps, one past for the interpolation) to the baked code, √p^(2/γ) × 65472: the panel's code by the root of its light, which keeps the dark end's steps fine.
    code: *const f32,
    /// A 16-bit g2t code's light (f16 bits, the E peak at 2^14): tanh²(k · atanh(c / 2^16)) × 2^14, every code; fluor's pixels and the wallpaper's codes (`g2t_row`, `codes_row`).
    light: *mut u16,
    /// White (1, 1, 1) baked, Q16 a channel: the blend modes' unit (main.rs `fold`).
    pub white: [u32; 3],
}

/// The code table's steps.
const CODE_STEPS: usize = 1 << 12;
/// The baked code at the panel's peak: the old present's Q4 top (16368, 1023 × 16) times four, so a code means what it did.
pub const TOP: f32 = 65472.;

static mut SLOTS: [Option<Bake>; 2] = [None, None];
/// The slot in use.
static NOW: AtomicUsize = AtomicUsize::new(0);
/// Counts every bake set: a cache made under another generation is stale.
static GEN: AtomicU32 = AtomicU32::new(0);
/// The two slots' light tables (`Bake::light`), allocated once at start (2 × 128 KiB, before the heap's mark), and their palette and code tables (`Bake::l8`, `Bake::code`: 17 KiB each). On the heap, not in the `Bake`: the display's first thread has a 64 KiB stack, and a `Bake` built there with its tables in it ran past it (2026-11-12, the first start on the 8a: a permission fault at sp).
static mut LIGHTS: [usize; 2] = [0, 0];
static mut TABLES: [usize; 2] = [0, 0];

/// The bake in use. Its slot is rebuilt only by the second `set` after this one, so a reader holds it for a draw or a row, never across frames.
#[inline]
pub fn now() -> &'static Bake {
    // SAFETY: `set` fills a slot before it is published by `NOW`, and the first `set` runs at start before any reader (the render thread's start, before the workers and the console).
    unsafe { (*(&raw const SLOTS))[NOW.load(Ordering::Acquire)].as_ref().unwrap_unchecked() }
}

/// The bake before the one in use (its slot is the other, and stays as it was until the next `set`): what a re-bake undoes (main.rs `wall_rebake_rows`). At start, the one in use.
#[inline]
pub fn before() -> &'static Bake {
    // SAFETY: as `now`; a slot never filled (the first `set` only) falls back to the one in use.
    unsafe {
        let slots = &*(&raw const SLOTS);
        let i = NOW.load(Ordering::Acquire);
        slots[i ^ 1].as_ref().unwrap_or_else(|| slots[i].as_ref().unwrap_unchecked())
    }
}

/// The bake's generation: every `set` moves it.
#[inline]
pub fn generation() -> u32 {
    GEN.load(Ordering::Acquire)
}

/// A new bake for `white` and the E peak it rolls off into (main.rs `brightness`), built into the slot not in use and then made current. The render thread's, at start (before anything is drawn) and at a brightness change.
pub fn set(white: f32, peak_e: f32) {
    unsafe {
        if LIGHTS[0] == 0 {
            LIGHTS = [alloc::vec![0u16; 1 << 16].leak().as_mut_ptr() as usize, alloc::vec![0u16; 1 << 16].leak().as_mut_ptr() as usize];
            TABLES = [alloc::vec![0f32; 256 + CODE_STEPS + 2].leak().as_mut_ptr() as usize, alloc::vec![0f32; 256 + CODE_STEPS + 2].leak().as_mut_ptr() as usize];
        }
        let first = (*(&raw const SLOTS))[0].is_none();
        let s = if first { 0 } else { NOW.load(Ordering::Acquire) ^ 1 };
        let k = libm::atanh(libm::sqrt((white / peak_e).min(0.999) as f64)) as f32;
        let m = odt::odt();
        let g = 1. / odt::panel_gamma();
        let (l8, code) = core::slice::from_raw_parts_mut(TABLES[s] as *mut f32, 256 + CODE_STEPS + 2).split_at_mut(256);
        for (c, l) in l8.iter_mut().enumerate() {
            let t = libm::tanhf(k * c as f32 * (1. / 255.));
            *l = t * t;
        }
        for (i, e) in code.iter_mut().enumerate() {
            *e = odt::pow(i as f32 * (1. / CODE_STEPS as f32), 2. * g) * TOP;
        }
        let light = core::slice::from_raw_parts_mut(LIGHTS[s] as *mut u16, 1 << 16);
        for (c, e) in light.iter_mut().enumerate() {
            let t = libm::tanh(k as f64 * libm::atanh(c as f64 * (1. / 65536.)));
            *e = odt::f32_to_f16((t * t * odt::F16_SCALE) as f32);
        }
        // Made in its slot, not on the stack and moved there.
        let slot = &mut (*(&raw mut SLOTS))[s];
        *slot = Some(Bake { odt: odt::odt_tables(white, peak_e), k, m, g, l8: l8.as_ptr(), code: code.as_ptr(), light: light.as_mut_ptr(), white: [0; 3] });
        let b = slot.as_mut().unwrap_unchecked();
        let w = b.px([1., 1., 1.]);
        b.white = [w as u32 & 0xFFFF, (w >> 16) as u32 & 0xFFFF, (w >> 32) as u32 & 0xFFFF];
        NOW.store(s, Ordering::Release);
        GEN.fetch_add(1, Ordering::AcqRel);
    }
}

impl Bake {
    /// A panel light (the E peak at 1) to its baked code: thru the root, the table, linear between its steps. A light under 0 (a channel out of the panel's gamut) has no root: NaN all the way, which the cast makes code 0, as the NEON path's tables do. The cast saturates too (an f32 to an integer stops at the type's ends).
    #[inline]
    fn code(&self, p: f32) -> u64 {
        let r = libm::sqrtf(p) * CODE_STEPS as f32;
        // WHY the test: a light past the peak (a saturated colour beyond the panel's primaries, VSF red's panel red at 1.36) runs past the table's last step; its code is worked out whole, and the cast holds it at the top as the NEON path's saturating shift does.
        if r >= CODE_STEPS as f32 {
            return (odt::pow(p, self.g) * TOP) as u16 as u64;
        }
        let i = r as usize;
        let f = r - i as f32;
        // SAFETY: `set` allocated `CODE_STEPS + 2` entries here for the process; `i + 1` is at most `CODE_STEPS` (r is under `CODE_STEPS` past the test, and NaN casts to 0).
        let code = unsafe { core::slice::from_raw_parts(self.code, CODE_STEPS + 2) };
        (code[i] + (code[i + 1] - code[i]) * f) as u16 as u64
    }

    /// Panel lights (E's peak at 1) to a baked pixel's channels.
    #[inline]
    fn of_lights(&self, l: [f32; 3]) -> u64 {
        let m = &self.m;
        let p = |r: usize| m[r][0] * l[0] + m[r][1] * l[1] + m[r][2] * l[2];
        self.code(p(0)) | self.code(p(1)) << 16 | self.code(p(2)) << 32
    }

    /// A working colour (VSF RGB, gamma 2, white at 1.0, so a Q14 value over 16384) baked: its channels, α 0 (the caller's).
    pub fn px(&self, v: [f32; 3]) -> u64 {
        let l = v.map(|v| {
            let t = libm::tanhf(self.k * v);
            t * t
        });
        self.of_lights(l)
    }

    /// A G#RRGGBB colour (8-bit VSF RGB gamma 2 codes, white at 255: the theme's, the ops') baked: its channels, α 0.
    #[inline]
    pub fn rgb8(&self, c: u32) -> u64 {
        // SAFETY: `set` allocated 256 entries here for the process; an 8-bit index.
        let l8 = unsafe { core::slice::from_raw_parts(self.l8, 256) };
        self.of_lights([l8[(c >> 16 & 0xFF) as usize], l8[(c >> 8 & 0xFF) as usize], l8[(c & 0xFF) as usize]])
    }

    /// A row of working pixels baked in place, 16 at a time (`neon::bake16`), each pixel's fourth lane kept; a tail under 16 thru a padded block.
    pub fn working_row(&self, row: &mut [u64]) {
        let n = row.len();
        let mut x = 0;
        unsafe {
            while x + 16 <= n {
                neon::bake16(&self.odt, row.as_ptr().add(x), row.as_mut_ptr().add(x));
                x += 16;
            }
        }
        if x < n {
            let mut pad = [0u64; 16];
            pad[..n - x].copy_from_slice(&row[x..]);
            unsafe { neon::bake16(&self.odt, pad.as_ptr(), pad.as_mut_ptr()) };
            row[x..].copy_from_slice(&pad[..n - x]);
        }
    }

    /// A row of fluor's 16-bit pixels (`G2T16`: α in bits 48-63, each channel's premultiplied darkness of a g2t code, R in bits 32-47) baked into `dst`, as main.rs `working` decoded them: each channel's light over black is the code α − darkness, its light by the table; the fourth lane its transparency (bits 49-62, Q14), which is exactly the see-through bit (2^14 << 49 = 2^63) where α is 0. WHY the saturating subtract: the pixels may be another process's (a window's surface), and darkness ≤ α is fluor's invariant there, not one the display can prove; a pixel breaking it would index past the table. fluor's own pixels decode exactly.
    pub fn g2t_row(&self, src: &[u64], dst: &mut [u64]) {
        // WHY the min: the two rows are the caller's; only the pixels both hold are baked.
        let n = src.len().min(dst.len());
        unsafe { self.g2t_ptr(src.as_ptr(), dst.as_mut_ptr(), n) };
    }

    /// `g2t_row` over one buffer, in place.
    pub fn g2t_in_place(&self, buf: &mut [u64]) {
        unsafe { self.g2t_ptr(buf.as_ptr(), buf.as_mut_ptr(), buf.len()) };
    }

    /// `g2t_row`'s work on `n` pixels from `src` into `dst`, which may be the same: each block of 16 is read whole before it is written.
    unsafe fn g2t_ptr(&self, src: *const u64, dst: *mut u64, n: usize) {
        let light = unsafe { core::slice::from_raw_parts(self.light, 1 << 16) };
        let mut x = 0;
        while x < n {
            let m = (n - x).min(16);
            let (mut l, mut a) = ([0u16; 48], [0u16; 16]);
            for j in 0..m {
                let p = unsafe { *src.add(x + j) };
                let al = p >> 48;
                let ch = |sh: u32| light[al.saturating_sub(p >> sh & 0xFFFF) as usize];
                l[j] = ch(32);
                l[16 + j] = ch(16);
                l[32 + j] = ch(0);
                a[j] = ((16384 - (al >> 2)) << 1) as u16;
            }
            let mut out = [0u64; 16];
            unsafe {
                use core::arch::aarch64::*;
                let v = |i: usize| vld1q_u16(l.as_ptr().add(8 * i));
                neon::bake16_lights(&self.odt, [v(0), v(1), v(2), v(3), v(4), v(5)], vld1q_u16(a.as_ptr()), vld1q_u16(a.as_ptr().add(8)), out.as_mut_ptr());
                core::ptr::copy_nonoverlapping(out.as_ptr(), dst.add(x), m);
            }
            x += m;
        }
    }

    /// A row of opaque pixels whose lanes hold 16-bit g2t codes (the wallpaper as the loader puts it, red in bits 0-15) baked in place, α whole.
    pub fn codes_in_place(&self, row: &mut [u64]) {
        let light = unsafe { core::slice::from_raw_parts(self.light, 1 << 16) };
        let n = row.len();
        let mut x = 0;
        while x < n {
            let m = (n - x).min(16);
            let mut l = [0u16; 48];
            for j in 0..m {
                let p = row[x + j];
                l[j] = light[(p & 0xFFFF) as usize];
                l[16 + j] = light[(p >> 16 & 0xFFFF) as usize];
                l[32 + j] = light[(p >> 32 & 0xFFFF) as usize];
            }
            let mut out = [0u64; 16];
            unsafe {
                use core::arch::aarch64::*;
                let v = |i: usize| vld1q_u16(l.as_ptr().add(8 * i));
                let a = vdupq_n_u16(0xFFFF);
                neon::bake16_lights(&self.odt, [v(0), v(1), v(2), v(3), v(4), v(5)], a, a, out.as_mut_ptr());
            }
            row[x..x + m].copy_from_slice(&out[..m]);
            x += m;
        }
    }

    /// The tables this bake is undone with (`unbake_with`): each code's panel light (`to_p`, every code), and the working value of each root of a VSF light (`to_v`, its length less one steps over √l: atanh(√l) / k). About 65536 powers: a few ms.
    pub fn unbake_tables(&self, to_p: &mut [f32], to_v: &mut [u16]) {
        for (c, e) in to_p.iter_mut().enumerate() {
            *e = odt::pow(c as f32 * (1. / TOP), 1. / self.g);
        }
        let n = to_v.len() - 1;
        for (i, e) in to_v.iter_mut().enumerate() {
            // atanh runs to infinity at the peak: the roots past every working value (4.0) cast to the field's top.
            *e = (libm::atanhf(i as f32 / n as f32) / self.k * 16384.) as u16;
        }
    }

    /// The bake undone (a screen capture, op 30, tools/screencap: VSF RGB working values, as before the bake; the wallpaper at a brightness change, baked again after): each pixel's codes to panel light (`to_p`), the ODT inverted back to VSF light, its root unrolled (`to_v`), as working values with their fourth lane kept. Opaque composites, so their premultiplied codes are their colours. Approximate where the bake lost something for good: past the panel's gamut (a channel held at 0 or the top) and in the roll-off's last stops.
    pub fn unbake_with(&self, buf: &mut [u64], to_p: &[f32], to_v: &[u16]) {
        let n = to_v.len() - 1;
        let inv = invert(&self.m);
        for p in buf.iter_mut() {
            let q = [to_p[(*p & 0xFFFF) as usize], to_p[(*p >> 16 & 0xFFFF) as usize], to_p[(*p >> 32 & 0xFFFF) as usize]];
            let w = |r: usize| {
                let l = inv[r][0] * q[0] + inv[r][1] * q[1] + inv[r][2] * q[2];
                // A negative light (a colour the bake held at a channel's 0) has no root: NaN, which the cast makes index 0, black on that channel. WHY the min: the inverse of codes the bake rounded or held at the top can land past the peak's light; its root is past the table's end, and it is the top.
                to_v[((libm::sqrtf(l) * n as f32) as usize).min(n)] as u64
            };
            *p = (*p & 0xFFFF << 48) | w(0) | w(1) << 16 | w(2) << 32;
        }
    }
}

/// A 3 × 3 inverse by cofactors (the ODT is well conditioned: its determinant is about 0.94 on both panels).
fn invert(m: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let c = |r: usize, k: usize| {
        let (r0, r1, k0, k1) = ((r + 1) % 3, (r + 2) % 3, (k + 1) % 3, (k + 2) % 3);
        m[r0][k0] * m[r1][k1] - m[r0][k1] * m[r1][k0]
    };
    let det = m[0][0] * c(0, 0) + m[0][1] * c(0, 1) + m[0][2] * c(0, 2);
    core::array::from_fn(|r| core::array::from_fn(|k| c(k, r) / det))
}
