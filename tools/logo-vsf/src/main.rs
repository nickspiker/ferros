//! logo.svg → logo.vsf: the ferros mark (the round iron phase map) as analytic VSF shapes, not transcribed Béziers.
//!
//! Every piece of the mark is a ring segment or a pie sector about one centre. Illustrator wrote each as cubic Béziers approximating arcs, to a tenth of a unit. This tool reads those curves back into what they stand for:
//! 0. Fit one centre to every curve's points (Gauss-Newton on equal distance per arc).
//! 1. Read each piece's radii (an outer arc and an inner arc, or one arc and the centre) and the angles where its outer arc starts and ends.
//! 2. Pieces that meet share an edge, so the same radius or angle measured on different pieces is clustered into one value, weighted by how finely the SVG pins it (a tenth of a unit is a smaller angle at radius 290 than at 41). Shared edges then meet exactly, with no seam.
//! 3. Check every point of the SVG lies within a quarter unit of the shape it became, and stop if one does not.
//! 4. Write one `roe` per piece, in the SVG's order (painter's), under a node whose `view` is the SVG's viewBox, colours converted from sRGB to 16-bit VSF RGB.
//!
//! Run from anywhere: `cargo run --release --manifest-path ferros/tools/logo-vsf/Cargo.toml`. It prints what it found and renders the result with vsf's reference rasterizer to confirm the coverage matches.

use std::f64::consts::TAU;
use std::path::PathBuf;
use vsf::colour::convert::ColourFormat;
use vsf::types::{Ellipse, Node, Num, Nums, Paint};
use vsf::vsf_builder::VsfBuilder;
use vsf::VsfType;

/// Half the SVG's 0.1-unit rounding: the standard error of a coordinate it wrote.
const ROUNDING: f64 = 0.05;

/// The furthest any SVG point may sit from the shape it was read as.
const FIT_LIMIT: f64 = 0.25;

/// Arcs closer than this in radius belong to the same circle of one piece.
const SAME_ARC: f64 = 2.;

#[derive(Debug, Clone, Copy)]
enum Seg {
    Line([f64; 2], [f64; 2]),
    Cubic([f64; 2], [f64; 2], [f64; 2], [f64; 2]),
}

impl Seg {
    /// Points on the segment: a cubic's at t = 0, ¼, ½, ¾, 1, a line's two ends.
    fn samples(&self) -> Vec<[f64; 2]> {
        match *self {
            Seg::Line(a, b) => vec![a, b],
            Seg::Cubic(a, b, c, d) => (0..5)
                .map(|k| {
                    let t = k as f64 / 4.;
                    let u = 1. - t;
                    let w = [u * u * u, 3. * u * u * t, 3. * u * t * t, t * t * t];
                    [w[0] * a[0] + w[1] * b[0] + w[2] * c[0] + w[3] * d[0], w[0] * a[1] + w[1] * b[1] + w[2] * c[1] + w[3] * d[1]]
                })
                .collect(),
        }
    }
}

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("logo-vsf: {msg}");
    std::process::exit(1)
}

/// The numbers in a run of path data: SVG packs them with `-` and `.` as separators as well as commas and spaces.
fn numbers(s: &str) -> Vec<f64> {
    let b = s.as_bytes();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < b.len() {
        let c = b[i];
        if c == b'-' || c == b'.' || c.is_ascii_digit() {
            let start = i;
            let mut dot = c == b'.';
            i += 1;
            while i < b.len() && (b[i].is_ascii_digit() || (b[i] == b'.' && !dot)) {
                dot |= b[i] == b'.';
                i += 1;
            }
            out.push(s[start..i].parse().unwrap_or_else(|e| fail(format!("number '{}': {e}", &s[start..i]))));
        } else {
            i += 1;
        }
    }
    out
}

