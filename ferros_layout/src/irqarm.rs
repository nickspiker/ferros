//! An armed action: a few register operations a driver arms on its own granted interrupt line, which the kernel runs when the line fires, before the driver is woken and without the kernel lock (SYS_IRQ_ARM; docs/KERNEL.md "Armed actions").
//! Why (camera-full-rate, 2026-11-18, the Pro at 96 a second): the camera's next PDMA descriptor must be released inside the frame's blanking, 403 us; the driver's wake waits for the kernel lock, which other work held 1 to 148 ms; a release 2 ms late stopped the stream every time.
//! The encoding and its check live here, shared by the kernel (which checks once, at the arm) and the processes (which encode), and tested on the host.

/// Go on only if `read(off) & a != 0`.
pub const TEST_BITS: u32 = 1;
/// Go on only if `read(off) == read(b)`, `b` a second offset.
pub const TEST_EQ: u32 = 2;
/// `write(off, (read(off) + a) & b)`, `b` a mask of the form 2^k - 1: a ring index advanced.
pub const ADVANCE: u32 = 3;
/// `write(off, b)`.
pub const WRITE: u32 = 4;

/// The most operations a line may be armed with.
pub const OPS_MOST: usize = 1 << 2;
/// One operation's bytes: op, offset, a, b, little-endian words.
pub const OP_BYTES: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Op {
    pub op: u32,
    pub off: u32,
    pub a: u32,
    pub b: u32,
}

/// Why an armed program was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not a whole number of operations, none, or more than `OPS_MOST`.
    Length,
    /// An operation number this kernel has none of.
    Op,
    /// An offset not on a register's 4 bytes, or past the grant.
    Offset,
    /// A test with no bits, or an advance whose mask is not 2^k - 1.
    Mask,
}

