//! toka's runtime: the few functions translated code calls (memory.copy, memory.fill, memory.init), built here as Cranelift IR and compiled and masked like any other function, so the image holds no code toka did not emit.
//! Byte loops for now; 16-byte NEON strides when they are measured worth it.

use crate::environ::{NS_FUNC, NS_RUNTIME, RUNTIME_ENTRY, RUNTIME_MEMMOVE, RUNTIME_MEMSET};
use crate::module::Module;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::immediates::Offset32;
use cranelift_codegen::ir::types::*;
use cranelift_codegen::ir::{self, AbiParam, InstBuilder, MemFlagsData};
use cranelift_codegen::isa::{CallConv, TargetFrontendConfig};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};

fn signature(params: usize) -> ir::Signature {
    let mut sig = ir::Signature::new(CallConv::Tail);
    sig.params.extend((0..params).map(|_| AbiParam::new(I64)));
    sig
}

/// `memmove(dst, src, len)`: forward when dst is below src, else backward, so overlapping ranges copy as WebAssembly's memory.copy says.
fn memmove(config: TargetFrontendConfig) -> ir::Function {
    let name = ir::UserExternalName::new(NS_RUNTIME, RUNTIME_MEMMOVE);
    let mut func = ir::Function::with_name_signature(ir::UserFuncName::User(name), signature(3));
    let mut ctx = FunctionBuilderContext::new();
    let mut b = FunctionBuilder::new(&mut func, &mut ctx);
    let flags = MemFlagsData::trusted();
    let entry = b.create_block();
    let fwd = b.create_block();
    let fwd_body = b.create_block();
    let back = b.create_block();
    let back_body = b.create_block();
    let done = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.append_block_param(fwd, I64);
    b.append_block_param(back, I64);
    b.switch_to_block(entry);
    let (d, s, n) = {
        let p = b.block_params(entry);
        (p[0], p[1], p[2])
    };
    let zero = b.ins().iconst(I64, 0);
    let below = b.ins().icmp(IntCC::UnsignedLessThanOrEqual, d, s);
    b.ins().brif(below, fwd, &[zero.into()], back, &[n.into()]);

    // forward: i from 0 up to n
    b.switch_to_block(fwd);
    let i = b.block_params(fwd)[0];
    let more = b.ins().icmp(IntCC::UnsignedLessThan, i, n);
    b.ins().brif(more, fwd_body, &[], done, &[]);
    b.switch_to_block(fwd_body);
    let sa = b.ins().iadd(s, i);
    let da = b.ins().iadd(d, i);
    let byte = b.ins().uload8(I32, flags, sa, Offset32::new(0));
    b.ins().istore8(flags, byte, da, Offset32::new(0));
    let next = b.ins().iadd_imm_s(i, 1);
    b.ins().jump(fwd, &[next.into()]);

    // backward: i from n down to 0
    b.switch_to_block(back);
    let i = b.block_params(back)[0];
    let more = b.ins().icmp_imm_s(IntCC::NotEqual, i, 0);
    b.ins().brif(more, back_body, &[], done, &[]);
    b.switch_to_block(back_body);
    let j = b.ins().iadd_imm_s(i, -1);
    let sa = b.ins().iadd(s, j);
    let da = b.ins().iadd(d, j);
    let byte = b.ins().uload8(I32, flags, sa, Offset32::new(0));
    b.ins().istore8(flags, byte, da, Offset32::new(0));
    b.ins().jump(back, &[j.into()]);

    b.switch_to_block(done);
    b.ins().return_(&[]);
    b.seal_all_blocks();
    b.finalize(config);
    func
}

/// `memset(dst, value, len)`: the value's low byte in every byte of the range.
fn memset(config: TargetFrontendConfig) -> ir::Function {
    let name = ir::UserExternalName::new(NS_RUNTIME, RUNTIME_MEMSET);
    let mut func = ir::Function::with_name_signature(ir::UserFuncName::User(name), signature(3));
    let mut ctx = FunctionBuilderContext::new();
    let mut b = FunctionBuilder::new(&mut func, &mut ctx);
    let flags = MemFlagsData::trusted();
    let entry = b.create_block();
    let head = b.create_block();
    let body = b.create_block();
    let done = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.append_block_param(head, I64);
    b.switch_to_block(entry);
    let (d, v, n) = {
        let p = b.block_params(entry);
        (p[0], p[1], p[2])
    };
    let zero = b.ins().iconst(I64, 0);
    b.ins().jump(head, &[zero.into()]);
    b.switch_to_block(head);
    let i = b.block_params(head)[0];
    let more = b.ins().icmp(IntCC::UnsignedLessThan, i, n);
    b.ins().brif(more, body, &[], done, &[]);
    b.switch_to_block(body);
    let da = b.ins().iadd(d, i);
    b.ins().istore8(flags, v, da, Offset32::new(0));
    let next = b.ins().iadd_imm_s(i, 1);
    b.ins().jump(head, &[next.into()]);
    b.switch_to_block(done);
    b.ins().return_(&[]);
    b.seal_all_blocks();
    b.finalize(config);
    func
}