/// Path data with the commands this file uses (M, L, l, C, c, S, s, z) as absolute segments. Any other command stops the tool: it would need reading, not guessing.
fn parse_path(d: &str) -> Vec<Seg> {
    let mut cmds: Vec<(char, Vec<f64>)> = Vec::new();
    let chars: Vec<char> = d.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_alphabetic() {
            let c = chars[i];
            let start = i + 1;
            i += 1;
            while i < chars.len() && !chars[i].is_ascii_alphabetic() {
                i += 1;
            }
            cmds.push((c, numbers(&chars[start..i].iter().collect::<String>())));
        } else {
            i += 1;
        }
    }
    let (mut segs, mut cur, mut start, mut last_c2) = (Vec::new(), [0.; 2], [0.; 2], None::<[f64; 2]>);
    for (c, v) in cmds {
        let rel = c.is_ascii_lowercase();
        let o = |cur: [f64; 2]| if rel { cur } else { [0., 0.] };
        match c {
            'M' => {
                cur = [v[0], v[1]];
                start = cur;
                last_c2 = None;
            }
            'L' | 'l' => {
                for p in v.chunks(2) {
                    let b = o(cur);
                    let to = [b[0] + p[0], b[1] + p[1]];
                    segs.push(Seg::Line(cur, to));
                    cur = to;
                }
                last_c2 = None;
            }
            'C' | 'c' => {
                for p in v.chunks(6) {
                    let b = o(cur);
                    let (c1, c2, to) = ([b[0] + p[0], b[1] + p[1]], [b[0] + p[2], b[1] + p[3]], [b[0] + p[4], b[1] + p[5]]);
                    segs.push(Seg::Cubic(cur, c1, c2, to));
                    last_c2 = Some(c2);
                    cur = to;
                }
            }
            'S' | 's' => {
                for p in v.chunks(4) {
                    let b = o(cur);
                    // The first control point mirrors the previous curve's second about the current point.
                    let c1 = last_c2.map_or(cur, |l| [2. * cur[0] - l[0], 2. * cur[1] - l[1]]);
                    let (c2, to) = ([b[0] + p[0], b[1] + p[1]], [b[0] + p[2], b[1] + p[3]]);
                    segs.push(Seg::Cubic(cur, c1, c2, to));
                    last_c2 = Some(c2);
                    cur = to;
                }
            }
            'z' | 'Z' => {
                if cur != start {
                    segs.push(Seg::Line(cur, start));
                }
                cur = start;
            }
            other => fail(format!("path command '{other}' is not one this tool reads")),
        }
    }
    segs
}

/// The text between `open` and the next `close` after it.
fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let a = s.find(open)? + open.len();
    Some(&s[a..a + s[a..].find(close)?])
}

struct Piece {
    class: String,
    rgb: [u8; 3],
    segs: Vec<Seg>,
}

fn read_svg(svg: &str) -> ([f64; 4], Vec<Piece>) {
    let vb = numbers(between(svg, "viewBox=\"", "\"").unwrap_or_else(|| fail("no viewBox")));
    let style = between(svg, "<style", "</style>").unwrap_or_else(|| fail("no <style>"));
    let colour = |class: &str| -> [u8; 3] {
        let hex = between(style, &format!(".{class}{{fill:#"), ";").unwrap_or_else(|| fail(format!("no fill for .{class}")));
        let n = u32::from_str_radix(hex, 16).unwrap_or_else(|e| fail(format!(".{class} fill '{hex}': {e}")));
        [(n >> 16) as u8, (n >> 8) as u8, n as u8]
    };
    let pieces = svg
        .split("<path")
        .skip(1)
        .map(|p| {
            let class = between(p, "class=\"", "\"").unwrap_or_else(|| fail("a path with no class")).to_string();
            let d: String = between(p, " d=\"", "\"").unwrap_or_else(|| fail("a path with no d")).chars().filter(|c| !c.is_whitespace()).collect();
            Piece { rgb: colour(&class), segs: parse_path(&d), class }
        })
        .collect();
    ([vb[0], vb[1], vb[2], vb[3]], pieces)
}

