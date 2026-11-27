//! The toka image: functions laid out, their references resolved, every instruction held to the sandbox checker (docs/TOKA.md "The image").
//!
//! Layout, from the image's start (which the kernel places at arc offset G#1000, a 4 KiB boundary, so ADRP's pages are the arc's):
//! the code (each function on a 16-byte boundary, padding `brk #0`); the globals area; each table; each memory's size cell; each data segment's bytes; then each memory, on a 64 KiB boundary, its active segments in place.
//! Everything is reached PC-relatively, so the image runs wherever its arc is.

use crate::environ::*;
use crate::module::Module;
use crate::{Compiled, Mask, Options};
use cranelift_codegen::binemit;
use cranelift_codegen::ir::UserExternalName;

/// A reference out of a function, by name.
#[derive(Clone, Debug)]
pub struct Reloc {
    pub offset: u32,
    pub kind: binemit::Reloc,
    pub target: UserExternalName,
    pub addend: i64,
}

pub struct Function {
    pub name: UserExternalName,
    pub offset: usize,
    pub len: usize,
}

pub struct Image {
    pub bytes: Vec<u8>,
    /// How many bytes from the start are instructions (the span the checker covers, and the capsule declares).
    pub text_len: usize,
    pub functions: Vec<Function>,
    /// Instructions the checker accepted.
    pub checked: usize,
}

/// `brk #0`: padding the checker accepts and that ends the process if ever reached.
const BRK_0: u32 = 0xD420_0000;
const MEMORY_ALIGN: usize = 1 << 16;

fn align(x: usize, a: usize) -> usize {
    (x + a - 1) & !(a - 1)
}

fn sandbox_mode(m: Mask) -> ferros_sandbox::Mode {
    match m {
        Mask::Ring => ferros_sandbox::Mode::Ring,
        Mask::Fixed(k) => ferros_sandbox::Mode::Fixed(k),
    }
}