/// The one native stub, `syscall(n, a0..a5)` called as AAPCS64 (n in x0, the arguments in x1 to x6): moved to the kernel's registers (the number in x8, the arguments in x0 to x5), `svc #0`, the answer in x0; under BTI a `bti c` before it (it is reached by `bl`) and `br x30` for its return. Written as words because Cranelift has no `svc`; every one of them is a form the checker accepts (moves into x0 to x8, `svc #0`).
pub fn syscall_stub(bti: bool) -> Vec<u8> {
    let mut w: Vec<u32> = vec![];
    if bti {
        w.push(0xD503_245F); // bti c
    }
    w.extend([
        0xAA00_03E8, // mov x8, x0
        0xAA01_03E0, // mov x0, x1
        0xAA02_03E1, // mov x1, x2
        0xAA03_03E2, // mov x2, x3
        0xAA04_03E3, // mov x3, x4
        0xAA05_03E4, // mov x4, x5
        0xAA06_03E5, // mov x5, x6
        0xD400_0001, // svc #0
    ]);
    w.push(if bti { 0xD61F_03C0 } else { 0xD65F_03C0 }); // br x30 / ret
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// The timer stubs, `now()` and `freq()` as AAPCS64 (`isb; mrs x0, cntvct_el0` and `mrs x0, cntfrq_el0`, the two system registers the checker lets EL0 read), `bti c` before each under BTI and `br x30` for the return.
pub fn timer_stub(count: bool, bti: bool) -> Vec<u8> {
    let mut w: Vec<u32> = vec![];
    if bti {
        w.push(0xD503_245F); // bti c
    }
    if count {
        w.push(0xD503_3FDF); // isb
        w.push(0xD53B_E040); // mrs x0, cntvct_el0
    } else {
        w.push(0xD53B_E000); // mrs x0, cntfrq_el0
    }
    w.push(if bti { 0xD61F_03C0 } else { 0xD65F_03C0 });
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// The image's entry, reached from the header's branch with the kernel's x0 to x2 (the grants offset, the input's offset and length): the module's start function, then its `_ferros_main(grants, input, length: i64)` if it exports one, else its `_start()`, then exit (syscall 0) with 0.
pub fn entry(module: &Module<'_>, config: TargetFrontendConfig) -> Result<ir::Function, String> {
    let name = ir::UserExternalName::new(NS_RUNTIME, RUNTIME_ENTRY);
    let mut sig = ir::Signature::new(CallConv::SystemV);
    sig.params.extend((0..3).map(|_| AbiParam::new(I64)));
    let mut func = ir::Function::with_name_signature(ir::UserFuncName::User(name), sig);
    let mut ctx = FunctionBuilderContext::new();
    let mut b = FunctionBuilder::new(&mut func, &mut ctx);
    let entry = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);
    let p: Vec<ir::Value> = b.block_params(entry).to_vec();
    let mut call_wasm = |b: &mut FunctionBuilder, f: u32, args: &[ir::Value]| {
        let t = module.func_type(crate::types::FuncIndex::from_u32(f));
        let sig = b.func.import_signature(crate::environ::signature(t));
        let fname = b.func.declare_imported_user_function(ir::UserExternalName::new(NS_FUNC, f));
        let fr = b.func.import_function(ir::ExtFuncData { name: ir::ExternalName::user(fname), signature: sig, colocated: true, patchable: false });
        b.ins().call(fr, args);
    };
    if let Some(s) = module.start {
        call_wasm(&mut b, s.as_u32(), &[]);
    }
    let find = |n: &str| module.exports.iter().find(|(e, _)| e == n).map(|(_, f)| *f);
    use crate::types::WasmValType::I64 as W64;
    if let Some(m) = find("_ferros_main") {
        let t = module.func_type(m);
        if t.params != [W64, W64, W64] || !t.results.is_empty() {
            return Err(format!("_ferros_main has type {t:?}, not (i64 grants, i64 input, i64 length) -> ()"));
        }
        call_wasm(&mut b, m.as_u32(), &p);
    } else if let Some(m) = find("_start") {
        let t = module.func_type(m);
        if !t.params.is_empty() || !t.results.is_empty() {
            return Err(format!("_start has type {t:?}, not () -> ()"));
        }
        call_wasm(&mut b, m.as_u32(), &[]);
    } else {
        return Err("the module exports neither _ferros_main nor _start: an image needs one to run".into());
    }
    // exit(0): syscall 0 thru the stub, which never returns; a trap after it, should it.
    let mut ssig = ir::Signature::new(CallConv::SystemV);
    ssig.params.extend((0..7).map(|_| AbiParam::new(I64)));
    ssig.returns.push(AbiParam::new(I64));
    ssig.returns.push(AbiParam::new(I64));
    let ssig = b.func.import_signature(ssig);
    let sname = b.func.declare_imported_user_function(ir::UserExternalName::new(NS_RUNTIME, crate::environ::RUNTIME_SYSCALL));
    let sf = b.func.import_function(ir::ExtFuncData { name: ir::ExternalName::user(sname), signature: ssig, colocated: true, patchable: false });
    let zero = b.ins().iconst(I64, 0);
    b.ins().call(sf, &[zero; 7]);
    b.ins().trap(crate::environ::TRAP_UNREACHABLE);
    b.seal_all_blocks();
    b.finalize(config);
    Ok(func)
}

/// Every runtime function, each named in `NS_RUNTIME`.
pub fn functions(config: TargetFrontendConfig) -> Vec<ir::Function> {
    vec![memmove(config), memset(config)]
}
