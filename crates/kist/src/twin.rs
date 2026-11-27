//! Two BLAKE3 hashes of a capsule in one pass over its bytes (2026-11-17, owner: "it takes TWO SECONDS to open Photon"): its address (the plain hash of the file) and its file hash (the hash with the signature's slot read as zeros, what the signature covers).
//!
//! BLAKE3 is a tree: the bytes are hashed in leaves, the leaves' chaining values merged pairwise up to the root, the tree's shape fixed by the length alone (`hazmat::left_subtree_len`).
//! The two inputs differ only inside the slot, a few KiB in the header, so every leaf the slot does not touch has the same chaining value in both trees.
//! Each leaf is hashed once; a leaf the slot touches is hashed a second time with the slot as zeros; both roots are merged from the leaves at the end.
//! For Photon's 92 MB capsule that is one pass where the kernel made two (its address check and the gate's signature hash), each 92 MB of BLAKE3 on the kernel's core.
//!
//! The leaves are LEAF bytes from offset 0, the last one shorter: every leaf is a subtree of BLAKE3's canonical tree (each split it makes lies at a multiple of a power of two no smaller than LEAF), so the roots are exactly `blake3::hash` of each input (the tests hold them to it).

use alloc::vec::Vec;
use blake3::hazmat::{merge_subtrees_non_root, merge_subtrees_root, ChainingValue, HasherExt, Mode};
use blake3::Hasher;

/// A leaf is 2^16 bytes: the kernel's kist job hands transfers of 2^18 to 2^20, and a leaf's chaining value is 32 bytes (45 KB of them for a 92 MB capsule).
pub const LEAF_LOG2: u32 = 16;
pub const LEAF: usize = 1 << LEAF_LOG2;

/// The two hashes being taken.
pub struct Twin {
    /// The span read as zeros in the second hash: its start and length.
    slot: (usize, usize),
    /// The finished leaves' chaining values, and the second hash's for each leaf the slot touches (by leaf index).
    leaves: Vec<ChainingValue>,
    zeroed: Vec<(usize, ChainingValue)>,
    /// The leaf being hashed, and its zeroed twin while the slot touches it.
    cur: Hasher,
    cur_z: Option<Hasher>,
    /// Bytes taken.
    at: usize,
}

/// A leaf's hasher at leaf `k`: the first one plain, so a capsule of one leaf finalizes as a root.
fn leaf_hasher(k: usize) -> Hasher {
    let mut h = Hasher::new();
    if k > 0 {
        h.set_input_offset((k << LEAF_LOG2) as u64);
    }
    h
}

impl Twin {
    /// `slot`: the signature's value in the file, its start and length (`fetch::head`).
    pub fn new(slot: (usize, usize)) -> Twin {
        let mut t = Twin { slot, leaves: Vec::new(), zeroed: Vec::new(), cur: Hasher::new(), cur_z: None, at: 0 };
        t.cur_z = t.touches(0).then(Hasher::new);
        t
    }

    /// Whether the slot touches leaf `k`.
    fn touches(&self, k: usize) -> bool {
        let (s, n) = self.slot;
        n > 0 && s < (k + 1) << LEAF_LOG2 && s + n > k << LEAF_LOG2
    }

    /// The next bytes of the file, in order.
    pub fn update(&mut self, mut bytes: &[u8]) {
        const ZEROS: [u8; 64] = [0; 64];
        while !bytes.is_empty() {
            // The leaf before is whole and more bytes follow: closed now, never at its last byte, so the file's last leaf stays open for `finish`.
            if self.at == (self.leaves.len() + 1) << LEAF_LOG2 {
                self.close_leaf(self.leaves.len());
            }
            let k = self.at >> LEAF_LOG2;
            let room = ((k + 1) << LEAF_LOG2) - self.at;
            // WHY the min: a leaf takes only up to its end; the rest goes to the next one.
            let n = room.min(bytes.len());
            let (part, rest) = bytes.split_at(n);
            self.cur.update(part);
            if let Some(z) = self.cur_z.as_mut() {
                // The part as the second hash reads it: what lies in the slot as zeros.
                let (s, len) = self.slot;
                let mut p = self.at;
                let end = self.at + n;
                while p < end {
                    let inside = p >= s && p < s + len;
                    // WHY the min: a run stops at the slot's edge or the part's end, whichever is first, and a run of zeros at the length of ZEROS.
                    let stop = if inside { (s + len).min(end).min(p + ZEROS.len()) } else if p < s { s.min(end) } else { end };
                    if inside {
                        z.update(&ZEROS[..stop - p]);
                    } else {
                        z.update(&part[p - self.at..stop - self.at]);
                    }
                    p = stop;
                }
            }
            self.at += n;
            bytes = rest;
        }
    }

