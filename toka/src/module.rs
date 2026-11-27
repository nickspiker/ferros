//! A module read and validated: its types, functions, memories, globals, table, segments and exports, in the shape the translator and the linker need.

use crate::types::*;
use crate::wasm_unsupported;
use wasmparser::{FuncValidator, FunctionBody, Parser, Payload, ValidPayload, Validator, ValidatorResources, WasmFeatures};

/// A function imported from outside the module: reached by name at link time (toka's own intrinsics, docs/TOKA.md).
#[derive(Clone, Debug)]
pub struct Import {
    pub module: String,
    pub name: String,
}

/// A memory imported from "ferros": a region another process shares, reached thru a handle slot of the process's table (docs/TOKA.md "Memories"): `share` read-write, `view` read-only.
#[derive(Clone, Copy, Debug)]
pub struct Handle {
    /// The slot its accesses name: the i-th imported handle memory takes slot 31 - i, which the program binds a view to with SYS_SHARE's BIND.
    pub slot: u8,
    pub read_only: bool,
}

#[derive(Clone, Debug)]
pub struct Memory {
    /// Some for a memory imported from "ferros" (a shared region); None for the module's own, which lives in its arc after the image.
    pub handle: Option<Handle>,
    pub memory64: bool,
    pub shared: bool,
    /// Initial and maximum size in pages of 2^page_log2 bytes.
    pub initial: u64,
    pub maximum: Option<u64>,
    pub page_log2: u32,
}

#[derive(Clone, Debug)]
pub struct Global {
    pub ty: WasmValType,
    pub mutable: bool,
    /// Its value at start (a constant expression folded: i32/i64/f32/f64/v128 constants only), as the little-endian bytes it holds.
    pub init: [u8; 16],
}

#[derive(Clone, Debug)]
pub struct Table {
    pub initial: u64,
    pub maximum: Option<u64>,
    /// The functions placed at each slot by active element segments (None: a null slot, which traps when called).
    pub slots: Vec<Option<FuncIndex>>,
}

#[derive(Clone, Debug)]
pub struct Data {
    /// Active: its memory and its offset there; passive: None.
    pub active: Option<(MemoryIndex, u64)>,
    pub bytes: Vec<u8>,
}

/// One function body, with its validator (translation validates as it goes, operator by operator).
pub struct Body<'a> {
    pub index: FuncIndex,
    pub body: FunctionBody<'a>,
    pub validator: FuncValidator<ValidatorResources>,
}

pub struct Module<'a> {
    pub types: Vec<FuncType>,
    /// Every function's type, imported functions first.
    pub functions: Vec<TypeIndex>,
    pub imports: Vec<Import>,
    pub memories: Vec<Memory>,
    pub globals: Vec<Global>,
    pub tables: Vec<Table>,
    pub data: Vec<Data>,
    pub exports: Vec<(String, FuncIndex)>,
    pub start: Option<FuncIndex>,
    pub bodies: Vec<Body<'a>>,
}

impl Module<'_> {
    pub fn num_imported_funcs(&self) -> usize {
        self.imports.len()
    }
    pub fn func_type(&self, f: FuncIndex) -> &FuncType {
        &self.types[self.functions[f.as_u32() as usize].as_u32() as usize]
    }
}

/// The proposals toka takes (docs/TOKA.md "What toka takes"): the MVP and everything numeric, vector, threaded and 64-bit; none that needs a garbage collector, exceptions or stack switching.
pub fn features() -> WasmFeatures {
    WasmFeatures::MUTABLE_GLOBAL
        | WasmFeatures::SATURATING_FLOAT_TO_INT
        | WasmFeatures::SIGN_EXTENSION
        | WasmFeatures::MULTI_VALUE
        | WasmFeatures::BULK_MEMORY
        | WasmFeatures::SIMD
        | WasmFeatures::RELAXED_SIMD
        | WasmFeatures::THREADS
        | WasmFeatures::TAIL_CALL
        | WasmFeatures::MEMORY64
        | WasmFeatures::MULTI_MEMORY
        | WasmFeatures::EXTENDED_CONST
        | WasmFeatures::WIDE_ARITHMETIC
        | WasmFeatures::FLOATS
}

