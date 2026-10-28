//! Shape writing on the honeycomb (Claude memories/input_rhe_on_glass.md: "lead with shape writing"): one finger slides through a word's letters without lifting, and the word whose key path the trace is closest to, weighed by how common it is, is the one meant. The letters view only; the attest screen never decodes (a handle is exact bytes, typed by tap).
//!
//! In the manner of SHARK² (Kristensson and Zhai, 2004): both the trace and each candidate's ideal path (its letters' key centres, repeated letters once) are resampled to `N` points evenly spaced along their length, then compared point by point in two channels, location (where on the keyboard) and shape (centred and scaled to one pitch, so a small or offset trace of the right word still matches). A word must start and end within reach of the trace's ends, which leaves a few hundred of 32768 to compare. The prior is the word's frequency rank, through log2 by leading zeros (Zipf: a word's probability goes as one over its rank).
use crate::Keyboard;

/// Points each path is resampled to.
pub const N: usize = 32;

/// A word list in frequency order, most common first: each word one length byte then its letters (a to z only: `decode` indexes a 26-entry table by them), built from rhe's English frequency list by the host's build script. Empty is fine: nothing decodes.
#[derive(Clone, Copy)]
pub struct Lexicon<'a>(pub &'a [u8]);

impl<'a> Lexicon<'a> {
    /// Each word with its rank (0 the most common).
    pub fn words(&self) -> impl Iterator<Item = (u32, &'a [u8])> + 'a {
        let b = self.0;
        let mut at = 0usize;
        let mut rank = 0u32;
        core::iter::from_fn(move || {
            if at >= b.len() {
                return None;
            }
            let n = b[at] as usize;
            let w = &b[at + 1..at + 1 + n];
            at += 1 + n;
            rank += 1;
            Some((rank - 1, w))
        })
    }

    /// The word of `rank`.
    pub fn word(&self, rank: u32) -> Option<&'a [u8]> {
        self.words().find(|&(r, _)| r == rank).map(|(_, w)| w)
    }
}

/// log2(x) in Q8 for x ≥ 1: the exponent by leading zeros, the next eight bits of the mantissa as the fraction (linear between powers of two).
fn log2_q8(x: u64) -> i64 {
    let e = 63 - x.leading_zeros() as i64;
    let frac = if e >= 8 { (x >> (e - 8)) & 255 } else { (x << (8 - e)) & 255 };
    (e << 8) + frac as i64
}

/// `path` (Q8 points, at least one) resampled to N points evenly spaced along its length.
fn resample(path: &[(i64, i64)], out: &mut [(i64, i64); N]) {
    let mut total = 0i64;
    for s in path.windows(2) {
        total += seg(s[0], s[1]);
    }
    if total == 0 {
        *out = [path[0]; N];
        return;
    }
    let (mut k, mut walked) = (0usize, 0i64);
    for (i, o) in out.iter_mut().enumerate() {
        let want = total * i as i64 / (N as i64 - 1);
        while k + 2 < path.len() && walked + seg(path[k], path[k + 1]) < want {
            walked += seg(path[k], path[k + 1]);
            k += 1;
        }
        if path.len() == 1 {
            *o = path[0];
            continue;
        }
        let l = seg(path[k], path[k + 1]);
        let t = if l == 0 { 0 } else { ((want - walked) << 8) / l };
        let t = if t > 256 { 256 } else { t };
        *o = (path[k].0 + ((path[k + 1].0 - path[k].0) * t >> 8), path[k].1 + ((path[k + 1].1 - path[k].1) * t >> 8));
    }
}

fn seg(a: (i64, i64), b: (i64, i64)) -> i64 {
    (((a.0 - b.0).pow(2) + (a.1 - b.1).pow(2)) as u64).isqrt() as i64
}

