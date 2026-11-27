//! WebAssembly to Cranelift IR, operator by operator: wasmtime's translator (crates/cranelift/src/translate at v51.0.0, Apache-2.0 WITH LLVM-exception; NOTICE beside this file), with what toka refuses cut out (garbage collection, exceptions, stack switching, reference types and tables beyond call_indirect) and its memory accesses made toka's (environ.rs).

mod code_translator;
mod func_translator;
mod stack;
mod translation_utils;

pub use self::func_translator::FuncTranslator;
pub use self::stack::FuncTranslationStacks;

