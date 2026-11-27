//! toka: ferros's compiler (docs/TOKA.md; the owner, 2026-11-26: "Crainlift it is! Let's build it!").
//! WebAssembly, forked and extended where ferros needs it, compiled by the Cranelift fork into aarch64 that ferros's sandbox checker accepts: every memory access masked into the arc as it is emitted.
//! It runs on the developer's machine today and, under trust model A, on the device at stow (the device compiles everything it runs).

pub mod environ;
pub mod link;
pub mod module;
pub mod runtime;
mod translate;
pub mod types;

use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::ir;
use cranelift_codegen::isa::{self, OwnedTargetIsa};
use cranelift_codegen::settings::{self, Configurable};

/// The wasmtime translator's control: a function's code reaches past here only when it is reachable.
pub enum Reachability<T> {
    Reachable(T),
    Unreachable,
}

/// A br_table's most targets (as wasmtime's translator holds it).
pub const MAX_BR_TABLE_TARGETS: u32 = 65520;

/// How addresses are made safe (docs/SANDBOX.md "Modes").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mask {
    Ring,
    Fixed(u32),
    Exact,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub mask: Mask,
    pub bti: bool,
    /// The address space each memory may grow into, as a power of two (its reserve; memory.grow past it answers -1).
    pub memory_reserve_log2: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            mask: Mask::Ring,
            bti: false,
            memory_reserve_log2: 26,
        }
    }
}

/// The ISA toka compiles for: aarch64 as the Pixel 8 Pro and 8a implement it (docs/CPU-FEATURES.md), with the ferros masking on and pointer authentication off (its keys hold shares of the ira and the wairua).
pub fn isa(opts: &Options) -> Result<OwnedTargetIsa, String> {
    let mut flags = settings::builder();
    let set = |b: &mut settings::Builder, k: &str, v: &str| b.set(k, v).map_err(|e| format!("{k}={v}: {e}"));
    set(&mut flags, "opt_level", "speed")?;
    set(&mut flags, "enable_probestack", "false")?;
    // The kernel walks a death's call chain by its frame records (KERNEL.md "A process that dies").
    set(&mut flags, "preserve_frame_pointers", "true")?;
    set(&mut flags, "unwind_info", "false")?;
    set(&mut flags, "enable_verifier", "true")?;
    let (mask, k) = match opts.mask {
        Mask::Ring => ("ring", 24),
        Mask::Fixed(k) => ("fixed", k),
        Mask::Exact => ("exact", 24),
    };
    set(&mut flags, "ferros_mask", mask)?;
    set(&mut flags, "ferros_fixed_log2", &k.to_string())?;
    let triple: target_lexicon::Triple = "aarch64-unknown-unknown-elf".parse().map_err(|e| format!("{e:?}"))?;
    let mut b = isa::lookup(triple).map_err(|e| e.to_string())?;
    for f in ["has_lse", "has_fp16", "has_dotprod", "has_i8mm"] {
        b.enable(f).map_err(|e| format!("{f}: {e}"))?;
    }
    if opts.bti {
        b.enable("use_bti").map_err(|e| e.to_string())?;
    }
    b.finish(settings::Flags::new(flags)).map_err(|e| e.to_string())
}

/// One function compiled: its code and where it refers outside itself.
pub struct Compiled {
    pub name: ir::UserExternalName,
    pub code: Vec<u8>,
    pub relocs: Vec<link::Reloc>,
}

/// Compile one Cranelift function and gather its relocations by name.
pub fn compile_function(isa: &dyn isa::TargetIsa, func: ir::Function, name: ir::UserExternalName) -> Result<Compiled, String> {
    let mut ctx = cranelift_codegen::Context::for_function(func);
    let code = ctx.compile(isa, &mut ControlPlane::default()).map_err(|e| format!("{:?}", e.inner))?;
    let bytes = code.code_buffer().to_vec();
    let raw: Vec<_> = code.buffer.relocs().to_vec();
    let mut relocs = vec![];
    for r in raw {
        let cranelift_codegen::RelocTarget::ExternalName(ir::ExternalName::User(user)) = r.target else {
            return Err(format!("a relocation to {:?}", r.target));
        };
        let target = ctx.func.params.user_named_funcs()[user].clone();
        relocs.push(link::Reloc {
            offset: r.offset,
            kind: r.kind,
            target,
            addend: r.addend,
        });
    }
    Ok(Compiled { name, code: bytes, relocs })
}

/// Compile a module: every function and toka's runtime, laid out and linked into one image the checker has passed.
pub fn compile(bytes: &[u8], opts: &Options) -> Result<link::Image, String> {
    let module = module::parse(bytes).map_err(|e| e.to_string())?;
    let isa = isa(opts)?;
    let plan = environ::Plan::new(&module, opts.memory_reserve_log2);
    let mut compiled = vec![];
    let mut translator = translate::FuncTranslator::new();
    let mut module = module;
    let bodies = std::mem::take(&mut module.bodies);
    for body in bodies {
        let mut validator = body.validator;
        let name = ir::UserExternalName::new(environ::NS_FUNC, body.index.as_u32());
        let mut func = ir::Function::with_name_signature(ir::UserFuncName::User(name.clone()), environ::signature(module.func_type(body.index)));
        let mut env = environ::FuncEnvironment::new(&module, &plan, &*isa, body.index);
        translator
            .translate_body(&mut validator, body.body, &mut func, &mut env)
            .map_err(|e| format!("function {}: {e}", body.index.as_u32()))?;
        compiled.push(compile_function(&*isa, func, name).map_err(|e| format!("function {}: {e}", body.index.as_u32()))?);
    }
    for f in runtime::functions(isa.frontend_config()) {
        let name = match &f.name {
            ir::UserFuncName::User(n) => n.clone(),
            _ => unreachable!(),
        };
        compiled.push(compile_function(&*isa, f, name)?);
    }
    link::link(&module, &plan, compiled, opts)
}

pub use types::WasmError as Error;
