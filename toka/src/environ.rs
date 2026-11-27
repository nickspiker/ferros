//! toka's translation environment: how a WebAssembly function's memories, globals, table and calls become Cranelift IR (docs/TOKA.md "The image").
//!
//! There is no runtime context pointer. Everything a function reaches lies in its own image at a place the linker fixes, reached PC-relatively (ADRP and ADD): the globals, the table, each memory's size cell and each memory itself, which follows the image in the arc.
//! Nothing is bounds-checked: the masking the Cranelift fork emits makes every access safe (an address wraps inside its arc's window, and the window's unbacked rest faults), and WebAssembly's traps on an out-of-bounds access are not kept.

use crate::module::Module;
use crate::translate::FuncTranslationStacks;
use crate::types::*;
use crate::wasm_unsupported;
use cranelift_codegen::cursor::FuncCursor;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::immediates::{Imm64, Offset32};
use cranelift_codegen::ir::types::*;
use cranelift_codegen::ir::{self, InstBuilder, MemFlagsData};
use cranelift_codegen::isa::{TargetFrontendConfig, TargetIsa};
use cranelift_frontend::FunctionBuilder;
use std::borrow::Cow;

/// External-name namespaces (`ir::UserExternalName`): functions by index (imports first), toka's data symbols, toka's runtime functions.
pub const NS_FUNC: u32 = 0;
pub const NS_DATA: u32 = 1;
pub const NS_RUNTIME: u32 = 2;

/// A data symbol's index: its kind in the high 16 bits, which one in the low.
pub const DATA_GLOBALS: u32 = 0 << 16;
pub const DATA_TABLE: u32 = 1 << 16;
pub const DATA_MEMORY: u32 = 2 << 16;
pub const DATA_MEMORY_PAGES: u32 = 3 << 16;
pub const DATA_SEGMENT: u32 = 4 << 16;

/// toka's runtime functions, compiled by toka itself (runtime.rs).
pub const RUNTIME_MEMMOVE: u32 = 0;
pub const RUNTIME_MEMSET: u32 = 1;
/// The one native stub: `syscall(n, a0..a5)` as AAPCS64 (runtime.rs `syscall_stub`): the arguments moved to the kernel's registers, `svc #0`, back.
pub const RUNTIME_SYSCALL: u32 = 2;
/// The image's entry (runtime.rs `entry`): the kernel's x0 to x2 (grants, input, input's length) to the module's start function and `_ferros_main`, then exit.
pub const RUNTIME_ENTRY: u32 = 3;

/// Each global's slot in the globals area (a v128 needs 16).
pub const GLOBAL_SLOT: u32 = 16;
/// A table entry: the callee's canonical type id (u32), then its offset from the table's start (i32). A null slot holds `NULL_TYPE`, which matches no type, so calling it traps as a signature mismatch.
pub const TABLE_ENTRY: u32 = 8;
pub const NULL_TYPE: u32 = u32::MAX;

/// Trap codes (ir::TrapCode user codes; any trap is `brk` and ends the process).
pub const TRAP_UNREACHABLE: ir::TrapCode = ir::TrapCode::unwrap_user(1);
pub const TRAP_HEAP_MISALIGNED: ir::TrapCode = ir::TrapCode::unwrap_user(2);
pub const TRAP_TABLE_OUT_OF_BOUNDS: ir::TrapCode = ir::TrapCode::unwrap_user(3);
pub const TRAP_BAD_SIGNATURE: ir::TrapCode = ir::TrapCode::unwrap_user(4);

/// Alias regions (ir::AliasRegionData user ids): one per memory, one for the globals, one for the tables.
const REGION_GLOBALS: u32 = 0;
const REGION_TABLES: u32 = 1;
const REGION_MEMORY: u32 = 2;

/// What the module promises the linker: each type's canonical id (structurally equal types share one) and each memory's largest size in pages.
pub struct Plan {
    pub canonical_type: Vec<u32>,
    pub max_pages: Vec<u64>,
}

impl Plan {
    pub fn new(module: &Module<'_>, memory_reserve_log2: u32) -> Plan {
        let canonical_type = (0..module.types.len())
            .map(|k| module.types.iter().position(|t| *t == module.types[k]).unwrap() as u32)
            .collect();
        let max_pages = module
            .memories
            .iter()
            .map(|m| {
                let reserve = 1u64 << (memory_reserve_log2 - m.page_log2);
                // WHY min: a memory may grow only within the address space the image sets aside for it; a declared maximum past that reserve would let memory.grow answer success for pages laid over the next memory (link.rs places each memory one reserve after the last) or past the arc.
                // PROOF it is needed: wasm-ld declares no maximum by default and the WebAssembly limit is 2^16 pages (4 GiB) for a 32-bit memory, far past a 2^26-byte reserve, so without it the grow check compares against a size the image cannot hold.
                // It prevents no memory unsafety (masking keeps every access in the arc); it keeps memory.grow's answer true.
                m.maximum.map_or(reserve, |max| max.min(reserve))
            })
            .collect();
        Plan { canonical_type, max_pages }
    }
}