/// One centre for every arc: each cubic's samples share a distance from it, the distance its own.
fn fit_centre(pieces: &[Piece]) -> [f64; 2] {
    // An arc is a run of consecutive cubics in one piece, one radius for the whole run: a single short cubic barely constrains the centre along its own normal, and fitted alone each lets the centre drift to a wrong minimum.
    let mut arcs: Vec<Vec<[f64; 2]>> = Vec::new();
    for p in pieces {
        let mut run: Vec<[f64; 2]> = Vec::new();
        for s in &p.segs {
            match s {
                Seg::Cubic(..) => run.extend(s.samples()),
                Seg::Line(..) => {
                    if !run.is_empty() {
                        arcs.push(std::mem::take(&mut run));
                    }
                }
            }
        }
        if !run.is_empty() {
            arcs.push(run);
        }
    }
    // Start from the mean of all arc points; the arcs surround the centre on most sides.
    let all: Vec<[f64; 2]> = arcs.iter().flatten().copied().collect();
    let mut c = [all.iter().map(|p| p[0]).sum::<f64>() / all.len() as f64, all.iter().map(|p| p[1]).sum::<f64>() / all.len() as f64];
    for _ in 0..1 << 6 {
        let (mut jtj, mut jtr) = ([[0.; 2]; 2], [0.; 2]);
        for a in &arcs {
            let dist = |p: &[f64; 2]| ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt();
            let r = a.iter().map(dist).sum::<f64>() / a.len() as f64;
            for p in a {
                let d = dist(p);
                let g = [-(p[0] - c[0]) / d, -(p[1] - c[1]) / d];
                for i in 0..2 {
                    jtr[i] += g[i] * (d - r);
                    for j in 0..2 {
                        jtj[i][j] += g[i] * g[j];
                    }
                }
            }
        }
        let det = jtj[0][0] * jtj[1][1] - jtj[0][1] * jtj[1][0];
        c[0] -= (jtj[1][1] * jtr[0] - jtj[0][1] * jtr[1]) / det;
        c[1] -= (jtj[0][0] * jtr[1] - jtj[1][0] * jtr[0]) / det;
    }
    c
}

/// A piece as read: radii (outer, and inner when it is a ring) and the turns where its outer arc starts and how far it sweeps.
struct Measured {
    outer: f64,
    inner: Option<f64>,
    start: f64,
    sweep: f64,
}

fn turns(c: [f64; 2], p: [f64; 2]) -> f64 {
    let t = (p[1] - c[1]).atan2(p[0] - c[0]) / TAU;
    t - t.floor()
}

fn radius(c: [f64; 2], p: [f64; 2]) -> f64 {
    ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt()
}

/// The signed step between two angles in turns, taken the short way round.
fn step(a: f64, b: f64) -> f64 {
    let d = b - a;
    d - d.round()
}

fn measure(p: &Piece, c: [f64; 2]) -> Measured {
    let arcs: Vec<([f64; 2], [f64; 2], f64)> = p
        .segs
        .iter()
        .filter_map(|s| match *s {
            Seg::Cubic(a, _, _, d) => Some((a, d, (radius(c, a) + radius(c, d)) / 2.)),
            Seg::Line(..) => None,
        })
        .collect();
    let outer_r = arcs.iter().map(|a| a.2).fold(0., f64::max);
    let inner_r = arcs.iter().map(|a| a.2).fold(f64::INFINITY, f64::min);
    let outer: Vec<&([f64; 2], [f64; 2], f64)> = arcs.iter().filter(|a| (a.2 - outer_r).abs() < SAME_ARC).collect();
    let start = turns(c, outer[0].0);
    let sweep: f64 = outer.iter().map(|a| step(turns(c, a.0), turns(c, a.1))).sum();
    // A sector's outline runs out to the centre itself; a ring's never comes near it.
    let reaches_centre = p.segs.iter().any(|s| s.samples().iter().any(|q| radius(c, *q) < SAME_ARC));
    let inner = match (reaches_centre, (outer_r - inner_r).abs() < SAME_ARC) {
        (true, true) => None,
        (false, false) => Some(inner_r),
        _ => fail(format!(".{}: neither a sector (one arc and the centre) nor a ring segment (two arcs)", p.class)),
    };
    Measured { outer: outer_r, inner, start, sweep }
}

/// One measurement of a shared value, its standard error, and where it goes back.
struct Reading {
    value: f64,
    sigma: f64,
    slot: (usize, usize),
}

