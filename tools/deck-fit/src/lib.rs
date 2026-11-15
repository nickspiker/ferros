//! The map's camera geometry on the host: payloads/map/src/camera.rs itself, tested.
//! `cargo test --release --manifest-path tools/deck-fit/Cargo.toml -- --nocapture` prints what the fit before 2026-11-15 lost on the same layouts.

#[path = "../../../payloads/map/src/camera.rs"]
#[allow(dead_code)]
pub mod camera;

#[cfg(test)]
mod tests {
    use super::camera::{self, Disc, Room, Spiral};
    use core::f64::consts::{LN_2, PI};

    /// The map's turn, Q14.
    const ONE: f64 = 16384.;

    /// A screen as the map sees it: width, height, and the big circle (ferros_sys::active_circle: tangent to the camera cutout's bottom and the screen's bottom), its centre's row and its radius truncated to whole pixels as `Map::arrived` takes them.
    struct Screen {
        w: i64,
        h: i64,
        ay: i64,
        ar: i64,
    }
    fn screen(w: i64, h: i64, cut: f32) -> Screen {
        let r = (h as f32 - cut) * 0.5;
        Screen { w, h, ay: (cut + r) as i64, ar: r as i64 }
    }
    /// The 8a (akita) and the Pro (husky), their cutouts' bottoms (ferros_sys CUTOUT_BOTTOM_AKITA, CUTOUT_BOTTOM).
    fn screens() -> [Screen; 2] {
        [screen(1080, 2400, 101.8303), screen(1344, 2992, 118.5610)]
    }
    fn room(s: &Screen) -> Room {
        Room { half_w: s.w as f64 * 0.5, radius: s.ar as f64 }
    }

    /// A layout as the map keeps it: centres in world units ×256 (any sub-unit bits), radii in whole world units.
    struct Layout {
        xs: Vec<i64>,
        ys: Vec<i64>,
        rs: Vec<i64>,
    }
    impl Layout {
        /// The discs the map's `deck_fit_at` hands the fit: the scene's whole world units, as the display draws them.
        fn discs(&self) -> Vec<Disc> {
            (0..self.xs.len()).map(|i| (((self.xs[i] >> 8) << 8) as f64, ((self.ys[i] >> 8) << 8) as f64, (self.rs[i] << 8) as f64)).collect()
        }
    }

