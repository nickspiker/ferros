//! `panel-white report <profile dir> [--md out.md]`: a display profile run (tools/display-profile.sh, its readings.csv) summarised in Markdown, as tools/display-profile-report.py wrote it: the roll-off of a small window against full field at every brightness, sustained white over time, the limiter's response to average picture level, the tone curve per channel at each brightness (gamma fitted over codes 16-240, with the deviation of each step), near-black, primaries and secondaries (xy, Y, and R+G+B against white), and drift.
//! Run on data/oracle/display/husky-hk3/profile-20261022 it writes that profile's report.md byte for byte.

/// One reading: its CSV row by column name, and Y, x, y as numbers.
struct Row {
    cells: Vec<(String, String)>,
    y: f64,
    cx: f64,
    cy: f64,
}

impl Row {
    fn get(&self, col: &str) -> &str {
        self.cells.iter().find(|(k, _)| k == col).map(|(_, v)| v.as_str()).unwrap_or_else(|| panic!("readings.csv has no column {col}"))
    }
}

/// A map that keeps its keys in the order they first came, as the Python's dicts did (the report's rows come out in the CSV's order); a handful of keys, so a linear search.
struct Ordered<K, V>(Vec<(K, V)>);

impl<K: PartialEq, V> Ordered<K, V> {
    fn new() -> Self {
        Ordered(Vec::new())
    }
    fn entry(&mut self, k: K, new: impl FnOnce() -> V) -> &mut V {
        let i = match self.0.iter().position(|(a, _)| *a == k) {
            Some(i) => i,
            None => {
                self.0.push((k, new()));
                self.0.len() - 1
            }
        };
        &mut self.0[i].1
    }
    fn get(&self, k: &K) -> Option<&V> {
        self.0.iter().find(|(a, _)| a == k).map(|(_, v)| v)
    }
    fn set(&mut self, k: K, v: V) {
        match self.0.iter_mut().find(|(a, _)| *a == k) {
            Some(e) => e.1 = v,
            None => self.0.push((k, v)),
        }
    }
}

/// A number to `prec` places as Python formats it: "nan" (or "+nan" when signed) where Rust says "NaN".
fn num(v: f64, prec: usize, signed: bool) -> String {
    if v.is_nan() {
        return if signed { "+nan".into() } else { "nan".into() };
    }
    if signed { format!("{v:+.prec$}") } else { format!("{v:.prec$}") }
}

/// The digits of a label as a number (`grey128` is 128).
fn code_of(label: &str) -> u32 {
    label.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or_else(|_| panic!("label {label} has no code"))
}

