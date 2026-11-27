//! The WebAssembly vocabulary the translator speaks: index types, value types, the error type.
//! In place of wasmtime-environ's, which carry a runtime toka does not have (GC, components, a VMContext).

use cranelift_codegen::ir;
use cranelift_entity::entity_impl;

/// A function, imported ones first.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct FuncIndex(u32);
entity_impl!(FuncIndex);

/// A type in the type section.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct TypeIndex(u32);
entity_impl!(TypeIndex);

/// A global, imported ones first.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct GlobalIndex(u32);
entity_impl!(GlobalIndex);

/// A linear memory.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct MemoryIndex(u32);
entity_impl!(MemoryIndex);

/// A table.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct TableIndex(u32);
entity_impl!(TableIndex);

/// A data segment.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct DataIndex(u32);
entity_impl!(DataIndex);

/// The value types toka takes: numbers and vectors; references (GC, function references) are refused.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum WasmValType {
    I32,
    I64,
    F32,
    F64,
    V128,
}

impl WasmValType {
    pub fn clif(self) -> ir::Type {
        match self {
            WasmValType::I32 => ir::types::I32,
            WasmValType::I64 => ir::types::I64,
            WasmValType::F32 => ir::types::F32,
            WasmValType::F64 => ir::types::F64,
            WasmValType::V128 => ir::types::I8X16,
        }
    }
}

/// A function type: parameters and results.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct FuncType {
    pub params: Vec<WasmValType>,
    pub results: Vec<WasmValType>,
}

/// Why a module was refused.
#[derive(Debug)]
pub enum WasmError {
    /// The module is not valid WebAssembly (wasmparser's validation).
    Invalid { message: String, offset: usize },
    /// Valid, but uses something toka does not take (a proposal, a reference type, an import).
    Unsupported(String),
    /// A limit of this implementation.
    ImplLimitExceeded,
    /// Cranelift refused the function it was given (a bug here, or there).
    User(String),
}

impl core::fmt::Display for WasmError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            WasmError::Invalid { message, offset } => write!(f, "invalid WebAssembly at byte {offset}: {message}"),
            WasmError::Unsupported(s) => write!(f, "not taken by toka: {s}"),
            WasmError::ImplLimitExceeded => write!(f, "beyond a limit of toka"),
            WasmError::User(s) => write!(f, "{s}"),
        }
    }
}

impl From<wasmparser::BinaryReaderError> for WasmError {
    fn from(e: wasmparser::BinaryReaderError) -> Self {
        WasmError::Invalid {
            message: e.message().to_string(),
            offset: e.offset() as usize,
        }
    }
}

pub type WasmResult<T> = Result<T, WasmError>;

/// Refuse with a formatted reason.
#[macro_export]
macro_rules! wasm_unsupported {
    ($($arg:tt)*) => { $crate::types::WasmError::Unsupported(format!($($arg)*)) }
}

/// wasmparser's value type in toka's terms.
pub fn convert_valtype(ty: wasmparser::ValType) -> WasmResult<WasmValType> {
    Ok(match ty {
        wasmparser::ValType::I32 => WasmValType::I32,
        wasmparser::ValType::I64 => WasmValType::I64,
        wasmparser::ValType::F32 => WasmValType::F32,
        wasmparser::ValType::F64 => WasmValType::F64,
        wasmparser::ValType::V128 => WasmValType::V128,
        wasmparser::ValType::Ref(r) => return Err(wasm_unsupported!("reference type {r:?}")),
    })
}