    struct Rng(u64);
    impl Rng {
        fn unit(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// Random layouts of 2 to 9 circles that do not overlap, as a person's deck might be after dragging: 40 to 240 world units across a radius, within 600 of the origin and a random offset from it (the plane is panned anywhere).
    fn layouts(count: usize) -> Vec<Layout> {
        let mut g = Rng(0x6465_636b_2066_6974);
        let mut out = Vec::new();
        while out.len() < count {
            let n = 2 + (g.unit() * 8.) as usize;
            let (ox, oy) = ((g.unit() - 0.5) * 8000., (g.unit() - 0.5) * 8000.);
            let mut l = Layout { xs: vec![], ys: vec![], rs: vec![] };
            let mut tries = 0;
            while l.xs.len() < n && tries < 1 << 12 {
                tries += 1;
                let (x, y, r) = ((g.unit() - 0.5) * 1200. + ox, (g.unit() - 0.5) * 1200. + oy, 40. + g.unit() * 200.);
                let r = r as i64;
                if (0..l.xs.len()).all(|j| {
                    let (dx, dy) = (x - l.xs[j] as f64 / 256., y - l.ys[j] as f64 / 256.);
                    (dx * dx + dy * dy).sqrt() >= (r + l.rs[j]) as f64
                }) {
                    l.xs.push((x * 256.) as i64);
                    l.ys.push((y * 256.) as i64);
                    l.rs.push(r);
                }
            }
            out.push(l);
        }
        out
    }

    /// The scale (world ×256 a pixel) the circles need with the room's centre on the world point (wx, wy) at the turn (c, s): the definition, straight.
    fn need(discs: &[Disc], c: f64, s: f64, room: Room, wx: f64, wy: f64) -> f64 {
        let (cu, cv) = (c * wx - s * wy, s * wx + c * wy);
        discs.iter().fold(0f64, |g, &(x, y, r)| {
            let (u, v) = (c * x - s * y - cu, s * x + c * y - cv);
            g.max((u.abs() + r) / room.half_w).max(((u * u + v * v).sqrt() + r) / room.radius)
        })
    }

    /// Brute force: the least scale over centres by a coarse grid over a box well past the circles, then shrinking pattern steps from its best (the scale is convex in the centre, so the steps reach its least).
    fn brute(discs: &[Disc], c: f64, s: f64, room: Room) -> f64 {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &(x, y, r) in discs {
            x0 = x0.min(x - r);
            y0 = y0.min(y - r);
            x1 = x1.max(x + r);
            y1 = y1.max(y + r);
        }
        let n = 1 << 6;
        let (mut bx, mut by, mut bg) = (0., 0., f64::MAX);
        for i in 0..=n {
            for j in 0..=n {
                let (x, y) = (x0 + (x1 - x0) * i as f64 / n as f64, y0 + (y1 - y0) * j as f64 / n as f64);
                let g = need(discs, c, s, room, x, y);
                if g < bg {
                    (bx, by, bg) = (x, y, g);
                }
            }
        }
        let mut step = (x1 - x0).max(y1 - y0) / n as f64;
        while step > 1e-6 {
            let mut moved = false;
            for k in 0..16 {
                let a = PI * k as f64 / 8.;
                let (x, y) = (bx + step * a.cos(), by + step * a.sin());
                let g = need(discs, c, s, room, x, y);
                if g < bg {
                    (bx, by, bg) = (x, y, g);
                    moved = true;
                }
            }
            if !moved {
                step *= 0.5;
            }
        }
        bg
    }

    fn q14(a: f64) -> (f64, f64) {
        ((a.cos() * ONE).round(), (a.sin() * ONE).round())
    }

    /// The fit before 2026-11-15: the room's centre on the middle of the circles' extent, both ways.
    fn old_fit(discs: &[Disc], c: f64, s: f64, room: Room) -> f64 {
        let (mut u0, mut v0, mut u1, mut v1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &(x, y, r) in discs {
            let (u, v) = (c * x - s * y, s * x + c * y);
            u0 = u0.min(u - r);
            v0 = v0.min(v - r);
            u1 = u1.max(u + r);
            v1 = v1.max(v + r);
        }
        let (um, vm) = ((u0 + u1) * 0.5, (v0 + v1) * 0.5);
        let d = c * c + s * s;
        (1u64 << 24) as f64 / need(discs, c, s, room, (c * um + s * vm) / d, (c * vm - s * um) / d)
    }

    /// The fit's zoom is the best any centre gives, to a millionth, at every turn; never less than the old fit's; and the centre it says gives that zoom.
    #[test]
    fn fit_is_the_best_centre() {
        let (mut short, mut worst, mut n) = (0, 0f64, 0);
        for sc in screens() {
            let room = room(&sc);
            for l in layouts(1 << 6) {
                let discs = l.discs();
                for k in 0..1 << 4 {
                    let (c, s) = q14(PI * k as f64 / 16. + 0.1);
                    let (c, s) = (c / ONE, s / ONE);
                    let (wx, wy, z) = camera::fit_at(&discs, c, s, room);
                    let best = (1u64 << 24) as f64 / brute(&discs, c, s, room);
                    assert!(z >= best * (1. - 1e-6), "fit {z} under the best {best}");
                    let at = (1u64 << 24) as f64 / need(&discs, c, s, room, wx, wy);
                    assert!((at / z - 1.).abs() < 1e-9, "the centre gives {at}, the fit says {z}");
                    let old = old_fit(&discs, c, s, room);
                    assert!(z >= old * (1. - 1e-9), "fit {z} under the old {old}");
                    n += 1;
                    if old < z * (1. - 1e-4) {
                        short += 1;
                    }
                    worst = worst.max(1. - old / z);
                }
            }
        }
        println!("the old fit's centre: short at {short} of {n} turns, by up to {:.2}% of the zoom", worst * 100.);
    }

    /// The circles on the glass as the display draws them (display main.rs, op 14: whole world units, Q14 turn, 16.16 zoom, centres on whole pixels, radius in 256ths): every one within the width and the big circle, the outermost touching, to a pixel, with the map's camera made as `deck_fit_at` makes it.
    #[test]
    fn circles_land_in_the_room_on_the_glass() {
        for sc in screens() {
            let room = room(&sc);
            for l in layouts(1 << 6) {
                let discs = l.discs();
                for k in 0..1 << 3 {
                    let (cq, sq) = q14(PI * k as f64 / 8. + 0.3);
                    let (c, s) = (cq as i64, sq as i64);
                    let (wx, wy, z) = camera::fit_at(&discs, cq / ONE, sq / ONE, room);
                    let z = z as i64;
                    // Cam::delta(0, h/2 − ay): the panel's centre off the big circle's.
                    let dy = sc.h / 2 - sc.ay;
                    let (cx, cy) = (wx.round() as i64 + ((s * dy) << 10) / z, wy.round() as i64 + ((c * dy) << 10) / z);
                    let (wq, hq) = (sc.w << 7, sc.h << 7);
                    let mut touch = f64::MAX;
                    for i in 0..l.xs.len() {
                        let (dx, dy) = (((l.xs[i] >> 8) << 8) - cx, ((l.ys[i] >> 8) << 8) - cy);
                        let sx = ((((c * dx - s * dy) >> 14) * z / 65536 + wq) & !255) | 128;
                        let sy = ((((s * dx + c * dy) >> 14) * z / 65536 + hq) & !255) | 128;
                        let sr = l.rs[i] * z >> 8;
                        let (px, py, pr) = (sx as f64 / 256., sy as f64 / 256., sr as f64 / 256.);
                        let across = room.half_w - ((px - sc.w as f64 * 0.5).abs() + pr);
                        let (ex, ey) = (px - sc.w as f64 * 0.5, py - sc.ay as f64);
                        let round = room.radius - ((ex * ex + ey * ey).sqrt() + pr);
                        assert!(across > -1. && round > -1., "a circle past the room: {across} px across, {round} px round");
                        touch = touch.min(across).min(round);
                    }
                    assert!(touch < 1., "nothing touches: {touch} px of air");
                }
            }
        }
    }

    /// The turn: within `TURN_TIE` of the best zoom a dense search finds (4096 turns across the half turn); the camera's own turn kept when it fits as well; and what the turn before 2026-11-15 lost (x⁴ / (1 + x⁴) of the way, x the gain over an eighth).
    #[test]
    fn turn_is_the_best() {
        let reach = |z: f64| z;
        let (mut worst_old, mut sum_old, mut turn_left, mut n, mut short, mut spent) = (0f64, 0f64, 0f64, 0, 0, 0u128);
        for sc in screens() {
            let room = room(&sc);
            for l in layouts(1 << 5) {
                let discs = l.discs();
                let fit = |a: f64| {
                    let (c, s) = q14(a);
                    camera::fit_at(&discs, c / ONE, s / ONE, room).2 * c.hypot(s) / ONE
                };
                let m = 1 << 12;
                let mut dense = 0f64;
                for j in 0..m {
                    dense = dense.max(fit(PI * j as f64 / m as f64));
                }
                for a0d in [0., 17., 45., 90., 133.] {
                    let a0 = (a0d as f64).to_radians();
                    let t0 = std::time::Instant::now();
                    let (a, z, best) = camera::best_turn(a0, fit, reach);
                    spent = spent.max(t0.elapsed().as_micros());
                    assert!(z >= dense * (1. - camera::TURN_TIE) * (1. - 1e-6), "turn {a} gives {z}, a dense search {dense}");
                    // The Q14 turn steps the angle by up to 3·10⁻⁵ of a radian: at a peak that is a kink, a few parts in a million of the zoom.
                    assert!(best >= dense * (1. - 1e-5), "best {best} under the dense search's {dense}");
                    assert!((fit(a) / z - 1.).abs() < 1e-12);
                    let (_, z_kept, _) = camera::best_turn(a, fit, reach);
                    assert!(z_kept >= z * (1. - 1e-12), "from the turn it chose, it chose again worse");
                    // The old swipe: x⁴ / (1 + x⁴) of the way to the best turn, with the old centre.
                    let old = |a: f64| {
                        let (c, s) = q14(a);
                        old_fit(&discs, c / ONE, s / ONE, room) * c.hypot(s) / ONE
                    };
                    let (ob, _, _) = camera::best_turn(a0, old, reach);
                    let x = (old(ob) / old(a0)).ln() * 8.;
                    let x4 = x * x * x * x;
                    let left = (1. - x4 / (1. + x4)) * (ob - a0);
                    let lands = old(ob - left);
                    n += 1;
                    if lands < dense * (1. - 1e-3) {
                        short += 1;
                        turn_left += left.abs().to_degrees();
                    }
                    worst_old = worst_old.max(1. - lands / dense);
                    sum_old += 1. - lands / dense;
                }
            }
        }
        println!("the longest search took {spent} us on this host");
        println!("the old swipe: short of the best by over a thousandth at {short} of {n} swipes, by {:.1}% of the zoom on average and up to {:.1}%; those stopped {:.1} degrees short of its own best turn on average", sum_old / n as f64 * 100., worst_old * 100., turn_left / short as f64);
    }

    /// The map's own camera (`Cam`): centre world ×256, zoom 16.16, turn Q14.
    #[derive(Debug, PartialEq, Clone, Copy)]
    struct Cam {
        x: i64,
        y: i64,
        z: i64,
        c: i64,
        s: i64,
    }

    fn isqrt(v: i64) -> i64 {
        if v <= 0 {
            return 0;
        }
        let (mut x, mut y) = (v, (v + 1) / 2);
        while y < x {
            x = y;
            y = (x + v / x) / 2;
        }
        x
    }

    /// `Map::zoom_limits`, as the map has it.
    fn zoom_limits(sc: &Screen, rs: &[i64]) -> (i64, i64) {
        let sum_r2: i64 = rs.iter().map(|r| r * r).sum();
        let r_min = *rs.iter().min().unwrap();
        let den = (sum_r2 * 804 * 256 / 256).max(1);
        let z_out = isqrt((sc.w * sc.h / den) << 32).max(1);
        let z_in = (sc.w.min(sc.h) << 16) / (2 * r_min);
        (z_out.min(z_in), z_in.max(z_out))
    }

    /// `Map::deck_fit_at` and `Map::deck_turned`, step for step, from the camera `from`; the old pair (the extent's middle, x⁴ / (1 + x⁴) of the way) beside them.
    fn deck_turned(sc: &Screen, l: &Layout, from: Cam, old: bool) -> (Cam, f64) {
        let discs = l.discs();
        let room = room(sc);
        let (lo, hi) = zoom_limits(sc, &l.rs);
        let fit_at = |c: i64, s: i64| {
            let (c0, s0) = (c as f64 / ONE, s as f64 / ONE);
            let (wx, wy, z) = if old {
                let z = old_fit(&discs, c0, s0, room);
                let (mut u0, mut v0, mut u1, mut v1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
                for &(x, y, r) in &discs {
                    let (u, v) = (c0 * x - s0 * y, s0 * x + c0 * y);
                    (u0, v0, u1, v1) = (u0.min(u - r), v0.min(v - r), u1.max(u + r), v1.max(v + r));
                }
                let (um, vm) = ((u0 + u1) * 0.5, (v0 + v1) * 0.5);
                (c0 * um + s0 * vm, c0 * vm - s0 * um, z)
            } else {
                camera::fit_at(&discs, c0, s0, room)
            };
            let z_cam = (z as i64).clamp(lo, hi);
            let dy = sc.h / 2 - sc.ay;
            (Cam { x: wx.round() as i64 + ((s * dy) << 10) / z_cam, y: wy.round() as i64 + ((c * dy) << 10) / z_cam, z: z_cam, c, s }, z)
        };
        let a0 = (from.s as f64).atan2(from.c as f64);
        let turn = |a: f64| ((a.cos() * ONE).round() as i64, (a.sin() * ONE).round() as i64);
        let fit = |a: f64| {
            let (c, s) = turn(a);
            let z = fit_at(c, s).1;
            if old { z } else { z * (c as f64).hypot(s as f64) / ONE }
        };
        let hi = hi as f64;
        let reach = |z: f64| {
            let a = z / hi;
            let a4 = a * a * a * a;
            z / (1. + a4 * a4).powf(1. / 8.)
        };
        let (a, z, best) = camera::best_turn(a0, fit, reach);
        let a = if old {
            let x = (reach(best) / reach(fit(a0))).ln() * 8.;
            let x4 = x * x * x * x;
            a0 + x4 / (1. + x4) * (a - a0)
        } else {
            a
        };
        let (c, s) = turn(a);
        let _ = z;
        (fit_at(c, s).0, fit(a))
    }

    /// The owner's 8a on 2026-11-15 (the deck as she keeps it, four circles): the swipe the device logged ("map: deck: from ... circles ...") lands where the device landed, and what the swipe before would have done.
    #[test]
    fn the_8a_swipe() {
        let sc = screens().into_iter().next().unwrap();
        let l = Layout { xs: vec![633055, 690524, 782330, 806152], ys: vec![-527450, -663945, -550807, -412226], rs: vec![343, 229, 240, 301] };
        let from = Cam { x: 727545, y: -491321, z: 1 << 16, c: -14506, s: -7616 };
        let (to, z) = deck_turned(&sc, &l, from, false);
        // The device: "landing at x A#729075 y A#-493147 zoom G#FE3B turn A#-12530 A#-10557".
        assert_eq!(to, Cam { x: 729075, y: -493147, z: 0xFE3B, c: -12530, s: -10557 });
        let (old, oz) = deck_turned(&sc, &l, from, true);
        let deg = |c: Cam| (c.s as f64).atan2(c.c as f64).to_degrees();
        println!("the 8a's deck: from {:.1} degrees; now {:.1} degrees at zoom {:.0}; the old swipe {:.1} degrees at zoom {:.0} ({:.1}% smaller)", deg(from), deg(to), z, deg(old), oz, (1. - oz / z) * 100.);
        // On the glass, thru the display's transform: each circle's air to the width's edge and to the big circle's, in pixels.
        let (wq, hq) = (sc.w << 7, sc.h << 7);
        for i in 0..4 {
            let (dx, dy) = (((l.xs[i] >> 8) << 8) - to.x, ((l.ys[i] >> 8) << 8) - to.y);
            let sx = ((((to.c * dx - to.s * dy) >> 14) * to.z / 65536 + wq) & !255) | 128;
            let sy = ((((to.s * dx + to.c * dy) >> 14) * to.z / 65536 + hq) & !255) | 128;
            let (px, py, pr) = (sx as f64 / 256., sy as f64 / 256., (l.rs[i] * to.z >> 8) as f64 / 256.);
            let (ex, ey) = (px - sc.w as f64 * 0.5, py - sc.ay as f64);
            println!("  circle {i}: centre ({px:.1}, {py:.1}) radius {pr:.1}: {:.1} px to the width's edge, {:.1} px to the big circle's", sc.w as f64 * 0.5 - ex.abs() - pr, sc.ar as f64 - (ex * ex + ey * ey).sqrt() - pr);
        }
        // And again from where it landed: nothing to gain, nothing turned.
        let (again, _) = deck_turned(&sc, &l, to, false);
        assert_eq!(again, to);
    }

    /// A camera as a complex similarity: the world point under screen offset f is x + f·q, q = e^(−(L + iθ)).
    fn under(x: (f64, f64), l: f64, a: f64, f: (f64, f64)) -> (f64, f64) {
        let m = (-l).exp();
        let q = (m * a.cos(), -m * a.sin());
        (x.0 + f.0 * q.0 - f.1 * q.1, x.1 + f.0 * q.1 + f.1 * q.0)
    }

    /// A flight from rest, frame by frame as `Map::step_cam` steps it: the zoom and the turn left in one proportion every frame, the one screen point both cameras show alike showing the same world point every frame, and the camera at the target at the end.
    #[test]
    fn flight_is_one_spiral() {
        // From a camera to the deck's fit: a 50 degree turn, the zoom halved, the centre far off.
        let (x0, l0, a0) = ((310_000., -95_000.), 0.4, 1.1);
        let (x1, l1, a1) = ((-120_000., 260_000.), 0.4 - LN_2, 1.1 - 50f64.to_radians());
        let d = (x0.0 - x1.0, x0.1 - x1.1);
        let (sp, po, pv) = Spiral::start(d, (0., 0.), l0 - l1, a0 - a1, 0., 0.);
        assert!(po.0.abs() < 1e-6 && po.1.abs() < 1e-6 && pv == (0., 0.));
        // The pivot: x₀ + f·q₀ = x₁ + f·q₁.
        let q = |l: f64, a: f64| ((-l).exp() * a.cos(), -(-l).exp() * a.sin());
        let (q0, q1) = (q(l0, a0), q(l1, a1));
        let m = (q0.0 - q1.0, q0.1 - q1.1);
        let mm = m.0 * m.0 + m.1 * m.1;
        let dx = (x1.0 - x0.0, x1.1 - x0.1);
        let f = ((dx.0 * m.0 + dx.1 * m.1) / mm, (dx.1 * m.0 - dx.0 * m.1) / mm);
        let p = under(x0, l0, a0, f);
        let off = [po.0, po.1, l0 - l1, a0 - a1];
        for t in 1..1 << 6 {
            let s: Vec<(f64, f64)> = (0..4).map(|i| camera::spring_at(off[i], 0., t, LN_2)).collect();
            assert!((s[2].0 / off[2] - s[3].0 / off[3]).abs() < 1e-12, "zoom and turn apart at frame {t}");
            let (co, _) = sp.centre(s[0], s[1], s[2], s[3]);
            let x = (x1.0 + co.0, x1.1 + co.1);
            let w = under(x, l1 + s[2].0, a1 + s[3].0, f);
            assert!((w.0 - p.0).abs() < 1e-6 && (w.1 - p.1).abs() < 1e-6, "the pivot moved at frame {t}: {w:?} against {p:?}");
        }
        let s: Vec<(f64, f64)> = (0..4).map(|i| camera::spring_at(off[i], 0., 1 << 7, LN_2)).collect();
        let (co, cv) = sp.centre(s[0], s[1], s[2], s[3]);
        assert!(co.0.abs() < 1e-9 && co.1.abs() < 1e-9 && cv.0.abs() < 1e-9 && cv.1.abs() < 1e-9);
    }

    /// A flight turned round mid-way (a second tap) keeps the camera's place and speed: the new spiral starts where the old one is, at its speed.
    #[test]
    fn flight_turned_round_keeps_its_speed() {
        let off = [0., 0., 0.9, -0.7];
        let (sp, po, pv) = Spiral::start((200_000., 50_000.), (0., 0.), off[2], off[3], 0., 0.);
        let off = [po.0, po.1, off[2], off[3]];
        let s: Vec<(f64, f64)> = (0..4).map(|i| camera::spring_at(off[i], [pv.0, pv.1, 0., 0.][i], 7, LN_2)).collect();
        let (co, cv) = sp.centre(s[0], s[1], s[2], s[3]);
        // A new target, 30 000 off the old one's centre, its zoom and turn elsewhere.
        let (nd, nl, na) = ((co.0 - 30_000., co.1 + 12_000.), s[2].0 + 0.3, s[3].0 - 0.4);
        let (sp2, po2, pv2) = Spiral::start(nd, cv, nl, na, s[2].1, s[3].1);
        let (c2, v2) = sp2.centre((po2.0, pv2.0), (po2.1, pv2.1), (nl, s[2].1), (na, s[3].1));
        assert!((c2.0 - nd.0).abs() < 1e-6 && (c2.1 - nd.1).abs() < 1e-6, "the centre jumped");
        assert!((v2.0 - cv.0).abs() < 1e-6 && (v2.1 - cv.1).abs() < 1e-6, "the speed jumped: {v2:?} against {cv:?}");
    }

    /// With no zoom or turn to go the centre rides its own spring in a line; a hair of either and the spiral's path is that line to within a hair: no step where the pivot is dropped.
    #[test]
    fn spiral_meets_the_line() {
        let d = (400_000., -300_000.);
        let (line, lo, lv) = Spiral::start(d, (0., 0.), 0., 0., 0., 0.);
        assert!(line.k == (0., 0.) && lo == d && lv == (0., 0.));
        let w = 1e-9;
        let (sp, po, pv) = Spiral::start(d, (0., 0.), w, w, 0., 0.);
        for t in 0..1 << 5 {
            let s: Vec<(f64, f64)> = [po.0, po.1, w, w].iter().zip([pv.0, pv.1, 0., 0.]).map(|(&o, v)| camera::spring_at(o, v, t, LN_2)).collect();
            let (co, _) = sp.centre(s[0], s[1], s[2], s[3]);
            let (lx, ly) = (camera::spring_at(d.0, 0., t, LN_2).0, camera::spring_at(d.1, 0., t, LN_2).0);
            assert!((co.0 - lx).abs() < 1. && (co.1 - ly).abs() < 1., "frame {t}: {co:?} against the line's ({lx}, {ly})");
        }
    }
}