/// The operations as bytes, for SYS_IRQ_ARM: `OP_BYTES` each into `out`; the bytes used.
pub fn encode(ops: &[Op], out: &mut [u8]) -> usize {
    for (k, o) in ops.iter().enumerate() {
        for (j, w) in [o.op, o.off, o.a, o.b].iter().enumerate() {
            out[k * OP_BYTES + j * 4..k * OP_BYTES + j * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
    }
    ops.len() * OP_BYTES
}

/// The program checked against a hardware grant of `grant` bytes: every offset a whole register inside it, every operation known, its masks sane. The operations and how many.
/// This is the whole check: the kernel runs a program that passed it with no test of its own at interrupt time.
pub fn check(bytes: &[u8], grant: usize) -> Result<([Op; OPS_MOST], usize), Refusal> {
    let n = bytes.len() / OP_BYTES;
    if bytes.len() % OP_BYTES != 0 || n == 0 || n > OPS_MOST {
        return Err(Refusal::Length);
    }
    let reg = |off: u32| off % 4 == 0 && (off as usize) < grant && grant - (off as usize) >= 4;
    let mut ops = [Op::default(); OPS_MOST];
    for (k, o) in ops.iter_mut().take(n).enumerate() {
        let w = |j: usize| u32::from_le_bytes([bytes[k * OP_BYTES + j * 4], bytes[k * OP_BYTES + j * 4 + 1], bytes[k * OP_BYTES + j * 4 + 2], bytes[k * OP_BYTES + j * 4 + 3]]);
        *o = Op { op: w(0), off: w(1), a: w(2), b: w(3) };
        if !reg(o.off) {
            return Err(Refusal::Offset);
        }
        match o.op {
            TEST_BITS if o.a == 0 => return Err(Refusal::Mask),
            TEST_BITS | WRITE => {}
            TEST_EQ if !reg(o.b) => return Err(Refusal::Offset),
            TEST_EQ => {}
            // WHY wrapping: b = 2^32 - 1 (the whole word) is a sane mask whose successor is 0; the test asks that b + 1 has no bit in common with b.
            ADVANCE if o.b == 0 || o.b & o.b.wrapping_add(1) != 0 => return Err(Refusal::Mask),
            ADVANCE => {}
            _ => return Err(Refusal::Op),
        }
    }
    Ok((ops, n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(ops: &[Op]) -> ([u8; OPS_MOST * OP_BYTES + OP_BYTES], usize) {
        let mut b = [0u8; OPS_MOST * OP_BYTES + OP_BYTES];
        let mut n = 0;
        for o in ops {
            n += encode(core::slice::from_ref(o), &mut b[n..]);
        }
        (b, n)
    }

    /// The camera's program on its front end's 1 MiB grant (payloads/camera): DMA_DONE, every released program run, the index advanced round a 2^14-byte ring.
    #[test]
    fn the_cameras_program() {
        let cam = [Op { op: TEST_BITS, off: 0x2_0424, a: 0x10, b: 0 }, Op { op: TEST_EQ, off: 0x9_0014, a: 0, b: 0x9_0010 }, Op { op: ADVANCE, off: 0x9_0010, a: 16, b: (1 << 14) - 1 }];
        let (b, n) = bytes(&cam);
        let (ops, k) = check(&b[..n], 1 << 20).unwrap();
        assert_eq!(k, 3);
        assert_eq!(&ops[..3], &cam);
    }

    #[test]
    fn lengths() {
        let w = Op { op: WRITE, off: 0, a: 0, b: 1 };
        assert_eq!(check(&[], 1 << 12), Err(Refusal::Length));
        let (b, n) = bytes(&[w; OPS_MOST + 1]);
        assert_eq!(check(&b[..n], 1 << 12), Err(Refusal::Length));
        let (b, n) = bytes(&[w; OPS_MOST]);
        assert!(check(&b[..n], 1 << 12).is_ok());
        assert_eq!(check(&b[..n - 1], 1 << 12), Err(Refusal::Length));
    }

    #[test]
    fn offsets() {
        let at = |off: u32| check(&bytes(&[Op { op: WRITE, off, a: 0, b: 0 }]).0[..OP_BYTES], 1 << 12);
        assert!(at(0).is_ok());
        assert!(at((1 << 12) - 4).is_ok());
        assert_eq!(at(1 << 12), Err(Refusal::Offset));
        assert_eq!(at((1 << 12) - 2), Err(Refusal::Offset));
        assert_eq!(at(2), Err(Refusal::Offset));
        assert_eq!(at(u32::MAX - 3), Err(Refusal::Offset));
        // A grant under one register holds none.
        assert_eq!(check(&bytes(&[Op { op: WRITE, off: 0, a: 0, b: 0 }]).0[..OP_BYTES], 2), Err(Refusal::Offset));
        // TEST_EQ's second register is held to the grant too.
        let eq = |b: u32| check(&bytes(&[Op { op: TEST_EQ, off: 0, a: 0, b }]).0[..OP_BYTES], 1 << 12);
        assert!(eq(8).is_ok());
        assert_eq!(eq(1 << 12), Err(Refusal::Offset));
        assert_eq!(eq(6), Err(Refusal::Offset));
    }

    #[test]
    fn masks_and_ops() {
        let one = |o: Op| check(&bytes(&[o]).0[..OP_BYTES], 1 << 12);
        assert_eq!(one(Op { op: TEST_BITS, off: 0, a: 0, b: 0 }), Err(Refusal::Mask));
        for good in [1u32, 0xF, (1 << 14) - 1, u32::MAX] {
            assert!(one(Op { op: ADVANCE, off: 0, a: 16, b: good }).is_ok(), "mask {good:X}");
        }
        for bad in [0u32, 0xE, 0x10, 0x3FF0] {
            assert_eq!(one(Op { op: ADVANCE, off: 0, a: 16, b: bad }), Err(Refusal::Mask), "mask {bad:X}");
        }
        for op in [0u32, 5, u32::MAX] {
            assert_eq!(one(Op { op, off: 0, a: 1, b: 1 }), Err(Refusal::Op));
        }
    }
}