pub fn link(module: &Module<'_>, plan: &Plan, compiled: Vec<Compiled>, opts: &Options) -> Result<Image, String> {
    // The code.
    let mut bytes: Vec<u8> = vec![];
    let mut functions = vec![];
    for c in &compiled {
        while bytes.len() % 16 != 0 {
            bytes.extend_from_slice(&BRK_0.to_le_bytes());
        }
        functions.push(Function {
            name: c.name.clone(),
            offset: bytes.len(),
            len: c.code.len(),
        });
        bytes.extend_from_slice(&c.code);
    }
    let text_len = bytes.len();

    // The data areas.
    let mut symbols: Vec<(UserExternalName, usize)> = functions.iter().map(|f| (f.name.clone(), f.offset)).collect();
    let mut put = |bytes: &mut Vec<u8>, sym: u32, at_align: usize, data: &[u8]| -> usize {
        bytes.resize(align(bytes.len(), at_align), 0);
        let at = bytes.len();
        symbols.push((UserExternalName::new(NS_DATA, sym), at));
        bytes.extend_from_slice(data);
        at
    };
    let mut globals = vec![];
    for g in &module.globals {
        globals.extend_from_slice(&g.init);
    }
    put(&mut bytes, DATA_GLOBALS, 16, &globals);
    let mut table_at = vec![];
    for (t, table) in module.tables.iter().enumerate() {
        let at = put(&mut bytes, DATA_TABLE | t as u32, 16, &vec![0u8; table.slots.len() * TABLE_ENTRY as usize]);
        table_at.push(at);
    }
    for (m, mem) in module.memories.iter().enumerate() {
        put(&mut bytes, DATA_MEMORY_PAGES | m as u32, 8, &mem.initial.to_le_bytes());
    }
    for (k, d) in module.data.iter().enumerate() {
        put(&mut bytes, DATA_SEGMENT | k as u32, 16, &d.bytes);
    }
    let mut memory_at = vec![];
    for (m, _) in module.memories.iter().enumerate() {
        bytes.resize(align(bytes.len(), MEMORY_ALIGN), 0);
        let at = bytes.len();
        symbols.push((UserExternalName::new(NS_DATA, DATA_MEMORY | m as u32), at));
        memory_at.push(at);
        // The next memory starts past this one's whole reserve; the image itself ends at the last byte a segment writes.
        if m + 1 < module.memories.len() {
            bytes.resize(at + (1usize << opts.memory_reserve_log2), 0);
        }
    }
    for d in &module.data {
        if let Some((m, offset)) = d.active {
            let at = memory_at[m.as_u32() as usize] + offset as usize;
            if bytes.len() < at + d.bytes.len() {
                bytes.resize(at + d.bytes.len(), 0);
            }
            bytes[at..at + d.bytes.len()].copy_from_slice(&d.bytes);
        }
    }

    let find = |name: &UserExternalName| -> Result<usize, String> {
        symbols
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, at)| *at)
            .ok_or_else(|| match name.namespace {
                NS_FUNC if (name.index as usize) < module.num_imported_funcs() => {
                    let i = &module.imports[name.index as usize];
                    format!("import {}.{} has nothing to bind to (toka's intrinsics come with the image, docs/TOKA.md)", i.module, i.name)
                }
                _ => format!("no symbol {name:?}"),
            })
    };

    // The tables: each slot the callee's canonical type and its offset from the table's start.
    for (t, table) in module.tables.iter().enumerate() {
        for (k, slot) in table.slots.iter().enumerate() {
            let at = table_at[t] + k * TABLE_ENTRY as usize;
            let (ty, rel) = match slot {
                None => (NULL_TYPE, 0i32),
                Some(f) => {
                    let target = find(&UserExternalName::new(NS_FUNC, f.as_u32()))?;
                    let ty = plan.canonical_type[module.functions[f.as_u32() as usize].as_u32() as usize];
                    (ty, i32::try_from(target as i64 - table_at[t] as i64).map_err(|_| "a table entry out of reach")?)
                }
            };
            bytes[at..at + 4].copy_from_slice(&ty.to_le_bytes());
            bytes[at + 4..at + 8].copy_from_slice(&rel.to_le_bytes());
        }
    }

    // The references.
    for (c, f) in compiled.iter().zip(&functions) {
        for r in &c.relocs {
            let p = f.offset + r.offset as usize;
            let s = (find(&r.target)? as i64 + r.addend) as u64;
            let mut insn = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
            match r.kind {
                binemit::Reloc::Arm64Call => {
                    let d = s as i64 - p as i64;
                    if d % 4 != 0 || d >= 1 << 27 || d < -(1 << 27) {
                        return Err(format!("a call out of BL's reach ({d} bytes)"));
                    }
                    insn = (insn & 0xFC00_0000) | ((d >> 2) as u32 & 0x03FF_FFFF);
                }
                binemit::Reloc::Aarch64AdrPrelPgHi21 => {
                    let pages = ((s & !0xFFF) as i64 - (p as u64 & !0xFFF) as i64) >> 12;
                    let v = pages as u32;
                    insn = (insn & 0x9F00_001F) | ((v & 3) << 29) | (((v >> 2) & 0x7_FFFF) << 5);
                }
                binemit::Reloc::Aarch64AddAbsLo12Nc => {
                    insn = (insn & 0xFFC0_03FF) | (((s & 0xFFF) as u32) << 10);
                }
                other => return Err(format!("a relocation toka does not resolve: {other:?}")),
            }
            bytes[p..p + 4].copy_from_slice(&insn.to_le_bytes());
        }
    }

    // The checker, function by function, so a refusal names its function.
    let mode = sandbox_mode(opts.mask);
    let mut checked = 0;
    let mut refused = vec![];
    for f in &functions {
        let span = align(f.len, 4);
        match ferros_sandbox::check_image_with(&bytes[f.offset..f.offset + span], mode, opts.bti) {
            Ok(n) => checked += n,
            Err(v) => refused.push(format!("{:?} at +G#{:X}: G#{:08X} {:?}", f.name, v.offset, v.insn, v.why)),
        }
    }
    if !refused.is_empty() {
        return Err(format!("the checker refused {} function(s):\n{}", refused.len(), refused.join("\n")));
    }
    Ok(Image { bytes, text_len, functions, checked })
}