pub struct FuncEnvironment<'a> {
    pub module: &'a Module<'a>,
    pub plan: &'a Plan,
    pub isa: &'a dyn TargetIsa,
    pub stacks: FuncTranslationStacks,
    pub next_srcloc: ir::SourceLoc,
    /// The function being translated, for its parameters.
    pub func: FuncIndex,
    /// The data symbols this function has named, so each is one global value and its address is computed once (a handful per function: searched in order).
    symbols: Vec<(u32, ir::GlobalValue)>,
}

impl<'a> FuncEnvironment<'a> {
    pub fn new(module: &'a Module<'a>, plan: &'a Plan, isa: &'a dyn TargetIsa, func: FuncIndex) -> Self {
        FuncEnvironment {
            module,
            plan,
            isa,
            stacks: FuncTranslationStacks::new(),
            next_srcloc: ir::SourceLoc::default(),
            func,
            symbols: vec![],
        }
    }

    // ---- the target ----

    pub fn target_config(&self) -> TargetFrontendConfig {
        self.isa.frontend_config()
    }
    pub fn isa(&self) -> &dyn TargetIsa {
        self.isa
    }
    pub fn pointer_type(&self) -> ir::Type {
        I64
    }
    pub fn is_x86(&self) -> bool {
        false
    }
    pub fn relaxed_simd_deterministic(&self) -> bool {
        false
    }
    pub fn use_x86_pmaddubsw_for_dot(&self) -> bool {
        false
    }
    pub fn use_x86_pmulhrsw_for_relaxed_q15mul(&self) -> bool {
        false
    }
    pub fn use_blendv_for_relaxed_laneselect(&self, _ty: ir::Type) -> bool {
        false
    }

    // ---- reachability and the hooks wasmtime uses for fuel, epochs and debugging (none in toka) ----

    pub fn is_reachable(&self) -> bool {
        self.stacks.reachable()
    }
    pub fn take_branch_hint(&mut self, _offset: u64) -> Option<wasmparser::BranchHint> {
        None
    }
    pub fn before_translate_function(&mut self, _builder: &mut FunctionBuilder) -> WasmResult<()> {
        Ok(())
    }
    pub fn after_translate_function(&mut self, _builder: &mut FunctionBuilder) -> WasmResult<()> {
        Ok(())
    }
    pub fn before_translate_operator(&mut self, op: &wasmparser::Operator, _types: Option<&[WasmValType]>, _builder: &mut FunctionBuilder) -> WasmResult<()> {
        self.check_memory_use(op)
    }
    pub fn after_translate_operator(&mut self, _op: &wasmparser::Operator, _validator: &wasmparser::FuncValidator<impl wasmparser::WasmModuleResources>, _builder: &mut FunctionBuilder) -> WasmResult<()> {
        Ok(())
    }
    pub fn before_load(&mut self, _builder: &mut FunctionBuilder, _size: u8, _index: ir::Value, _offset: u64) {}
    pub fn before_store(&mut self, _builder: &mut FunctionBuilder, _size: u8, _index: ir::Value, _offset: u64) {}
    pub fn before_memory_grow(&mut self, _builder: &mut FunctionBuilder, _delta: ir::Value, _mem: MemoryIndex) {}
    pub fn translate_loop_header(&mut self, _builder: &mut FunctionBuilder) {}
    pub fn fuel_flush_consumed(&mut self, _builder: &mut FunctionBuilder) {}
    pub fn handle_before_return(&mut self, _retvals: &[ir::Value], _builder: &mut FunctionBuilder) {}
    pub fn create_state_slot(&mut self, _builder: &mut FunctionBuilder) {}
    pub fn add_state_slot_local(&mut self, _builder: &mut FunctionBuilder, _ty: WasmValType, _init: impl FnOnce(&mut Self, &mut FunctionBuilder) -> WasmResult<ir::Value>) -> WasmResult<()> {
        Ok(())
    }
    pub fn state_slot_local_set(&mut self, _builder: &mut FunctionBuilder, _local: u32, _value: ir::Value) {}

    // ---- the function's own signature ----

    pub fn clif_param_as_wasm_param(&self, index: usize) -> Option<WasmValType> {
        self.module.func_type(self.func).params.get(index).copied()
    }
    pub fn is_wasm_parameter(&self, _index: usize) -> bool {
        true
    }
    pub fn param_needs_stack_map(&self, _signature: &ir::Signature, _index: usize) -> bool {
        false
    }
    pub fn val_ty_needs_stack_map(&self, _ty: WasmValType) -> bool {
        false
    }
    pub fn convert_valtype(&self, ty: wasmparser::ValType) -> WasmResult<WasmValType> {
        convert_valtype(ty)
    }

    // ---- traps ----

    pub fn trap(&mut self, builder: &mut FunctionBuilder, code: ir::TrapCode) {
        builder.ins().trap(code);
    }
    pub fn trapz(&mut self, builder: &mut FunctionBuilder, value: ir::Value, code: ir::TrapCode) {
        builder.ins().trapz(value, code);
    }
    pub fn trapnz(&mut self, builder: &mut FunctionBuilder, value: ir::Value, code: ir::TrapCode) {
        builder.ins().trapnz(value, code);
    }
    pub fn uadd_overflow_trap(&mut self, builder: &mut FunctionBuilder, a: ir::Value, b: ir::Value, code: ir::TrapCode) -> ir::Value {
        builder.ins().uadd_overflow_trap(a, b, code)
    }

