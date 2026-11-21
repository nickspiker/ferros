//! The deck packed into the screen (owner, 2026-11-21: "zoomed in as possible to fit all pieces but do a dense pack, not a cluster, then a fit, like a pack into screen instead of bunch").
//! His method: "zoom to fit, numerically, then zoom in with the screen edges and great circle top/bottom as hard surfaces and zoom in until it explodes, then you know it's touching on all sides and pressure is ... tracking".
//! That is a compression to the jam (Lubachevsky-Stillinger), quasi-static and deterministic: from the fit, the zoom grows a step at a time, each circle growing about its own centre on the glass (the compression's inflation), and after each step the overlaps (circle on circle, circle on wall) are relaxed by projection with momentum (a heavy ball), a fixed order, no randomness.
//! The pressure is the overlap left after relaxing: drained, the zoom is legal; one that will not drain is jammed, and the zoom is bisected between the last legal one and the first jammed one to under a sixteenth of a pixel on the largest circle.
//! The walls are the room the deck's fit uses (`camera::Room`): the screen's left and right edges, and the big circle (ferros_sys::active_circle), whose top arc is tangent to the camera's knockout and whose bottom arc is tangent to the screen's bottom.
//! f64 and libm only, no state, no allocation: the host tests it (tools/deck-fit).

use super::camera::{MAX, Room};

/// Each step of the zoom's growth: a sixteenth more than the last legal one.
const GROW: f64 = 1. / ((1 << 4) as f64);
/// Relaxing sweeps a zoom gets to drain its pressure: past them it is jammed.
const SWEEPS: u32 = 1 << 8;
/// The relaxation's momentum (a heavy ball): each sweep starts from where the last left plus this much of the way it moved, dropped whenever the pressure rose. Without it a deck crept toward its jam a hair a sweep (the 8a's deck of 2026-11-21 stopped 55 pixels short of the bottom arc in 2^8 sweeps and needed 2^12 to reach it); with it, 2^8 reach it (tools/deck-fit).
const BALL: f64 = 1. - 1. / ((1 << 3) as f64);
/// Pressure drained: no correction in a sweep as large as this (pixels).
const DRAINED: f64 = 1. / ((1 << 10) as f64);
/// The bisection ends when the largest circle's radius at the two zooms differs by under this (pixels).
const FINE: f64 = 1. / ((1 << 4) as f64);
/// The most halvings of the fit's zoom for a deck whose circles overlap where they stand: every overlap halves with the zoom while the room stays, so a few drain any deck the map can hold (2^5 is a zoom 2^-32 of the fit).
const HALVINGS: u32 = 1 << 5;
/// A circle further than this from everything (pixels) is a rattler, loose in its cage once the rest jammed: it is moved to touch the nearest thing.
const LOOSE: f64 = 1. / ((1 << 4) as f64);

/// What a pack found, for the console and the tests.
#[derive(Clone, Copy, Debug)]
pub struct Pack {
    /// The zoom: pixels a unit of the radii.
    pub k: f64,
    /// The zooms tried (each a relaxation), the sweeps all of them took, and the pressure left at the first jammed zoom (pixels, the largest correction its last sweep made).
    pub trials: u32,
    pub sweeps: u32,
    pub pressure: f64,
    /// The rattlers moved to touch.
    pub loose: u32,
}