fn const_value(expr: &wasmparser::ConstExpr<'_>, ty: WasmValType) -> WasmResult<[u8; 16]> {
    let mut out = [0u8; 16];
    let mut ops = expr.get_operators_reader();
    let op = ops.read()?;
    match op {
        wasmparser::Operator::I32Const { value } => out[..4].copy_from_slice(&value.to_le_bytes()),
        wasmparser::Operator::I64Const { value } => out[..8].copy_from_slice(&value.to_le_bytes()),
        wasmparser::Operator::F32Const { value } => out[..4].copy_from_slice(&value.bits().to_le_bytes()),
        wasmparser::Operator::F64Const { value } => out[..8].copy_from_slice(&value.bits().to_le_bytes()),
        wasmparser::Operator::V128Const { value } => out.copy_from_slice(value.bytes()),
        other => return Err(wasm_unsupported!("constant expression {other:?} for a {ty:?}")),
    }
    match ops.read()? {
        wasmparser::Operator::End => Ok(out),
        other => Err(wasm_unsupported!("constant expression with {other:?} (only single constants)")),
    }
}

fn const_offset(expr: &wasmparser::ConstExpr<'_>) -> WasmResult<u64> {
    let mut ops = expr.get_operators_reader();
    let v = match ops.read()? {
        wasmparser::Operator::I32Const { value } => u64::from(value as u32),
        wasmparser::Operator::I64Const { value } => value as u64,
        other => return Err(wasm_unsupported!("segment offset {other:?} (only constants)")),
    };
    match ops.read()? {
        wasmparser::Operator::End => Ok(v),
        other => Err(wasm_unsupported!("segment offset with {other:?}")),
    }
}

