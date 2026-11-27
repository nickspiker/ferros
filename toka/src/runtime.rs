//! toka's runtime: the few functions translated code calls (memory.copy, memory.fill, memory.init), built here as Cranelift IR and compiled and masked like any other function, so the image holds no code toka did not emit.
//! Byte loops for now; 16-byte NEON strides when they are measured worth it.

use crate::environ::{NS_RUNTIME, RUNTIME_MEMMOVE, RUNTIME_MEMSET};
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

/// Every runtime function, each named in `NS_RUNTIME`.
pub fn functions(config: TargetFrontendConfig) -> Vec<ir::Function> {
    vec![memmove(config), memset(config)]
}