    // ---- arithmetic: aarch64 has every one natively, and Cranelift's divisions and conversions trap as WebAssembly says ----

    pub fn translate_sdiv(&mut self, b: &mut FunctionBuilder, l: ir::Value, r: ir::Value) -> ir::Value {
        b.ins().sdiv(l, r)
    }
    pub fn translate_udiv(&mut self, b: &mut FunctionBuilder, l: ir::Value, r: ir::Value) -> ir::Value {
        b.ins().udiv(l, r)
    }
    pub fn translate_srem(&mut self, b: &mut FunctionBuilder, l: ir::Value, r: ir::Value) -> ir::Value {
        b.ins().srem(l, r)
    }
    pub fn translate_urem(&mut self, b: &mut FunctionBuilder, l: ir::Value, r: ir::Value) -> ir::Value {
        b.ins().urem(l, r)
    }
    pub fn translate_fcvt_to_sint(&mut self, b: &mut FunctionBuilder, ty: ir::Type, v: ir::Value) -> ir::Value {
        b.ins().fcvt_to_sint(ty, v)
    }
    pub fn translate_fcvt_to_uint(&mut self, b: &mut FunctionBuilder, ty: ir::Type, v: ir::Value) -> ir::Value {
        b.ins().fcvt_to_uint(ty, v)
    }
    pub fn ceil_f32(&mut self, b: &mut FunctionBuilder, v: ir::Value) -> ir::Value {
        b.ins().ceil(v)
    }
    pub fn ceil_f64(&mut self, b: &mut FunctionBuilder, v: ir::Value) -> ir::Value {
        b.ins().ceil(v)
    }
    pub fn ceil_f32x4(&mut self, b: &mut FunctionBuilder, v: ir::Value) -> ir::Value {
        b.ins().ceil(v)
    }
    pub fn ceil_f64x2(&mut self, b: &mut FunctionBuilder, v: ir::Value) -> ir::Value {
        b.ins().ceil(v)
    }
    pub fn floor_f32(&mut self, b: &mut FunctionBuilder, v: ir::Value) -> ir::Value {
        b.ins().floor(v)
    }
    pub fn floor_f64(&mut self, b: &mut FunctionBuilder, v: ir::Value) -> ir::Value {
        b.ins().floor(v)
    }
    pub fn floor_f32x4(&mut self, b: &mut FunctionBuilder, v: ir::Value) -> ir::Value {
        b.ins().floor(v)
    }
    pub fn floor_f64x2(&mut self, b: &mut FunctionBuilder, v: ir::Value) -> ir::Value {
        b.ins().floor(v)
    }
    pub fn trunc_f32(&mut self, b: &mut FunctionBuilder, v: ir::Value) -> ir::Value {
        b.ins().trunc(v)
    }
    pub fn trunc_f64(&mut self, b: &mut FunctionBuilder, v: ir::Value) -> ir::Value {
        b.ins().trunc(v)
    }
    pub fn trunc_f32x4(&mut self, b: &mut FunctionBuilder, v: ir::Value) -> ir::Value {
        b.ins().trunc(v)
    }
    pub fn trunc_f64x2(&mut self, b: &mut FunctionBuilder, v: ir::Value) -> ir::Value {
        b.ins().trunc(v)
    }
    pub fn nearest_f32(&mut self, b: &mut FunctionBuilder, v: ir::Value) -> ir::Value {
        b.ins().nearest(v)
    }
    pub fn nearest_f64(&mut self, b: &mut FunctionBuilder, v: ir::Value) -> ir::Value {
        b.ins().nearest(v)
    }
    pub fn nearest_f32x4(&mut self, b: &mut FunctionBuilder, v: ir::Value) -> ir::Value {
        b.ins().nearest(v)
    }
    pub fn nearest_f64x2(&mut self, b: &mut FunctionBuilder, v: ir::Value) -> ir::Value {
        b.ins().nearest(v)
    }
    pub fn fma_f32x4(&mut self, b: &mut FunctionBuilder, x: ir::Value, y: ir::Value, z: ir::Value) -> ir::Value {
        b.ins().fma(x, y, z)
    }
    pub fn fma_f64x2(&mut self, b: &mut FunctionBuilder, x: ir::Value, y: ir::Value, z: ir::Value) -> ir::Value {
        b.ins().fma(x, y, z)
    }
    pub fn swizzle(&mut self, b: &mut FunctionBuilder, x: ir::Value, y: ir::Value) -> ir::Value {
        b.ins().swizzle(x, y)
    }
    pub fn relaxed_swizzle(&mut self, b: &mut FunctionBuilder, x: ir::Value, y: ir::Value) -> ir::Value {
        b.ins().swizzle(x, y)
    }
    pub fn i8x16_shuffle(&mut self, b: &mut FunctionBuilder, x: ir::Value, y: ir::Value, lanes: &[u8; 16]) -> ir::Value {
        let lanes = ir::ConstantData::from(&lanes[..]);
        let mask = b.func.dfg.immediates.push(lanes);
        b.ins().shuffle(x, y, mask)
    }