/// Read and validate `bytes`.
pub fn parse(bytes: &[u8]) -> WasmResult<Module<'_>> {
    let mut validator = Validator::new_with_features(features());
    let mut m = Module {
        types: vec![],
        functions: vec![],
        imports: vec![],
        memories: vec![],
        globals: vec![],
        tables: vec![],
        data: vec![],
        exports: vec![],
        start: None,
        bodies: vec![],
    };
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload?;
        match validator.payload(&payload)? {
            ValidPayload::Func(func, body) => {
                let index = FuncIndex::from_u32(func.index);
                m.bodies.push(Body {
                    index,
                    body,
                    validator: func.into_validator(Default::default()),
                });
                continue;
            }
            ValidPayload::Ok | ValidPayload::Parser(_) | ValidPayload::End(_) => {}
        }
        match payload {
            Payload::TypeSection(r) => {
                for ty in r.into_iter_err_on_gc_types() {
                    let ty = ty?;
                    m.types.push(FuncType {
                        params: ty.params().iter().map(|t| convert_valtype(*t)).collect::<WasmResult<_>>()?,
                        results: ty.results().iter().map(|t| convert_valtype(*t)).collect::<WasmResult<_>>()?,
                    });
                }
            }
            Payload::ImportSection(r) => {
                for import in r.into_imports() {
                    let import = import?;
                    match import.ty {
                        wasmparser::TypeRef::Func(t) => {
                            m.functions.push(TypeIndex::from_u32(t));
                            m.imports.push(Import {
                                module: import.module.to_string(),
                                name: import.name.to_string(),
                            });
                        }
                        wasmparser::TypeRef::Memory(mem) if import.module == "ferros" && (import.name == "share" || import.name == "view") => {
                            let n = m.memories.iter().filter(|x| x.handle.is_some()).count();
                            if n >= 32 {
                                return Err(wasm_unsupported!("more handle memories than a handle table holds (32)"));
                            }
                            m.memories.push(Memory {
                                handle: Some(Handle { slot: 31 - n as u8, read_only: import.name == "view" }),
                                memory64: mem.memory64,
                                shared: mem.shared,
                                initial: mem.initial,
                                maximum: mem.maximum,
                                page_log2: mem.page_size_log2.unwrap_or(16),
                            });
                        }
                        other => return Err(wasm_unsupported!("an import of {other:?} ({}.{}): toka takes function imports and \"ferros\" \"share\" / \"view\" memories", import.module, import.name)),
                    }
                }
            }
            Payload::FunctionSection(r) => {
                for t in r {
                    m.functions.push(TypeIndex::from_u32(t?));
                }
            }
            Payload::TableSection(r) => {
                for t in r {
                    let t = t?;
                    if !t.ty.element_type.is_func_ref() {
                        return Err(wasm_unsupported!("a table of {:?}", t.ty.element_type));
                    }
                    if !matches!(t.init, wasmparser::TableInit::RefNull) {
                        return Err(wasm_unsupported!("a table with an initializer"));
                    }
                    m.tables.push(Table {
                        initial: t.ty.initial,
                        maximum: t.ty.maximum,
                        slots: vec![None; t.ty.initial as usize],
                    });
                }
            }
            Payload::MemorySection(r) => {
                for mem in r {
                    let mem = mem?;
                    m.memories.push(Memory {
                        handle: None,
                        memory64: mem.memory64,
                        shared: mem.shared,
                        initial: mem.initial,
                        maximum: mem.maximum,
                        page_log2: mem.page_size_log2.unwrap_or(16),
                    });
                }
            }
            Payload::GlobalSection(r) => {
                for g in r {
                    let g = g?;
                    let ty = convert_valtype(g.ty.content_type)?;
                    if g.ty.shared {
                        return Err(wasm_unsupported!("a shared global"));
                    }
                    m.globals.push(Global {
                        ty,
                        mutable: g.ty.mutable,
                        init: const_value(&g.init_expr, ty)?,
                    });
                }
            }
            Payload::ExportSection(r) => {
                for e in r {
                    let e = e?;
                    if let wasmparser::ExternalKind::Func = e.kind {
                        m.exports.push((e.name.to_string(), FuncIndex::from_u32(e.index)));
                    }
                }
            }
            Payload::StartSection { func, .. } => m.start = Some(FuncIndex::from_u32(func)),
            Payload::ElementSection(r) => {
                for e in r {
                    let e = e?;
                    let wasmparser::ElementKind::Active { table_index, offset_expr } = e.kind else {
                        return Err(wasm_unsupported!("a passive or declared element segment"));
                    };
                    let table = table_index.unwrap_or(0) as usize;
                    let offset = const_offset(&offset_expr)? as usize;
                    let wasmparser::ElementItems::Functions(funcs) = e.items else {
                        return Err(wasm_unsupported!("an element segment of expressions"));
                    };
                    for (k, f) in funcs.into_iter().enumerate() {
                        let f = FuncIndex::from_u32(f?);
                        let slots = &mut m.tables[table].slots;
                        let at = offset + k;
                        if at >= slots.len() {
                            return Err(WasmError::Invalid { message: "element segment past its table".into(), offset: 0 });
                        }
                        slots[at] = Some(f);
                    }
                }
            }
            Payload::DataSection(r) => {
                for d in r {
                    let d = d?;
                    let active = match d.kind {
                        wasmparser::DataKind::Passive => None,
                        wasmparser::DataKind::Active { memory_index, offset_expr } => {
                            if m.memories[memory_index as usize].handle.is_some() {
                                return Err(wasm_unsupported!("an active data segment into a shared memory (its holder's bytes are its server's)"));
                            }
                            Some((MemoryIndex::from_u32(memory_index), const_offset(&offset_expr)?))
                        }
                    };
                    m.data.push(Data { active, bytes: d.data.to_vec() });
                }
            }
            _ => {}
        }
    }
    Ok(m)
}