/// The circles at zoom `k` relaxed against each other and the walls, `p` in pixels from the room's centre and `r` in any unit (`k` pixels each): whether the pressure drained, the sweeps taken, and the largest correction the last sweep made.
/// A sweep pushes every overlapping pair apart along the line between their centres, the smaller circle (by area) moving more, then puts every circle back inside the walls; it is drained when no push or put was as large as `DRAINED`.
/// Each sweep starts with the momentum (`BALL`): every circle carried on by that share of how far the last sweep moved it, which goes when the pressure rises.
fn relax(r: &[f64], p: &mut [(f64, f64)], k: f64, room: Room) -> (bool, u32, f64) {
    let n = r.len();
    // WHY: a circle wider than the room between the edges, or than the big circle, fits at no position; that zoom is jammed whatever the others do, and the wall's projection below would have no side to put it on.
    let biggest = r.iter().fold(0f64, |m, &x| m.max(x)) * k;
    if biggest > room.half_w || biggest > room.radius {
        return (false, 0, biggest - room.half_w.min(room.radius));
    }
    let mut worst = 0.;
    let mut last = [(0f64, 0f64); MAX];
    last[..n].copy_from_slice(p);
    let mut was = f64::MAX;
    for s in 0..SWEEPS {
        for i in 0..n {
            let v = (p[i].0 - last[i].0, p[i].1 - last[i].1);
            last[i] = p[i];
            p[i] = (p[i].0 + BALL * v.0, p[i].1 + BALL * v.1);
        }
        worst = 0f64;
        for i in 0..n {
            for j in i + 1..n {
                let want = k * (r[i] + r[j]);
                let (dx, dy) = (p[j].0 - p[i].0, p[j].1 - p[i].1);
                let d2 = dx * dx + dy * dy;
                if d2 >= want * want {
                    continue;
                }
                let d = libm::sqrt(d2);
                // Two centres on one point have no line between them: apart across, the first to the left (a fixed choice, so the same deck packs the same).
                let (ux, uy) = if d > 0. { (dx / d, dy / d) } else { (1., 0.) };
                let o = want - d;
                worst = worst.max(o);
                let (ai, aj) = (r[i] * r[i], r[j] * r[j]);
                let (si, sj) = (o * aj / (ai + aj), o * ai / (ai + aj));
                p[i] = (p[i].0 - ux * si, p[i].1 - uy * si);
                p[j] = (p[j].0 + ux * sj, p[j].1 + uy * sj);
            }
        }
        for i in 0..n {
            let rho = k * r[i];
            // The walls are hard: a circle past one is put back against it (the physics of a wall, not a guard), and how far it was past is pressure like any overlap.
            let side = room.half_w - rho;
            if p[i].0 > side {
                worst = worst.max(p[i].0 - side);
                p[i].0 = side;
            } else if p[i].0 < -side {
                worst = worst.max(-side - p[i].0);
                p[i].0 = -side;
            }
            let round = room.radius - rho;
            let d = libm::sqrt(p[i].0 * p[i].0 + p[i].1 * p[i].1);
            if d > round {
                worst = worst.max(d - round);
                p[i] = (p[i].0 * round / d, p[i].1 * round / d);
            }
        }
        if worst < DRAINED {
            return (true, s + 1, worst);
        }
        if worst > was {
            last[..n].copy_from_slice(p);
        }
        was = worst;
    }
    (false, SWEEPS, worst)
}

/// How far circle `i` is from the nearest thing (another circle or a wall), in pixels at zoom `k`, and the way toward it (a unit vector).
fn clearance(r: &[f64], p: &[(f64, f64)], k: f64, room: Room, i: usize) -> (f64, (f64, f64)) {
    let rho = k * r[i];
    let (x, y) = p[i];
    let mut best = (room.half_w - rho - x, (1., 0.));
    let left = room.half_w - rho + x;
    if left < best.0 {
        best = (left, (-1., 0.));
    }
    let d = libm::sqrt(x * x + y * y);
    if d > 0. && room.radius - rho - d < best.0 {
        best = (room.radius - rho - d, (x / d, y / d));
    }
    for j in 0..r.len() {
        if j == i {
            continue;
        }
        let (dx, dy) = (p[j].0 - x, p[j].1 - y);
        let dd = libm::sqrt(dx * dx + dy * dy);
        let gap = dd - rho - k * r[j];
        if gap < best.0 && dd > 0. {
            best = (gap, (dx / dd, dy / dd));
        }
    }
    best
}

