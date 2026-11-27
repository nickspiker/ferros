//! A death as typed fields (LEDGER.md "Deaths"): a process's end (category `kernel.process`), or the kernel's own panic or fault found at the next boot (`kernel.panic`).
//!
//! The fields go in the sealed payload beside the category, the writer and the words, and in the `ledger_open` document that answers `diag log`, so a reader takes the message, the place in the source and the call chain as values, never by reading them back out of the words.
//! The words still say it all in a sentence, for a person reading `diag log`; nothing parses them.
//!
//! Every field but `end` is absent when it does not apply: an exit has no message, a panic no ESR, a kernel panic no arc.
//! A reader from before these fields reads the entry as before (vsf's schema parse passes over a field it does not know).

use alloc::string::String;
use alloc::vec::Vec;
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint, ValidationError};
use vsf::VsfType;

use crate::entry::{lean, text_constraint, uint};

/// How it ended (`end`). The numbers are on the flash: append, never renumber.
/// A process that called exit (or its last thread's exit).
pub const END_EXIT: u64 = 0;
/// A process that panicked: its panic handler (or std's hook) handed the kernel its message and place (SYS_PANIC), then exited.
pub const END_PANIC: u64 = 1;
/// A process that faulted: the kernel ended it at the trap.
pub const END_FAULT: u64 = 2;
/// The kernel panicked, found at the next boot in the panic record (platform.rs `record_panic`).
pub const END_KERNEL_PANIC: u64 = 3;
/// The kernel took a synchronous exception early in its boot, found at the next boot in the fault sink's record (main.rs `.Lexc_recover`).
pub const END_KERNEL_FAULT: u64 = 4;
/// A process ended by the kernel or another process (a kill, a restart).
pub const END_KILLED: u64 = 5;

pub const END: &str = "end";
pub const EXIT: &str = "exit";
pub const MESSAGE: &str = "message";
pub const FILE: &str = "file";
pub const LINE: &str = "line";
pub const COLUMN: &str = "column";
pub const THREAD: &str = "thread";
pub const ESR: &str = "esr";
pub const FAR: &str = "far";
pub const PC: &str = "pc";
pub const LR: &str = "lr";
pub const FRAMES: &str = "frames";
pub const CODE: &str = "code";
pub const LAST_WORDS: &str = "last_words";
pub const OF_BOOT: &str = "of_boot";
pub const CORE: &str = "core";
pub const CRUMB: &str = "crumb";

/// The longest message, file name and last line kept, in bytes (cut at a character boundary): an entry is one 4 KiB block at most (ring.rs splits only the words, never these), and these with the words and the frames stay well inside it.
pub const MESSAGE_MAX: usize = 1 << 9;
pub const FILE_MAX: usize = 1 << 8;
pub const WORDS_MAX: usize = 1 << 8;
/// The most frames kept: return addresses, innermost first.
pub const FRAMES_MAX: usize = 1 << 5;

/// A death's fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Death {
    /// How it ended (`END_*`).
    pub end: u64,
    /// The process's exit value.
    pub exit: Option<u64>,
    /// The panic's message, as the panic formatted it.
    pub message: Option<String>,
    /// Where it panicked: the source file as the compiler named it, its line and column.
    pub file: Option<String>,
    pub line: Option<u64>,
    pub column: Option<u64>,
    /// The thread that panicked or faulted, by the name it gave itself (SYS_THREAD_NAME), else the kernel's name for it (`name.k`).
    pub thread: Option<String>,
    /// A fault's syndrome, faulting address and PC (the PC an offset into the arc for a process, the kernel's own address for the kernel).
    pub esr: Option<u64>,
    pub far: Option<u64>,
    pub pc: Option<u64>,
    /// A fault's link register, as the PC is: a leaf function's caller, which no frame record holds yet.
    pub lr: Option<u64>,
    /// The call chain, innermost first: where it was (the panic's call into the kernel, or the faulting PC), then each return address from the frame-pointer walk. A process's are offsets into its arc.
    pub frames: Vec<u64>,
    /// Where in the arc the image starts (process.rs CODE): a frame less this is the address in the ELF the image was made from (payloads and std programs link at 0).
    pub code: Option<u64>,
    /// The last console line it printed.
    pub last_words: Option<String>,
    /// For the kernel's own end, found at the next boot: the boot it happened in (0 when the flash log was not up yet), the core, and the last crumb.
    pub of_boot: Option<u64>,
    pub core: Option<u64>,
    pub crumb: Option<u64>,
}