/// Centred on its mean and scaled so its larger extent is one `pitch`: the shape alone.
fn normalised(p: &[(i64, i64); N], pitch: i64) -> [(i64, i64); N] {
    let (mut sx, mut sy) = (0i64, 0i64);
    let (mut x0, mut x1, mut y0, mut y1) = (i64::MAX, i64::MIN, i64::MAX, i64::MIN);
    for &(x, y) in p {
        sx += x;
        sy += y;
        x0 = x0.min(x);
        x1 = x1.max(x);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    let (mx, my) = (sx / N as i64, sy / N as i64);
    let ext = (x1 - x0).max(y1 - y0).max(1);
    let mut o = [(0, 0); N];
    for (o, &(x, y)) in o.iter_mut().zip(p) {
        *o = ((x - mx) * pitch / ext, (y - my) * pitch / ext);
    }
    o
}

/// Mean squared distance between corresponding points.
fn mean_sq(a: &[(i64, i64); N], b: &[(i64, i64); N]) -> i64 {
    a.iter().zip(b).map(|(p, q)| (p.0 - q.0).pow(2) + (p.1 - q.1).pow(2)).sum::<i64>() / N as i64
}

impl Keyboard {
    /// A letter's key centre (Q8) in the letters view.
    fn letter_at(&self, c: u8) -> Option<(i64, i64)> {
        for row in 0..crate::VIS {
            let cells = self.extent_row(row);
            if let Some(col) = cells.iter().position(|&k| k == c as char) {
                return Some(self.centre(row, col));
            }
        }
        None
    }

    /// The trace (pixel points as the finger moved) against every word: the best `out.len()` as (rank, cost), lowest cost first; (u32::MAX, _) where fewer matched. Only in the letters view.
    pub fn decode(&self, trace: &[(i64, i64)], lex: Lexicon, out: &mut [(u32, i64)]) {
        out.fill((u32::MAX, i64::MAX));
        if self.view != 0 || trace.len() < 2 {
            return;
        }
        let pitch = self.px;
        // The trace in Q8, a point kept only once it is a quarter pitch from the last kept one (a dwelling finger's jitter is not path), and the last point always.
        let mut q = [(0i64, 0i64); 256];
        let mut nq = 0;
        let step2 = pitch * pitch / 16;
        for (i, &(x, y)) in trace.iter().enumerate() {
            let p = ((x << 8) + 128, (y << 8) + 128);
            let far = nq == 0 || (p.0 - q[nq - 1].0).pow(2) + (p.1 - q[nq - 1].1).pow(2) >= step2;
            if (far || i == trace.len() - 1) && nq < q.len() {
                if !far {
                    nq -= 1;
                }
                q[nq] = p;
                nq += 1;
            }
        }
        let q = &q[..nq];
        let mut t = [(0, 0); N];
        resample(q, &mut t);
        let tn = normalised(&t, pitch);
        let (first, last) = (t[0], t[N - 1]);
        // Reach for a word's first and last letters: a key pitch and a half.
        let reach2 = pitch * pitch * 9 / 4;
        // Each letter's centre once, a to z (the view is the letters, so all 26 are on show).
        let at: [(i64, i64); 26] = core::array::from_fn(|i| self.letter_at(b'a' + i as u8).unwrap_or((i64::MIN / 4, i64::MIN / 4)));
        let letter = |c: u8| at[(c - b'a') as usize];
        let mut centres = [(0i64, 0i64); 32];
        for (rank, w) in lex.words() {
            if w.len() < 2 || w.len() > centres.len() {
                continue;
            }
            let (a, z) = (letter(w[0]), letter(w[w.len() - 1]));
            if (a.0 - first.0).pow(2) + (a.1 - first.1).pow(2) > reach2 || (z.0 - last.0).pow(2) + (z.1 - last.1).pow(2) > reach2 {
                continue;
            }
            // The ideal path: each letter's centre, a repeated letter once.
            let mut n = 0;
            for &c in w {
                let p = letter(c);
                if n == 0 || centres[n - 1] != p {
                    centres[n] = p;
                    n += 1;
                }
            }
            let mut ideal = [(0, 0); N];
            resample(&centres[..n], &mut ideal);
            let loc = mean_sq(&t, &ideal);
            let shape = mean_sq(&tn, &normalised(&ideal, pitch));
            // In pitch² × 256: location weighs twice shape; the prior is log2 of the rank in the same units (a factor of two in frequency is worth an eighth of a pitch² of mean location error).
            let cost = (loc * 512 + shape * 256) / (pitch * pitch) + log2_q8(rank as u64 + 2) * 32 / 256;

            if cost < out[out.len() - 1].1 {
                let mut at = out.len() - 1;
                while at > 0 && out[at - 1].1 > cost {
                    out[at] = out[at - 1];
                    at -= 1;
                }
                out[at] = (rank, cost);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    fn lexicon(words: &[&str]) -> Vec<u8> {
        let mut v = Vec::new();
        for w in words {
            v.push(w.len() as u8);
            v.extend_from_slice(w.as_bytes());
        }
        v
    }

    /// A trace drawn through a word's keys, with a wobble, decodes to that word, ahead of neighbours that share its ends.
    #[test]
    fn traces_decode() {
        let kb = Keyboard::new(1344, 2992);
        let words = ["the", "tie", "toe", "hello", "help", "hell", "word", "world", "would", "keyboard", "kind", "good", "food", "there", "three", "trace"];
        let bytes = lexicon(&words);
        let lex = Lexicon(&bytes);
        let mut first = 0;
        for (i, w) in words.iter().enumerate() {
            let mut pts = Vec::new();
            let cs: Vec<(i64, i64)> = w.bytes().map(|c| kb.letter_at(c).unwrap()).collect();
            for s in cs.windows(2).filter(|s| s[0] != s[1]) {
                for k in 0..8 {
                    let x = s[0].0 + (s[1].0 - s[0].0) * k / 8;
                    let y = s[0].1 + (s[1].1 - s[0].1) * k / 8;
                    // A wobble of a fifth of a pitch, in pixels.
                    let wob = ((k as i64 * 37 + i as i64 * 11) % 9 - 4) * kb.px / 20;
                    pts.push(((x + wob) >> 8, (y - wob) >> 8));
                }
            }
            let l = *cs.last().unwrap();
            pts.push((l.0 >> 8, l.1 >> 8));
            let mut out = [(0u32, 0i64); 3];
            kb.decode(&pts, lex, &mut out);
            let got: Vec<_> = out.iter().map(|o| lex.word(o.0)).collect();
            // Every word in the first two (the strip offers the rest); a path almost inside a commoner word's (three through the's H to E) may come second.
            assert!(got[..2].contains(&Some(w.as_bytes())), "{w}: got {got:?}");
            first += (got[0] == Some(w.as_bytes())) as usize;
        }
        assert!(first >= words.len() - 1, "only {first} of {} first", words.len());
    }

    #[test]
    fn log2_by_leading_zeros() {
        assert_eq!(log2_q8(1), 0);
        assert_eq!(log2_q8(2), 256);
        assert_eq!(log2_q8(1024), 10 << 8);
        assert!((log2_q8(3) - 406).abs() < 30);
    }
}

#[cfg(test)]
mod real {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    /// The lexicon the hosts build (payloads/hive build.rs): rhe's English list, the first 32768 words of plain letters.
    pub fn rhe_lexicon() -> Option<Vec<u8>> {
        let text = std::fs::read_to_string("/mnt/Harbor/Code/rhe/data/en_freq.txt").ok()?;
        let mut v = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for line in text.lines() {
            let w = line.split_whitespace().next().unwrap_or("");
            if w.is_empty() || w.len() > 24 || !w.bytes().all(|b| b.is_ascii_lowercase()) || !seen.insert(std::string::String::from(w)) {
                continue;
            }
            v.push(w.len() as u8);
            v.extend_from_slice(w.as_bytes());
            if seen.len() == 1 << 15 {
                break;
            }
        }
        Some(v)
    }

    /// Clean traces (through each key centre, eight points a segment) of every 37th of the 2048 commonest words of two letters or more: how often the word comes first and in the first three, and the time per decode (`cargo test -p honeycomb --release -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn against_rhe() {
        let Some(bytes) = rhe_lexicon() else { return };
        let lex = Lexicon(&bytes);
        for layout in 0..crate::LAYOUTS {
        let mut kb = Keyboard::new(1344, 2992);
        kb.layout = layout;
        let (mut n, mut first, mut three) = (0, 0, 0);
        let t0 = std::time::Instant::now();
        for (rank, w) in lex.words().take(2048).filter(|(r, w)| w.len() >= 2 && r % 37 == 0) {
            let cs: Vec<(i64, i64)> = w.iter().map(|&c| kb.letter_at(c).unwrap()).collect();
            let mut pts = Vec::new();
            for s in cs.windows(2).filter(|s| s[0] != s[1]) {
                for k in 0..8 {
                    pts.push(((s[0].0 + (s[1].0 - s[0].0) * k / 8) >> 8, (s[0].1 + (s[1].1 - s[0].1) * k / 8) >> 8));
                }
            }
            let l = *cs.last().unwrap();
            pts.push((l.0 >> 8, l.1 >> 8));
            let mut out = [(0u32, 0i64); 3];
            kb.decode(&pts, lex, &mut out);
            n += 1;
            first += (out[0].0 == rank) as usize;
            three += out.iter().any(|o| o.0 == rank) as usize;
        }
        std::eprintln!("layout {layout}: {n} words: first {first}, in three {three}, {:?} a decode", t0.elapsed() / n as u32);
        }
    }
}