    // ---- symbols ----

    fn data_symbol(&mut self, func: &mut ir::Function, index: u32) -> ir::GlobalValue {
        if let Some(&(_, gv)) = self.symbols.iter().find(|(k, _)| *k == index) {
            return gv;
        }
        let name = func.declare_imported_user_function(ir::UserExternalName::new(NS_DATA, index));
        let gv = func.create_global_value(ir::GlobalValueData::Symbol {
            name: ir::ExternalName::user(name),
            offset: Imm64::new(0),
            colocated: true,
            tls: false,
        });
        self.symbols.push((index, gv));
        gv
    }
    fn data_address(&mut self, builder: &mut FunctionBuilder, index: u32) -> ir::Value {
        let gv = self.data_symbol(builder.func, index);
        builder.ins().symbol_value(I64, gv)
    }
    fn region(func: &mut ir::Function, user_id: u32, description: String) -> ir::AliasRegion {
        func.dfg.alias_regions.insert(ir::AliasRegionData {
            user_id,
            description: Cow::Owned(description),
        })
    }
    fn flags(func: &mut ir::Function, user_id: u32, description: String) -> MemFlagsData {
        let mut f = MemFlagsData::trusted();
        f.set_alias_region(Some(Self::region(func, user_id, description)));
        f
    }

    // ---- memories ----

    pub fn memory_alias_region(&mut self, func: &mut ir::Function, memory: MemoryIndex) -> ir::AliasRegion {
        Self::region(func, REGION_MEMORY + memory.as_u32(), format!("memory {}", memory.as_u32()))
    }
    /// The memory's index type: i64 for a 64-bit memory, else i32.
    pub fn memory_index_type(&self, memory: MemoryIndex) -> ir::Type {
        if self.module.memories[memory.as_u32() as usize].memory64 { I64 } else { I32 }
    }
    /// The handle slot of a shared memory (imported from "ferros"), None for the module's own.
    pub fn memory_handle_slot(&self, memory: MemoryIndex) -> Option<u8> {
        self.module.memories[memory.as_u32() as usize].handle.map(|h| h.slot)
    }