/// Every circle as large as it fits: the circles of radii `r` (any unit), standing at `p` (pixels from the room's centre, the turned frame of the screen: x across, y down) at the fit's zoom `k0` (pixels a unit, every circle inside the walls), packed into the room.
/// From the fit the zoom grows by `GROW` a step, each circle growing about its own centre on the glass, and the overlaps relaxed (`relax`); the first zoom that jams is bisected against the last legal one to `FINE`, each try starting from the last legal positions; the rattlers are then moved to touch the nearest thing, by the gap to it, which nothing else is nearer than (`clearance`).
/// `p` ends as the packed centres, at the zoom returned.
/// Should the fit's own zoom not drain (a deck whose circles overlap where they stand), the zoom halves until it does, at most `HALVINGS` times.
/// Growing each circle about its own centre, rather than zooming every centre out from the room's, was measured on the 8a's deck (2026-11-21): 1.471 times the fit's zoom against 1.438, and its largest hole a circle of radius 124 pixels against 148 (tools/deck-fit).
pub fn pack(r: &[f64], p: &mut [(f64, f64)], k0: f64, room: Room) -> Pack {
    let n = r.len();
    let mut out = Pack { k: k0, trials: 0, sweeps: 0, pressure: 0., loose: 0 };
    if n == 0 {
        return out;
    }
    let mut trial = [(0f64, 0f64); MAX];
    let mut lo = k0;
    let mut halvings = 0;
    loop {
        let (ok, s, _) = relax(r, p, lo, room);
        out.trials += 1;
        out.sweeps += s;
        // WHY the bound: a loop with no proof of its end is worse than a pack a little small; see `HALVINGS`.
        if ok || halvings == HALVINGS {
            break;
        }
        lo *= 0.5;
        halvings += 1;
    }
    // Growing to the first jam: each legal step is a sixteenth larger, and a circle wider than the room jams (`relax`), so the steps are at most log(room / (k0 · largest)) / log(17/16).
    let mut hi;
    loop {
        let k = lo * (1. + GROW);
        let t = &mut trial[..n];
        t.copy_from_slice(p);
        let (ok, s, pressure) = relax(r, t, k, room);
        out.trials += 1;
        out.sweeps += s;
        if ok {
            lo = k;
            p.copy_from_slice(t);
        } else {
            hi = k;
            out.pressure = pressure;
            break;
        }
    }
    // Bisected to the jam: the largest circle's radius at the two zooms within `FINE` of a pixel.
    let largest = r.iter().fold(0f64, |m, &x| m.max(x));
    while (hi - lo) * largest >= FINE {
        let k = (lo + hi) * 0.5;
        let t = &mut trial[..n];
        t.copy_from_slice(p);
        let (ok, s, pressure) = relax(r, t, k, room);
        out.trials += 1;
        out.sweeps += s;
        if ok {
            lo = k;
            p.copy_from_slice(t);
        } else {
            hi = k;
            out.pressure = pressure;
        }
    }
    // The rattlers: each loose circle moved by its gap toward the nearest thing, in order; everything else is at least that far, so nothing overlaps after.
    for i in 0..n {
        let (gap, (ux, uy)) = clearance(r, p, lo, room, i);
        if gap > LOOSE {
            p[i] = (p[i].0 + ux * gap, p[i].1 + uy * gap);
            out.loose += 1;
        }
    }
    out.k = lo;
    out
}

/// The largest overlap in a pack (pixels, 0 when none): circle on circle, or past a wall. For the tests and the console.
pub fn overlap(r: &[f64], p: &[(f64, f64)], k: f64, room: Room) -> f64 {
    let mut worst = 0f64;
    for i in 0..r.len() {
        let rho = k * r[i];
        worst = worst.max(p[i].0.abs() + rho - room.half_w);
        worst = worst.max(libm::sqrt(p[i].0 * p[i].0 + p[i].1 * p[i].1) + rho - room.radius);
        for j in i + 1..r.len() {
            let (dx, dy) = (p[j].0 - p[i].0, p[j].1 - p[i].1);
            worst = worst.max(k * (r[i] + r[j]) - libm::sqrt(dx * dx + dy * dy));
        }
    }
    worst
}

/// Each circle's gap to the nearest thing in a pack (pixels): 0 is touching. For the tests.
pub fn gap(r: &[f64], p: &[(f64, f64)], k: f64, room: Room, i: usize) -> f64 {
    clearance(r, p, k, room, i).0
}