/// Merge readings that agree within three combined standard errors into their weighted mean; `wrap` treats values as turns, equal across 1.
fn cluster(mut rs: Vec<Reading>, wrap: bool) -> Vec<(f64, Vec<(usize, usize)>)> {
    rs.sort_by(|a, b| a.value.total_cmp(&b.value));
    let mut out: Vec<(f64, f64, Vec<(usize, usize)>)> = Vec::new();
    for r in rs {
        if let Some(last) = out.last_mut() {
            if (r.value - last.0).abs() < 3. * (r.sigma.powi(2) + last.1.powi(2)).sqrt() {
                let (w0, w1) = (last.1.powi(-2), r.sigma.powi(-2));
                last.0 = (last.0 * w0 + r.value * w1) / (w0 + w1);
                last.1 = (w0 + w1).powf(-0.5);
                last.2.push(r.slot);
                continue;
            }
        }
        out.push((r.value, r.sigma, vec![r.slot]));
    }
    if wrap && out.len() > 1 {
        let (f, l) = (&out[0], &out[out.len() - 1]);
        if (f.0 + 1. - l.0).abs() < 3. * (f.1.powi(2) + l.1.powi(2)).sqrt() {
            let (w0, w1) = (f.1.powi(-2), l.1.powi(-2));
            let merged = ((f.0 * w0 + (l.0 - 1.) * w1) / (w0 + w1), (w0 + w1).powf(-0.5));
            let last = out.pop().unwrap();
            out[0].0 = merged.0 - merged.0.floor();
            out[0].1 = merged.1;
            out[0].2.extend(last.2);
        }
    }
    out.into_iter().map(|(v, _, s)| (v, s)).collect()
}

fn main() {
    let ferros = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap_or_else(|e| fail(format!("the ferros directory: {e}")));
    let svg_path = ferros.join("logo.svg");
    let svg = std::fs::read_to_string(&svg_path).unwrap_or_else(|e| fail(format!("{}: {e}", svg_path.display())));
    let (view_box, pieces) = read_svg(&svg);
    let c = fit_centre(&pieces);
    let mut shapes: Vec<Measured> = pieces.iter().map(|p| measure(p, c)).collect();

    // Shared radii: slot (piece, 0) is the outer radius, (piece, 1) the inner.
    let mut radii = Vec::new();
    // Shared angles: slot (piece, 0) is the start, (piece, 1) the end.
    let mut angles = Vec::new();
    for (i, m) in shapes.iter().enumerate() {
        radii.push(Reading { value: m.outer, sigma: ROUNDING, slot: (i, 0) });
        if let Some(r) = m.inner {
            radii.push(Reading { value: r, sigma: ROUNDING, slot: (i, 1) });
        }
        let sigma = ROUNDING / m.outer / TAU;
        angles.push(Reading { value: m.start, sigma, slot: (i, 0) });
        let end = m.start + m.sweep;
        angles.push(Reading { value: end - end.floor(), sigma, slot: (i, 1) });
    }
    let radius_groups = cluster(radii, false);
    let angle_groups = cluster(angles, true);
    let mut ends = vec![[0.; 2]; shapes.len()];
    for (v, slots) in &radius_groups {
        for &(i, k) in slots {
            match k {
                0 => shapes[i].outer = *v,
                _ => shapes[i].inner = Some(*v),
            }
        }
    }
    for (v, slots) in &angle_groups {
        for &(i, k) in slots {
            ends[i][k] = *v;
        }
    }
    for (m, e) in shapes.iter_mut().zip(&ends) {
        m.start = e[0];
        // The sweep keeps its measured direction and size; only its ends move to the shared angles.
        let s = e[1] - e[0];
        m.sweep = if m.sweep > 0. { s - s.floor() } else { s - s.ceil() };
    }

    // Every SVG point within FIT_LIMIT of the shape it became: on one of its arcs, or on one of its two radial edges.
    for (p, m) in pieces.iter().zip(&shapes) {
        let mut worst = 0f64;
        for q in p.segs.iter().flat_map(Seg::samples) {
            let r = radius(c, q);
            let on_arc = [Some(m.outer), m.inner].into_iter().flatten().map(|a| (r - a).abs()).fold(f64::INFINITY, f64::min);
            let t = turns(c, q);
            let on_edge = [m.start, m.start + m.sweep].iter().map(|e| (step(*e, t) * TAU).sin().abs() * r).fold(f64::INFINITY, f64::min);
            worst = worst.max(on_arc.min(on_edge));
        }
        if worst > FIT_LIMIT {
            fail(format!(".{}: a point {worst:.3} units from its shape (limit {FIT_LIMIT})", p.class));
        }
        println!(".{} #{:02X}{:02X}{:02X}  {}  r {:.3}{}  from {:.6} turning {:.6}  (worst point {worst:.3})", p.class, p.rgb[0], p.rgb[1], p.rgb[2], if m.inner.is_some() { "ring segment" } else { "sector      " }, m.outer, m.inner.map_or(String::new(), |r| format!("–{r:.3}")), m.start, m.sweep);
    }
    println!("centre ({:.3}, {:.3}); {} shared radii, {} shared angles", c[0], c[1], radius_groups.len(), angle_groups.len());

    let mut node = Node::new();
    let (half_w, half_h) = (view_box[2] / 2., view_box[3] / 2.);
    node.view = Some(Nums::f32(vec![2, 2], vec![(view_box[0] + half_w) as f32, (view_box[1] + half_h) as f32, half_w as f32, half_h as f32]).unwrap());
    for (p, m) in pieces.iter().zip(&shapes) {
        let mut g = vec![c[0] as f32, c[1] as f32, m.outer as f32, m.outer as f32];
        if let Some(r) = m.inner {
            g.extend([r as f32, r as f32]);
        }
        let colour = VsfType::from_srgb_f32(p.rgb[0], p.rgb[1], p.rgb[2], ColourFormat::Rt);
        node.children.push(VsfType::roe(Box::new(Ellipse {
            geometry: Nums::f32(vec![g.len() / 2, 2], g).unwrap(),
            rotation: None,
            start: Some(Num::from(m.start as f32)),
            sweep: Some(Num::from(m.sweep as f32)),
            fill: Some(Paint::colour(colour).unwrap_or_else(|e| fail(e))),
            stroke: None,
        })));
    }
    let scene = VsfType::ron(Box::new(node));
    let (name, fields) = vsf::types::renderable::scene_section(scene.clone());
    let bytes = VsfBuilder::new().add_section(name, fields).build().unwrap_or_else(|e| fail(e));
    let out = ferros.join("logo.vsf");
    std::fs::write(&out, &bytes).unwrap_or_else(|e| fail(format!("{}: {e}", out.display())));

    // Read it back and draw it: the file is what was meant, and what it draws covers what the SVG covers.
    let back = vsf::types::renderable::read_scene(&bytes).unwrap_or_else(|e| fail(format!("reading back: {e}")));
    if back != scene {
        fail("logo.vsf does not read back as written");
    }
    let side = 1 << 10;
    let drawn = vsf::raster::render_fitted(&back, side, side, ([0., 0.], [1., 1.]));
    let traced = vsf::raster::render_fitted(&bezier_scene(&pieces, &back), side, side, ([0., 0.], [1., 1.]));
    let (mut worst, mut total) = (0f32, 0f64);
    for (a, b) in drawn.image.rgba.iter().zip(&traced.image.rgba) {
        for ch in 0..4 {
            let d = (a[ch] - b[ch]).abs();
            worst = worst.max(d);
            total += d as f64;
        }
    }
    println!("wrote {} ({} bytes)", out.display(), bytes.len());
    println!("against the SVG's own Béziers drawn the same way at {side}²: mean difference {:.6}, largest {worst:.4} (of 1, in one pixel)", total / (side * side * 4) as f64);
}