    /// What toka refuses on a memory before translating an operator (`before_translate_operator`): a store, fill, copy or init into a view (read-only), and on any shared memory (a handle) what has no handle form yet: atomics (they take a bare base register, docs/TOKA.md "Memories"), the bulk operations (toka's runtime works on addresses) and growing (the region's size is its server's).
    fn check_memory_use(&self, op: &wasmparser::Operator) -> WasmResult<()> {
        use wasmparser::Operator as O;
        let mem = |idx: u32| &self.module.memories[idx as usize];
        let (store, atomic, idx) = match op {
            O::I32Store { memarg } | O::I64Store { memarg } | O::F32Store { memarg } | O::F64Store { memarg } | O::I32Store8 { memarg } | O::I32Store16 { memarg } | O::I64Store8 { memarg } | O::I64Store16 { memarg } | O::I64Store32 { memarg } | O::V128Store { memarg } => (true, false, memory_of(memarg)),
            O::V128Store8Lane { memarg, .. } | O::V128Store16Lane { memarg, .. } | O::V128Store32Lane { memarg, .. } | O::V128Store64Lane { memarg, .. } => (true, false, memory_of(memarg)),
            O::I32AtomicLoad { memarg } | O::I64AtomicLoad { memarg } | O::I32AtomicLoad8U { memarg } | O::I32AtomicLoad16U { memarg } | O::I64AtomicLoad8U { memarg } | O::I64AtomicLoad16U { memarg } | O::I64AtomicLoad32U { memarg } | O::MemoryAtomicNotify { memarg } | O::MemoryAtomicWait32 { memarg } | O::MemoryAtomicWait64 { memarg } => (false, true, memory_of(memarg)),
            O::I32AtomicStore { memarg } | O::I64AtomicStore { memarg } | O::I32AtomicStore8 { memarg } | O::I32AtomicStore16 { memarg } | O::I64AtomicStore8 { memarg } | O::I64AtomicStore16 { memarg } | O::I64AtomicStore32 { memarg } => (true, true, memory_of(memarg)),
            O::I32AtomicRmwAdd { memarg } | O::I64AtomicRmwAdd { memarg } | O::I32AtomicRmw8AddU { memarg } | O::I32AtomicRmw16AddU { memarg } | O::I64AtomicRmw8AddU { memarg } | O::I64AtomicRmw16AddU { memarg } | O::I64AtomicRmw32AddU { memarg }
            | O::I32AtomicRmwSub { memarg } | O::I64AtomicRmwSub { memarg } | O::I32AtomicRmw8SubU { memarg } | O::I32AtomicRmw16SubU { memarg } | O::I64AtomicRmw8SubU { memarg } | O::I64AtomicRmw16SubU { memarg } | O::I64AtomicRmw32SubU { memarg }
            | O::I32AtomicRmwAnd { memarg } | O::I64AtomicRmwAnd { memarg } | O::I32AtomicRmw8AndU { memarg } | O::I32AtomicRmw16AndU { memarg } | O::I64AtomicRmw8AndU { memarg } | O::I64AtomicRmw16AndU { memarg } | O::I64AtomicRmw32AndU { memarg }
            | O::I32AtomicRmwOr { memarg } | O::I64AtomicRmwOr { memarg } | O::I32AtomicRmw8OrU { memarg } | O::I32AtomicRmw16OrU { memarg } | O::I64AtomicRmw8OrU { memarg } | O::I64AtomicRmw16OrU { memarg } | O::I64AtomicRmw32OrU { memarg }
            | O::I32AtomicRmwXor { memarg } | O::I64AtomicRmwXor { memarg } | O::I32AtomicRmw8XorU { memarg } | O::I32AtomicRmw16XorU { memarg } | O::I64AtomicRmw8XorU { memarg } | O::I64AtomicRmw16XorU { memarg } | O::I64AtomicRmw32XorU { memarg }
            | O::I32AtomicRmwXchg { memarg } | O::I64AtomicRmwXchg { memarg } | O::I32AtomicRmw8XchgU { memarg } | O::I32AtomicRmw16XchgU { memarg } | O::I64AtomicRmw8XchgU { memarg } | O::I64AtomicRmw16XchgU { memarg } | O::I64AtomicRmw32XchgU { memarg }
            | O::I32AtomicRmwCmpxchg { memarg } | O::I64AtomicRmwCmpxchg { memarg } | O::I32AtomicRmw8CmpxchgU { memarg } | O::I32AtomicRmw16CmpxchgU { memarg } | O::I64AtomicRmw8CmpxchgU { memarg } | O::I64AtomicRmw16CmpxchgU { memarg } | O::I64AtomicRmw32CmpxchgU { memarg } => (true, true, memory_of(memarg)),
            O::MemoryFill { mem: m } | O::MemoryInit { mem: m, .. } | O::MemoryGrow { mem: m } => {
                return match mem(*m).handle {
                    Some(_) => Err(wasm_unsupported!("{op:?} on shared memory {m} (bulk operations and growth on a handle come later; a shared region's size is its server's)")),
                    None => Ok(()),
                };
            }
            O::MemoryCopy { dst_mem, src_mem } => {
                return if mem(*dst_mem).handle.is_some() || mem(*src_mem).handle.is_some() {
                    Err(wasm_unsupported!("memory.copy with shared memory {dst_mem} or {src_mem} (bulk operations on a handle come later)"))
                } else {
                    Ok(())
                };
            }
            _ => return Ok(()),
        };
        let Some(idx) = idx else { return Ok(()) };
        let Some(h) = mem(idx).handle else { return Ok(()) };
        if store && h.read_only {
            return Err(wasm_unsupported!("{op:?}: a store into memory {idx}, a view (read-only: imported as \"ferros\" \"view\")"));
        }
        if atomic {
            return Err(wasm_unsupported!("{op:?} on shared memory {idx}: an atomic takes a bare base register, which a handle does not offer yet (docs/TOKA.md \"Memories\")"));
        }
        Ok(())
    }