pub fn report(args: &[String]) {
    let mut args = args.to_vec();
    let md = args.iter().position(|a| a == "--md").map(|i| {
        let v = args.remove(i + 1);
        args.remove(i);
        v
    });
    let dir = args.first().unwrap_or_else(|| panic!("panel-white report <profile dir> [--md out.md]"));
    let csv = std::fs::read_to_string(format!("{dir}/readings.csv")).unwrap_or_else(|e| panic!("{dir}/readings.csv: {e}"));
    let mut lines = csv.lines();
    let head: Vec<String> = lines.next().unwrap_or_default().split(',').map(str::to_string).collect();
    let mut rows = Vec::new();
    for l in lines.filter(|l| !l.is_empty()) {
        let cells: Vec<(String, String)> = head.iter().cloned().zip(l.split(',').map(str::to_string)).collect();
        let mut r = Row { cells, y: 0., cx: 0., cy: 0. };
        let yv = r.get("Y_cdm2");
        if yv == "?" || yv.is_empty() {
            continue;
        }
        r.y = yv.parse().unwrap();
        r.cx = r.get("x").parse().unwrap();
        r.cy = r.get("y").parse().unwrap();
        rows.push(r);
    }
    let sec = |name: &str| -> Vec<&Row> { rows.iter().filter(|r| r.get("section") == name).collect() };
    let mut out: Vec<String> = Vec::new();
    let mut p = |s: String| out.push(s);

    p("## Window against full field (white)\n".into());
    p("| DBV | 1 cm | 2 cm | 4 cm | full | full ÷ 1 cm |".into());
    p("|---|---|---|---|---|---|".into());
    let mut by: Ordered<String, Ordered<u32, f64>> = Ordered::new();
    for r in sec("rolloff") {
        by.entry(r.get("dbv").into(), Ordered::new).set(r.get("window_px").parse().unwrap(), r.y);
    }
    for (dbv, w) in &by.0 {
        let mut ks: Vec<u32> = w.0.iter().map(|(k, _)| *k).filter(|&k| k != 0).collect();
        ks.sort();
        let small = ks.first().map(|k| *w.get(k).unwrap());
        let mut cells: Vec<String> = ks.iter().map(|k| num(*w.get(k).unwrap(), 1, false)).collect();
        cells.push(num(w.get(&0).copied().unwrap_or(f64::NAN), 1, false));
        let ratio = match (small, w.get(&0)) {
            (Some(s), Some(full)) if s != 0. => format!("{:.3}", full / s),
            _ => "-".into(),
        };
        p(format!("| {dbv} | {} | {ratio} |", cells.join(" | ")));
    }

    p("\n## Full white held\n".into());
    p("| DBV | first | last | change | readings |".into());
    p("|---|---|---|---|---|".into());
    let mut sus: Ordered<String, Vec<f64>> = Ordered::new();
    for r in sec("sustain") {
        sus.entry(r.get("dbv").into(), Vec::new).push(r.y);
    }
    for (dbv, ys) in &sus.0 {
        let (a, b) = (ys[0], ys[ys.len() - 1]);
        let all: Vec<String> = ys.iter().map(|y| format!("{y:.0}")).collect();
        p(format!("| {dbv} | {a:.1} | {b:.1} | {} % | {} |", num(100. * (b / a - 1.), 1, true), all.join(" ")));
    }

    p("\n## The 1 cm white disc against its surround (average picture level)\n".into());
    p("| DBV | surround 00 | 40 | 80 | C0 | FF |".into());
    p("|---|---|---|---|---|---|".into());
    let mut apl: Ordered<String, Ordered<String, f64>> = Ordered::new();
    for r in sec("apl") {
        apl.entry(r.get("dbv").into(), Ordered::new).set(r.get("surround").into(), r.y);
    }
    for (dbv, s) in &apl.0 {
        let cells: Vec<String> = ["000000", "404040", "808080", "c0c0c0", "ffffff"].iter().map(|k| num(s.get(&k.to_string()).copied().unwrap_or(f64::NAN), 1, false)).collect();
        p(format!("| {dbv} | {} |", cells.join(" | ")));
    }

    let mut w1353: Option<f64> = None;
    for (secname, full, title) in [("ramps", 255u32, "8-bit codes"), ("ramps10", 1023, "10-bit codes, 10 bpc compose + dither")] {
        let mut ramps: Ordered<(String, String), Ordered<u32, f64>> = Ordered::new();
        for r in sec(secname) {
            let label = r.get("label");
            let ch: String = label.chars().filter(|c| c.is_alphabetic()).collect();
            ramps.entry((r.get("dbv").into(), ch), Ordered::new).set(code_of(label), r.y);
        }
        if ramps.0.is_empty() {
            continue;
        }
        p(format!("\n## Tone curves, {title} (1 cm window)\n"));
        p("| DBV | channel | Y at full | black | gamma (6-94 %) | worst step off the fit |".into());
        p("|---|---|---|---|---|---|".into());
        let (lo, hi) = (full * 16 / 255, full * 240 / 255);
        let mut keys: Vec<&(String, String)> = ramps.0.iter().map(|(k, _)| k).collect();
        keys.sort();
        for k in keys {
            let m = ramps.get(k).unwrap();
            let top = match m.get(&full) {
                Some(&t) if t != 0. => t,
                _ => continue,
            };
            let blk = m.get(&0).copied().unwrap_or(0.);
            // Each step's (code as a fraction of full, light as a fraction of full, code), black taken off, in the CSV's order (the sums below are the Python's, in its order).
            let pts: Vec<(f64, f64, u32)> = m.0.iter().filter(|&&(c, y)| lo <= c && c <= hi && y > blk).map(|&(c, y)| (c as f64 / full as f64, (y - blk) / (top - blk), c)).collect();
            let num_: f64 = pts.iter().map(|(u, v, _)| v.ln() * u.ln()).sum();
            let den: f64 = pts.iter().map(|(u, _, _)| u.ln() * u.ln()).sum();
            let g = if den != 0. { num_ / den } else { f64::NAN };
            // The worst step: the largest relative miss, ties to the higher code (Python's max over (miss, code) tuples).
            let mut worst = (0f64, 0u32);
            for &(u, v, c) in &pts {
                let miss = (v / u.powf(g) - 1.).abs();
                if miss > worst.0 || (miss == worst.0 && c > worst.1) {
                    worst = (miss, c);
                }
            }
            p(format!("| {} | {} | {top:.2} | {blk:.4} | {} | {:.1} % at {} |", k.0, k.1, num(g, 3, false), 100. * worst.0, worst.1));
        }
        if secname == "ramps" {
            w1353 = ramps.get(&("1353".to_string(), "grey".to_string())).and_then(|m| m.get(&255)).copied();
        }
    }

    for (secname, full) in [("nearblack", 255u32), ("nearblack10", 1023)] {
        let rows_nb = sec(secname);
        if rows_nb.is_empty() {
            continue;
        }
        p(format!("\n## Near black, {}-bit codes (DBV 1353)\n", if full == 1023 { "10" } else { "8" }));
        p("| code | Y cd/m² | against γ 2.185 from white |".into());
        p("|---|---|---|".into());
        for r in rows_nb {
            let c = code_of(r.get("label"));
            let exp = match w1353 {
                Some(w) if w != 0. => w * (c as f64 / full as f64).powf(2.185),
                _ => f64::NAN,
            };
            p(format!("| {c} | {:.4} | {} % |", r.y, num(100. * (r.y / exp - 1.), 0, true)));
        }
    }

    p("\n## Colours (1 cm window)\n".into());
    p("| DBV | patch | Y | x | y |".into());
    p("|---|---|---|---|---|".into());
    let mut col: Ordered<String, Ordered<String, f64>> = Ordered::new();
    for r in sec("colours") {
        col.entry(r.get("dbv").into(), Ordered::new).set(r.get("label").into(), r.y);
        p(format!("| {} | {} | {:.2} | {:.4} | {:.4} |", r.get("dbv"), r.get("label"), r.y, r.cx, r.cy));
    }
    for (dbv, c) in &col.0 {
        let y = |k: &str| c.get(&k.to_string()).copied();
        if let (Some(r), Some(g), Some(b), Some(w)) = (y("red"), y("green"), y("blue"), y("white")) {
            let s = r + g + b;
            p(format!("\nAt DBV {dbv}: R+G+B = {s:.1} against white {w:.1} ({} %).", num(100. * (s / w - 1.), 1, true)));
        }
    }

    let dr = sec("drift");
    if dr.len() >= 2 {
        let (a, b) = (dr[0], dr[dr.len() - 1]);
        p(format!("\n## Drift\n\nWhite at 1353, first {:.2} at t={} s, last {:.2} at t={} s ({} %).", a.y, a.get("t_s"), b.y, b.get("t_s"), num(100. * (b.y / a.y - 1.), 2, true)));
    }

    let text = out.join("\n");
    println!("{text}");
    if let Some(md) = md {
        std::fs::write(&md, format!("{text}\n")).unwrap_or_else(|e| panic!("{md}: {e}"));
    }
}