/// The SVG's outlines exactly as written, as `rop` paths of lines and cubics, painted as the analytic scene paints them: the reference the conversion is checked against.
fn bezier_scene(pieces: &[Piece], analytic: &VsfType) -> VsfType {
    let VsfType::ron(a) = analytic else { fail("the scene is not a node") };
    let mut node = Node::new();
    node.view = a.view.clone();
    for (p, shape) in pieces.iter().zip(&a.children) {
        let VsfType::roe(e) = shape else { fail("a piece is not an ellipse") };
        let (mut pts, mut verbs) = (Vec::new(), vec![vsf::types::Verb::Move]);
        let first = match p.segs[0] {
            Seg::Line(a, _) | Seg::Cubic(a, ..) => a,
        };
        pts.push([first[0] as f32, first[1] as f32]);
        for s in &p.segs {
            match *s {
                Seg::Line(_, b) => {
                    verbs.push(vsf::types::Verb::Line);
                    pts.push([b[0] as f32, b[1] as f32]);
                }
                Seg::Cubic(_, b, c, d) => {
                    verbs.push(vsf::types::Verb::Cubic);
                    pts.extend([b, c, d].map(|q| [q[0] as f32, q[1] as f32]));
                }
            }
        }
        verbs.push(vsf::types::Verb::Close);
        node.children.push(VsfType::rop(Box::new(vsf::types::Path { points: Nums::points_f32(&pts), verbs, angles: None, rule: None, fill: e.fill.clone(), stroke: None })));
    }
    VsfType::ron(Box::new(node))
}