    /// The absolute address of the memory's first byte.
    pub fn memory_base(&mut self, builder: &mut FunctionBuilder, memory: MemoryIndex) -> ir::Value {
        self.data_address(builder, DATA_MEMORY | memory.as_u32())
    }
    /// `base + index`, the index zero-extended from a 32-bit memory; for a shared memory the index alone, an offset into the region its handle selects (the access's flags name the slot, and the back end masks the offset by the region's mask).
    pub fn memory_address(&mut self, builder: &mut FunctionBuilder, memory: MemoryIndex, index: ir::Value) -> ir::Value {
        let index = if self.memory_index_type(memory) == I32 { builder.ins().uextend(I64, index) } else { index };
        if self.memory_handle_slot(memory).is_some() {
            return index;
        }
        let base = self.memory_base(builder, memory);
        builder.ins().iadd(base, index)
    }
    fn pages_cell(&mut self, builder: &mut FunctionBuilder, memory: MemoryIndex) -> (ir::Value, MemFlagsData) {
        let cell = self.data_address(builder, DATA_MEMORY_PAGES | memory.as_u32());
        let flags = Self::flags(builder.func, REGION_GLOBALS, "globals".into());
        (cell, flags)
    }
    pub fn translate_memory_size(&mut self, mut pos: FuncCursor<'_>, memory: MemoryIndex) -> WasmResult<ir::Value> {
        // A shared memory's size is its region's: its pair's mask (in the grants page at arc offset 16 * slot + 8) plus one, in pages.
        if let Some(slot) = self.memory_handle_slot(memory) {
            let m = &self.module.memories[memory.as_u32() as usize];
            let arc = pos.ins().get_pinned_reg(I64);
            let mask = pos.ins().load(I64, MemFlagsData::trusted(), arc, Offset32::new(16 * i32::from(slot) + 8));
            let bytes = pos.ins().iadd_imm_s(mask, 1);
            let pages = pos.ins().ushr_imm_u(bytes, i64::from(m.page_log2));
            return Ok(if self.memory_index_type(memory) == I32 { pos.ins().ireduce(I32, pages) } else { pages });
        }
        let gv = self.data_symbol(pos.func, DATA_MEMORY_PAGES | memory.as_u32());
        let cell = pos.ins().symbol_value(I64, gv);
        let flags = Self::flags(pos.func, REGION_GLOBALS, "globals".into());
        let ty = self.memory_index_type(memory);
        Ok(pos.ins().load(ty, flags, cell, Offset32::new(0)))
    }
    /// memory.grow, branchless: the new size is kept when it is within the memory's reserve, else the old one, and the answer is the old size or -1.
    /// New pages are zero because the arc's memory is zeroed when it is allocated and the reserve is never handed back.
    pub fn translate_memory_grow(&mut self, builder: &mut FunctionBuilder, memory: MemoryIndex, delta: ir::Value) -> WasmResult<ir::Value> {
        let ty = self.memory_index_type(memory);
        let (cell, flags) = self.pages_cell(builder, memory);
        let old = builder.ins().load(ty, flags, cell, Offset32::new(0));
        let max = self.plan.max_pages[memory.as_u32() as usize];
        let (new, carry) = builder.ins().uadd_overflow(old, delta);
        let within = builder.ins().icmp_imm_u(IntCC::UnsignedLessThanOrEqual, new, max as i64);
        let no_carry = builder.ins().bxor_imm_u(carry, 1);
        let ok = builder.ins().band(within, no_carry);
        let kept = builder.ins().select(ok, new, old);
        builder.ins().store(flags, kept, cell, Offset32::new(0));
        let fail = builder.ins().iconst(ty, -1);
        Ok(builder.ins().select(ok, old, fail))
    }
    fn runtime_call(&mut self, builder: &mut FunctionBuilder, which: u32, args: &[ir::Value]) {
        let mut sig = ir::Signature::new(cranelift_codegen::isa::CallConv::Tail);
        for _ in args {
            sig.params.push(ir::AbiParam::new(I64));
        }
        let sig = builder.func.import_signature(sig);
        let name = builder.func.declare_imported_user_function(ir::UserExternalName::new(NS_RUNTIME, which));
        let f = builder.func.import_function(ir::ExtFuncData {
            name: ir::ExternalName::user(name),
            signature: sig,
            colocated: true,
            patchable: false,
        });
        builder.ins().call(f, args);
    }
    fn len64(&self, builder: &mut FunctionBuilder, memory: MemoryIndex, len: ir::Value) -> ir::Value {
        if self.memory_index_type(memory) == I32 { builder.ins().uextend(I64, len) } else { len }
    }
    pub fn translate_memory_copy(&mut self, builder: &mut FunctionBuilder, src_mem: MemoryIndex, dst_mem: MemoryIndex, dst: ir::Value, src: ir::Value, len: ir::Value) -> WasmResult<()> {
        let d = self.memory_address(builder, dst_mem, dst);
        let s = self.memory_address(builder, src_mem, src);
        // The length's type is the smaller memory's index type.
        let n = if builder.func.dfg.value_type(len) == I32 { builder.ins().uextend(I64, len) } else { len };
        self.runtime_call(builder, RUNTIME_MEMMOVE, &[d, s, n]);
        Ok(())
    }
    pub fn translate_memory_fill(&mut self, builder: &mut FunctionBuilder, memory: MemoryIndex, dst: ir::Value, val: ir::Value, len: ir::Value) -> WasmResult<()> {
        let d = self.memory_address(builder, memory, dst);
        let v = builder.ins().uextend(I64, val);
        let n = self.len64(builder, memory, len);
        self.runtime_call(builder, RUNTIME_MEMSET, &[d, v, n]);
        Ok(())
    }
    pub fn translate_memory_init(&mut self, builder: &mut FunctionBuilder, memory: MemoryIndex, seg: u32, dst: ir::Value, src: ir::Value, len: ir::Value) -> WasmResult<()> {
        let d = self.memory_address(builder, memory, dst);
        let base = self.data_address(builder, DATA_SEGMENT | seg);
        let src = builder.ins().uextend(I64, src);
        let s = builder.ins().iadd(base, src);
        let n = builder.ins().uextend(I64, len);
        self.runtime_call(builder, RUNTIME_MEMMOVE, &[d, s, n]);
        Ok(())
    }
    pub fn translate_data_drop(&mut self, _pos: FuncCursor, _seg: u32) -> WasmResult<()> {
        Ok(())
    }
    pub fn translate_atomic_wait(&mut self, _builder: &mut FunctionBuilder, _memory: MemoryIndex, _addr: ir::Value, _expected: ir::Value, _timeout: ir::Value) -> WasmResult<ir::Value> {
        Err(wasm_unsupported!("memory.atomic.wait (the kernel's wait on a word comes with threads, docs/TOKA.md)"))
    }
    pub fn translate_atomic_notify(&mut self, _builder: &mut FunctionBuilder, _memory: MemoryIndex, _addr: ir::Value, _count: ir::Value) -> WasmResult<ir::Value> {
        Err(wasm_unsupported!("memory.atomic.notify (the kernel's wake of a word comes with threads, docs/TOKA.md)"))
    }

    // ---- globals: an immutable one is its constant; a mutable one lives in the globals area ----

