//! Cisco's OpenH264 for ferros (docs/STD-TARGET.md "C++ through ferros_rt"): the encoder, the decoder and the two libraries under them (common, processing), C++ and their arm64 assembly, built by the cross g++ with no C++ standard library, no exceptions and no RTTI, against this crate's C header set (c/include), whose calls ferros_rt's `cxx` feature answers.
//!
//! The source is a pinned checkout, never fetched here: `OPENH264_SRC`, or /mnt/Harbor/ferros-ref/openh264, made by `git clone --depth 1 --branch v2.7.0 https://github.com/cisco/openh264` (commit 1bca62311e447fd9c39fc93de2c4b96cd5ce971c). Its codec_ver.h must say 2.7.0, the version the bindings in src/ were checked against (2.6's API headers, unchanged in 2.7 but for the version).
//!
//! The flags, each for a reason:
//! - `-ffixed-x24` to `-ffixed-x28`: the registers the target reserves (x28 the arc base, x27 its mask, which ferros_rt reads); the assembly never names them (checked by objdump, STD-TARGET.md).
//! - `-mbranch-protection=none`: Fedora's cross gcc defaults to pac-ret, whose PACIASP/AUTIASP would sign with keys that hold shares of the ira and wairua (tools/std-build.sh); it also keeps the assembly's BTI landing pads out.
//! - `-ffreestanding -fbuiltin`: no host headers or libraries, but the compiler still inlines a fixed-size memcpy or memset, which the codec's C does by the hundred.
//! - `-fno-exceptions -fno-rtti -nostdinc++ -fno-asynchronous-unwind-tables`: nothing throws, nothing asks a type at run time (the one dynamic_cast is an upcast), no C++ library headers, no unwind tables (the image keeps none).
//! - `-fno-math-errno`: sqrt and sqrtf become the instruction.
//! - `-mno-outline-atomics`: atomics inline, not libgcc's helpers.
//! - `-DNDEBUG -O3 -fno-strict-aliasing -fno-common`: upstream's release flags.

use std::path::{Path, PathBuf};

/// The checkout's version, as its codec_ver.h spells it.
const VERSION: &str = "OpenH264 version:2.7.0";

fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if os != "ferros" {
        panic!("this openh264-sys2 is ferros's fork and builds for aarch64-unknown-ferros only; elsewhere take crates.io's");
    }
    println!("cargo:rerun-if-env-changed=OPENH264_SRC");
    println!("cargo:rerun-if-changed=c/include");
    let src = PathBuf::from(std::env::var("OPENH264_SRC").unwrap_or_else(|_| "/mnt/Harbor/ferros-ref/openh264".into()));
    let ver = std::fs::read_to_string(src.join("codec/api/wels/codec_ver.h"))
        .unwrap_or_else(|e| panic!("no OpenH264 checkout at {} ({e}): git clone --depth 1 --branch v2.7.0 https://github.com/cisco/openh264 {}", src.display(), src.display()));
    if !ver.contains(VERSION) {
        panic!("{} is not OpenH264 2.7.0 (the pinned version, build.rs)", src.display());
    }
    let codec = src.join("codec");
    let include = Path::new(env!("CARGO_MANIFEST_DIR")).join("c/include");
    lib(&codec, &include, "common", "common", "arm64", &[]);
    lib(&codec, &include, "processing", "processing", "src/arm64", &["processing/src/common", "processing/interface"]);
    lib(&codec, &include, "decoder", "decoder", "core/arm64", &["decoder/core/inc", "decoder/plus/inc"]);
    lib(&codec, &include, "encoder", "encoder", "core/arm64", &["encoder/core/inc", "encoder/plus/inc", "processing/interface"]);
}

/// Every file under `dir` ending in `ext`, but `skip`, in a fixed order.
fn files(dir: &Path, ext: &str, skip: &str, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            files(&p, ext, skip, out);
        } else if p.extension().is_some_and(|e| e == ext) && p.file_name().is_some_and(|n| n != skip) {
            out.push(p);
        }
    }
}

/// One of OpenH264's four libraries: its C++ under `root` (all but DllEntry.cpp, Windows only), its assembly under `root/asm`, its own include directories.
fn lib(codec: &Path, include: &Path, name: &str, root: &str, asm: &str, incs: &[&str]) {
    let mut cpp = Vec::new();
    files(&codec.join(root), "cpp", "DllEntry.cpp", &mut cpp);
    let mut s = Vec::new();
    files(&codec.join(root).join(asm), "S", "arm_arch64_common_macro.S", &mut s);
    let mut b = cc::Build::new();
    b.cpp(true)
        .compiler(std::env::var("CXX_aarch64_unknown_ferros").unwrap_or_else(|_| "aarch64-linux-gnu-g++".into()))
        .archiver(std::env::var("AR_aarch64_unknown_ferros").unwrap_or_else(|_| "aarch64-linux-gnu-ar".into()))
        .cpp_link_stdlib(None)
        .warnings(false)
        .pic(true)
        .opt_level(3)
        .define("HAVE_NEON_AARCH64", None)
        .define("NDEBUG", None)
        .include(codec.join("api/wels"))
        .include(codec.join("common/inc"))
        .include(codec.join("common/arm64"));
    for i in incs {
        b.include(codec.join(i));
    }
    for f in [
        "-ffreestanding",
        "-fbuiltin",
        "-fno-stack-protector",
        "-mbranch-protection=none",
        "-ffixed-x24",
        "-ffixed-x25",
        "-ffixed-x26",
        "-ffixed-x27",
        "-ffixed-x28",
        "-march=armv8.2-a",
        "-mno-outline-atomics",
        "-fno-exceptions",
        "-fno-rtti",
        "-nostdinc++",
        "-fno-asynchronous-unwind-tables",
        "-fno-unwind-tables",
        "-fno-math-errno",
        "-fno-strict-aliasing",
        "-fno-common",
    ] {
        b.flag(f);
    }
    b.flag("-isystem").flag(include.to_str().unwrap());
    b.files(&cpp).files(&s);
    b.compile(&format!("openh264_{name}"));
}