/// `s` cut to at most `max` bytes, at a character boundary.
pub fn cut(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut n = max;
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    &s[..n]
}

/// The death's fields added to a section's schema (the sealed payload's and `ledger_open`'s).
pub fn fields(s: SectionSchema) -> SectionSchema {
    s.field(END, TypeConstraint::AnyUnsigned)
        .field(EXIT, TypeConstraint::AnyUnsigned)
        .field(MESSAGE, text_constraint())
        .field(FILE, text_constraint())
        .field(LINE, TypeConstraint::AnyUnsigned)
        .field(COLUMN, TypeConstraint::AnyUnsigned)
        .field(THREAD, text_constraint())
        .field(ESR, TypeConstraint::AnyUnsigned)
        .field(FAR, TypeConstraint::AnyUnsigned)
        .field(PC, TypeConstraint::AnyUnsigned)
        .field(LR, TypeConstraint::AnyUnsigned)
        .field(FRAMES, TypeConstraint::AnyUnsigned)
        .field(CODE, TypeConstraint::AnyUnsigned)
        .field(LAST_WORDS, text_constraint())
        .field(OF_BOOT, TypeConstraint::AnyUnsigned)
        .field(CORE, TypeConstraint::AnyUnsigned)
        .field(CRUMB, TypeConstraint::AnyUnsigned)
}

/// Set the death's fields on a builder; the characters the text table did not hold, counted (`lean`).
pub fn set(mut b: SectionBuilder, d: &Death) -> Result<(SectionBuilder, u64), ValidationError> {
    let mut refused = 0u64;
    b = b.set(END, uint(d.end))?;
    for (name, v) in [(EXIT, d.exit), (LINE, d.line), (COLUMN, d.column), (ESR, d.esr), (FAR, d.far), (PC, d.pc), (LR, d.lr), (CODE, d.code), (OF_BOOT, d.of_boot), (CORE, d.core), (CRUMB, d.crumb)] {
        if let Some(v) = v {
            b = b.set(name, uint(v))?;
        }
    }
    for (name, t, max) in [(MESSAGE, &d.message, MESSAGE_MAX), (FILE, &d.file, FILE_MAX), (THREAD, &d.thread, FILE_MAX), (LAST_WORDS, &d.last_words, WORDS_MAX)] {
        if let Some(t) = t {
            let (v, r) = lean(cut(t, max));
            refused += r;
            b = b.set(name, v)?;
        }
    }
    if !d.frames.is_empty() {
        let frames: Vec<VsfType> = d.frames.iter().take(FRAMES_MAX).map(|&f| uint(f)).collect();
        b = b.set_multi(FRAMES, frames)?;
    }
    Ok((b, refused))
}

/// The death a parsed section carries, if it carries one (its `end` is there).
pub fn get(s: &SectionBuilder) -> Option<Death> {
    let end = s.get_value::<u64>(END).ok()?;
    let n = |name: &str| s.get_value::<u64>(name).ok();
    let t = |name: &str| s.get_value::<String>(name).ok();
    let frames = s.get(FRAMES).map(|v| v.iter().filter_map(|x| <u64 as vsf::schema::FromVsfType>::from_vsf_type(x).ok()).collect()).unwrap_or_default();
    Some(Death {
        end,
        exit: n(EXIT),
        message: t(MESSAGE),
        file: t(FILE),
        line: n(LINE),
        column: n(COLUMN),
        thread: t(THREAD),
        esr: n(ESR),
        far: n(FAR),
        pc: n(PC),
        lr: n(LR),
        frames,
        code: n(CODE),
        last_words: t(LAST_WORDS),
        of_boot: n(OF_BOOT),
        core: n(CORE),
        crumb: n(CRUMB),
    })
}

/// How it ended, in words.
pub fn end_name(end: u64) -> &'static str {
    match end {
        END_EXIT => "ended",
        END_PANIC => "panicked",
        END_FAULT => "faulted",
        END_KERNEL_PANIC => "the kernel panicked",
        END_KERNEL_FAULT => "the kernel faulted",
        END_KILLED => "was ended",
        _ => "ended (a kind this build does not know)",
    }
}