    pub fn translate_global_get(&mut self, builder: &mut FunctionBuilder, index: GlobalIndex) -> WasmResult<ir::Value> {
        let g = self.module.globals[index.as_u32() as usize].clone();
        if !g.mutable {
            let lo = u64::from_le_bytes(g.init[..8].try_into().unwrap());
            return Ok(match g.ty {
                WasmValType::I32 => builder.ins().iconst(I32, lo as u32 as i64),
                WasmValType::I64 => builder.ins().iconst(I64, lo as i64),
                WasmValType::F32 => builder.ins().f32const(ir::immediates::Ieee32::with_bits(lo as u32)),
                WasmValType::F64 => builder.ins().f64const(ir::immediates::Ieee64::with_bits(lo)),
                WasmValType::V128 => {
                    let c = builder.func.dfg.constants.insert(ir::ConstantData::from(&g.init[..]));
                    builder.ins().vconst(I8X16, c)
                }
            });
        }
        let area = self.data_address(builder, DATA_GLOBALS);
        let flags = Self::flags(builder.func, REGION_GLOBALS, "globals".into());
        Ok(builder.ins().load(g.ty.clif(), flags, area, Offset32::new((GLOBAL_SLOT * index.as_u32()) as i32)))
    }
    pub fn translate_global_set(&mut self, builder: &mut FunctionBuilder, index: GlobalIndex, val: ir::Value) -> WasmResult<()> {
        let area = self.data_address(builder, DATA_GLOBALS);
        let flags = Self::flags(builder.func, REGION_GLOBALS, "globals".into());
        builder.ins().store(flags, val, area, Offset32::new((GLOBAL_SLOT * index.as_u32()) as i32));
        Ok(())
    }

    // ---- calls: every toka function is CallConv::Tail with its WebAssembly parameters only ----

    pub fn func_type_index(&self, f: FuncIndex) -> TypeIndex {
        self.module.functions[f.as_u32() as usize]
    }
    pub fn num_params_for_func(&self, f: FuncIndex) -> usize {
        self.module.func_type(f).params.len()
    }
    pub fn num_params_for_function_type(&self, t: TypeIndex) -> usize {
        self.module.types[t.as_u32() as usize].params.len()
    }
    pub fn get_or_create_sig_ref(&mut self, func: &mut ir::Function, t: TypeIndex) -> ir::SigRef {
        func.import_signature(signature(&self.module.types[t.as_u32() as usize]))
    }
    fn func_ref(&mut self, func: &mut ir::Function, callee: FuncIndex, sig: ir::SigRef) -> ir::FuncRef {
        let name = func.declare_imported_user_function(ir::UserExternalName::new(NS_FUNC, callee.as_u32()));
        func.import_function(ir::ExtFuncData {
            name: ir::ExternalName::user(name),
            signature: sig,
            colocated: true,
            patchable: false,
        })
    }
    pub fn translate_call(&mut self, builder: &mut FunctionBuilder, _srcloc: ir::SourceLoc, callee: FuncIndex, sig: ir::SigRef, args: &[ir::Value]) -> WasmResult<crate::Reachability<Vec<ir::Value>>> {
        if let Some(results) = self.intrinsic(builder, callee, args)? {
            return Ok(crate::Reachability::Reachable(results));
        }
        let f = self.func_ref(builder.func, callee, sig);
        let call = builder.ins().call(f, args);
        Ok(crate::Reachability::Reachable(builder.inst_results(call).to_vec()))
    }
    /// A call of a function imported from "ferros" (docs/TOKA.md "Intrinsics"), its results; None for any other callee.
    /// - `syscall(n, a0, a1, a2, a3, a4, a5: i64) -> i64`: the kernel's call `n` (process.rs's numbers, ferros_sys `nr`), thru the image's one native stub.
    /// - `arc_offset(p: i32 or i64) -> i64`: memory 0's address `p` as an offset into the process's arc, what every syscall takes for a buffer (the kernel masks it).
    fn intrinsic(&mut self, builder: &mut FunctionBuilder, callee: FuncIndex, args: &[ir::Value]) -> WasmResult<Option<Vec<ir::Value>>> {
        let k = callee.as_u32() as usize;
        if k >= self.module.num_imported_funcs() || self.module.imports[k].module != "ferros" {
            return Ok(None);
        }
        let name = self.module.imports[k].name.clone();
        let t = self.module.func_type(callee).clone();
        let want = |params: &[WasmValType], results: &[WasmValType]| -> WasmResult<()> {
            if t.params == params && t.results == results {
                Ok(())
            } else {
                Err(wasm_unsupported!("intrinsic ferros.{name} with type {t:?} (it is {params:?} -> {results:?})"))
            }
        };
        let w64 = WasmValType::I64;
        match name.as_str() {
            "syscall" => {
                want(&[w64; 7], &[w64])?;
                let mut sig = ir::Signature::new(cranelift_codegen::isa::CallConv::SystemV);
                sig.params.extend((0..7).map(|_| ir::AbiParam::new(I64)));
                sig.returns.push(ir::AbiParam::new(I64));
                let sig = builder.func.import_signature(sig);
                let fname = builder.func.declare_imported_user_function(ir::UserExternalName::new(NS_RUNTIME, RUNTIME_SYSCALL));
                let f = builder.func.import_function(ir::ExtFuncData {
                    name: ir::ExternalName::user(fname),
                    signature: sig,
                    colocated: true,
                    patchable: false,
                });
                let call = builder.ins().call(f, args);
                Ok(Some(builder.inst_results(call).to_vec()))
            }
            "arc_offset" => {
                let memory = MemoryIndex::from_u32(0);
                if self.module.memories.first().is_none_or(|m| m.handle.is_some()) {
                    return Err(wasm_unsupported!("ferros.arc_offset without a memory 0 of the module's own"));
                }
                let p = if self.memory_index_type(memory) == I32 {
                    want(&[WasmValType::I32], &[WasmValType::I64])?;
                    builder.ins().uextend(I64, args[0])
                } else {
                    want(&[WasmValType::I64], &[WasmValType::I64])?;
                    args[0]
                };
                let base = self.memory_base(builder, memory);
                let arc = builder.ins().get_pinned_reg(I64);
                let from_arc = builder.ins().isub(base, arc);
                Ok(Some(vec![builder.ins().iadd(from_arc, p)]))
            }
            other => Err(wasm_unsupported!("no intrinsic ferros.{other} (toka has syscall and arc_offset)")),
        }
    }