    /// Leaf `k` done (more bytes follow): its chaining values kept, the next leaf begun.
    fn close_leaf(&mut self, k: usize) {
        self.leaves.push(self.cur.finalize_non_root());
        if let Some(z) = self.cur_z.take() {
            self.zeroed.push((k, z.finalize_non_root()));
        }
        self.cur = leaf_hasher(k + 1);
        self.cur_z = self.touches(k + 1).then(|| leaf_hasher(k + 1));
    }

    /// Bytes taken so far.
    pub fn len(&self) -> usize {
        self.at
    }

    pub fn is_empty(&self) -> bool {
        self.at == 0
    }

    /// Every byte has come: (the plain hash, the hash with the slot as zeros).
    pub fn finish(mut self) -> ([u8; 32], [u8; 32]) {
        let n = self.at;
        if n <= LEAF {
            // One leaf: its own root, no merge.
            let a = *self.cur.finalize().as_bytes();
            let z = self.cur_z.as_ref().map(|z| *z.finalize().as_bytes()).unwrap_or(a);
            return (a, z);
        }
        let k = (n - 1) >> LEAF_LOG2;
        self.leaves.push(self.cur.finalize_non_root());
        if let Some(z) = self.cur_z.take() {
            self.zeroed.push((k, z.finalize_non_root()));
        }
        let plain = root(&self.leaves, 0, n, &[]);
        let zeroed = root(&self.leaves, 0, n, &self.zeroed);
        (plain, zeroed)
    }
}

/// The root of the bytes from `from` (a leaf's start) to `to` (the file's end) over `leaves`, a leaf in `swap` taking its chaining value from there.
fn root(leaves: &[ChainingValue], from: usize, to: usize, swap: &[(usize, ChainingValue)]) -> [u8; 32] {
    let left = blake3::hazmat::left_subtree_len((to - from) as u64) as usize;
    let l = node(leaves, from, from + left, swap);
    let r = node(leaves, from + left, to, swap);
    *merge_subtrees_root(&l, &r, Mode::Hash).as_bytes()
}

/// A subtree's chaining value: one leaf as it is (or its twin), else its two halves merged, split where BLAKE3 splits them.
fn node(leaves: &[ChainingValue], from: usize, to: usize, swap: &[(usize, ChainingValue)]) -> ChainingValue {
    if to - from <= LEAF {
        let k = from >> LEAF_LOG2;
        return swap.iter().find(|s| s.0 == k).map(|s| s.1).unwrap_or(leaves[k]);
    }
    let left = blake3::hazmat::left_subtree_len((to - from) as u64) as usize;
    let l = node(leaves, from, from + left, swap);
    let r = node(leaves, from + left, to, swap);
    merge_subtrees_non_root(&l, &r, Mode::Hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn data(n: usize, seed: u32) -> Vec<u8> {
        let mut x = seed.wrapping_mul(2654435761) | 1;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect()
    }

    fn both(d: &[u8], slot: (usize, usize)) -> ([u8; 32], [u8; 32]) {
        let mut z = d.to_vec();
        z[slot.0..slot.0 + slot.1].fill(0);
        (*blake3::hash(d).as_bytes(), *blake3::hash(&z).as_bytes())
    }

    #[test]
    fn the_twins_are_blake3_of_each_input_at_every_length_and_split() {
        let lens = [1usize, 100, 1023, 1024, 1025, 4096, LEAF - 1, LEAF, LEAF + 1, 2 * LEAF, 2 * LEAF + 7, 3 * LEAF + 1024, 5 * LEAF - 3, 8 * LEAF, 8 * LEAF + 1, 17 * LEAF + 999, 64 * LEAF + 5];
        for (i, &n) in lens.iter().enumerate() {
            let d = data(n, i as u32 + 1);
            let slots = [(0usize, n.min(10)), (n / 3, (n - n / 3).min(8600)), (n.saturating_sub(64), n.min(64)), (LEAF.saturating_sub(100).min(n - 1), 1.max((n - LEAF.saturating_sub(100).min(n - 1)).min(300)))];
            for &slot in &slots {
                let want = both(&d, slot);
                for step in [1usize << 20, 1 << 18, LEAF, 4093, 1000] {
                    let mut t = Twin::new(slot);
                    for c in d.chunks(step) {
                        t.update(c);
                    }
                    assert_eq!(t.finish(), want, "length {n}, slot {slot:?}, pieces of {step}");
                }
            }
        }
    }

    #[test]
    fn no_slot_gives_the_address_twice() {
        let d = data(3 * LEAF + 5, 9);
        let mut t = Twin::new((0, 0));
        t.update(&d);
        let h = *blake3::hash(&d).as_bytes();
        assert_eq!(t.finish(), (h, h));
        let _ = vec![0u8; 1];
    }
}