    pub fn translate_return_call(&mut self, builder: &mut FunctionBuilder, _srcloc: ir::SourceLoc, callee: FuncIndex, sig: ir::SigRef, args: &[ir::Value]) -> WasmResult<()> {
        if (callee.as_u32() as usize) < self.module.num_imported_funcs() && self.module.imports[callee.as_u32() as usize].module == "ferros" {
            return Err(wasm_unsupported!("a tail call of an intrinsic (call it, then return)"));
        }
        let f = self.func_ref(builder.func, callee, sig);
        builder.ins().return_call(f, args);
        Ok(())
    }
    /// The callee's address from the table: in bounds (else a trap), its type the expected one (a null slot's matches none), then the table's start plus the entry's offset.
    fn table_callee(&mut self, builder: &mut FunctionBuilder, table: TableIndex, ty: TypeIndex, callee: ir::Value) -> ir::Value {
        let t = &self.module.tables[table.as_u32() as usize];
        let len = t.slots.len() as i64;
        let idx = if builder.func.dfg.value_type(callee) == I32 { builder.ins().uextend(I64, callee) } else { callee };
        let oob = builder.ins().icmp_imm_u(IntCC::UnsignedGreaterThanOrEqual, idx, len);
        builder.ins().trapnz(oob, TRAP_TABLE_OUT_OF_BOUNDS);
        let base = self.data_address(builder, DATA_TABLE | table.as_u32());
        let scaled = builder.ins().ishl_imm_u(idx, i64::from(TABLE_ENTRY.trailing_zeros()));
        let entry = builder.ins().iadd(base, scaled);
        let flags = Self::flags(builder.func, REGION_TABLES, "tables".into());
        let got = builder.ins().load(I32, flags, entry, Offset32::new(0));
        let want = builder.ins().iconst(I32, i64::from(self.plan.canonical_type[ty.as_u32() as usize]));
        let bad = builder.ins().icmp(IntCC::NotEqual, got, want);
        builder.ins().trapnz(bad, TRAP_BAD_SIGNATURE);
        let rel = builder.ins().sload32(flags, entry, Offset32::new(4));
        builder.ins().iadd(base, rel)
    }
    pub fn translate_call_indirect(&mut self, builder: &mut FunctionBuilder, _srcloc: ir::SourceLoc, table: TableIndex, ty: TypeIndex, sig: ir::SigRef, callee: ir::Value, args: &[ir::Value]) -> WasmResult<Option<Vec<ir::Value>>> {
        let addr = self.table_callee(builder, table, ty, callee);
        let call = builder.ins().call_indirect(sig, addr, args);
        Ok(Some(builder.inst_results(call).to_vec()))
    }
    pub fn translate_return_call_indirect(&mut self, builder: &mut FunctionBuilder, _srcloc: ir::SourceLoc, table: TableIndex, ty: TypeIndex, sig: ir::SigRef, callee: ir::Value, args: &[ir::Value]) -> WasmResult<()> {
        let addr = self.table_callee(builder, table, ty, callee);
        builder.ins().return_call_indirect(sig, addr, args);
        Ok(())
    }
}

/// The memory an access names (None: an array target, refused by the translator).
fn memory_of(memarg: &wasmparser::MemArg) -> Option<u32> {
    match memarg.target {
        wasmparser::MemArgTarget::Memory(m) => Some(m),
        _ => None,
    }
}

/// A WebAssembly function type as a Cranelift signature (CallConv::Tail, so tail calls work between any two).
pub fn signature(t: &FuncType) -> ir::Signature {
    let mut sig = ir::Signature::new(cranelift_codegen::isa::CallConv::Tail);
    sig.params.extend(t.params.iter().map(|p| ir::AbiParam::new(p.clif())));
    sig.returns.extend(t.results.iter().map(|r| ir::AbiParam::new(r.clif())));
    sig
}
