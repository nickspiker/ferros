#[path = "src/gpu_layout.rs"]
#[allow(dead_code)]
mod gpu_layout;
#[path = "src/gpu_kernel.rs"]
mod gpu_kernel;
#[path = "src/gpu_layers.rs"]
mod gpu_layers;
/// The background noise's parallel form (the Photon session's generator, as compbench copies it; fluor's own constants stay private until the Photon session makes them public, GPU-COMPOSE.md 3.4).
#[path = "../compbench/src/noise.rs"]
#[allow(dead_code)]
mod noise;

fn main() {
    compose_kernel();
    let ld = std::path::Path::new("payloads/display/payload.ld");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    // Position-independent link: absolute addresses in data become .rela.dyn entries that ferros_sys::relocate() applies at entry (the vtables of `dyn` traits, for one — fontdue's outline builder faulted at an unrelocated one on 2026-10-16).
    println!("cargo:rustc-link-arg=-pie");
    println!("cargo:rustc-link-arg=--no-dynamic-linker");
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=norelro");
    // The absolute words live in .rodata; lld only leaves relocations in a read-only section with `-z notext`.
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=notext");
    println!("cargo:rerun-if-changed={}", ld.display());
    // No kist's icon is built in (owner, 2026-11-20): the map hands each over by its `hp` and the display keeps them in its cache (main.rs `ICON_SLOT`). Until then the bundle (tools/app-icons `bundle`, target/icons/icons.bin) went in here, numbered, and two builds with different numbering swapped the settings and Kūaha icons on the Pro.
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    // The ferros mark (logo.vsf, tools/logo-vsf's analytic pieces) as a table the display draws at any size (draw::logo), in place of the bundle's raster (owner, 2026-11-07). Each piece: its inner and outer radius as fractions of the mark's outer radius, where it starts and how far it sweeps (turns, from +x toward +y, the sweep made positive), and its colour as working values (gamma 2, white at Q14), lit and at rest (the grey: photopic luminance, VSF RGB's weights R 0.2584, G 0.6346, B 0.1070, as tools/app-icons makes the map's greys).
    let mark = manifest.join("../../assets/logo.vsf");
    println!("cargo:rerun-if-changed={}", mark.display());
    let bytes = std::fs::read(&mark).expect("logo.vsf");
    let vsf::VsfType::ron(node) = vsf::types::renderable::read_scene(&bytes).expect("logo.vsf: its scene") else { panic!("logo.vsf: the scene is not a node") };
    let mut pieces = Vec::new();
    let (mut centre, mut rmax) = (None, 0f64);
    for child in &node.children {
        let vsf::VsfType::roe(e) = child else { panic!("logo.vsf: a piece is not an ellipse") };
        let g = e.geometry.to_f64();
        // One centre for every piece (logo-vsf fits one): the table keeps none.
        let c = (g[0], g[1]);
        let c0 = *centre.get_or_insert(c);
        assert!((c.0 - c0.0).abs() < 1e-3 && (c.1 - c0.1).abs() < 1e-3, "logo.vsf: the pieces do not share a centre");
        let inner = if e.is_ring() { g[4] } else { 0. };
        let (start, sweep) = (e.start.as_ref().map_or(0., |s| s.to_f64()), e.sweep.as_ref().map_or(1., |s| s.to_f64()));
        let (start, sweep) = if sweep < 0. { (start + sweep, -sweep) } else { (start, sweep) };
        let Some(vsf::types::Paint::Colour(colour)) = e.fill.as_ref() else { panic!("logo.vsf: a piece without a colour fill") };
        let lin = colour.to_rgb_linear_f32().expect("logo.vsf: a fill that is not a colour");
        let code = |l: f32| (l.sqrt() * 16384. + 0.5) as u16;
        let grey = code(0.2584 * lin.r + 0.6346 * lin.g + 0.1070 * lin.b);
        rmax = rmax.max(g[2]);
        pieces.push((inner, g[2], start - start.floor(), sweep, [code(lin.r), code(lin.g), code(lin.b)], grey));
    }
    let mut table = format!("/// The ferros mark's pieces (build.rs, from logo.vsf): (inner, outer, start, sweep, lit colour, grey).\npub const LOGO: [(f32, f32, f32, f32, [u16; 3], u16); {}] = [\n", pieces.len());
    for (i, o, s, w, c, g) in &pieces {
        table += &format!("    ({:?}, {:?}, {:?}, {:?}, [{}, {}, {}], {}),\n", (i / rmax) as f32, (o / rmax) as f32, *s as f32, *w as f32, c[0], c[1], c[2], g);
    }
    table += "];\n";
    std::fs::write(std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("logo.rs"), table).unwrap();
}

/// The compose kernel (GPU-COMPOSE.md step 2, src/gpu_kernel.rs) recorded on the host into words for the display to load: `WORDS`, `CONSTS` (its uniforms past the caller's), `REGS64`.
fn compose_kernel() {
    use std::fmt::Write;
    println!("cargo:rerun-if-changed=src/gpu_kernel.rs");
    println!("cargo:rerun-if-changed=src/gpu_layout.rs");
    let k = gpu_kernel::record();
    assert!(k.words.len() * 8 <= gpu_layout::K_ROOM, "the compose kernel's {} words pass its room", k.words.len());
    let mut s = String::new();
    let _ = writeln!(s, "/// {} words, peak {} registers.", k.words.len(), k.peak);
    let _ = writeln!(s, "pub const REGS64: bool = {};", k.regs64);
    s.push_str("pub static WORDS: &[u64] = &[");
    for x in &k.words {
        let _ = write!(s, "{x},");
    }
    s.push_str("];\npub static CONSTS: &[u64] = &[");
    for x in &k.consts {
        let _ = write!(s, "{x},");
    }
    s.push_str("];\n");
    // The layered window's kernel (step 4, src/gpu_layers.rs): `L_WORDS`, `L_CONSTS`, `L_REGS64`, its workgroup and storage.
    println!("cargo:rerun-if-changed=src/gpu_layers.rs");
    println!("cargo:rerun-if-changed=../compbench/src/noise.rs");
    let l = gpu_layers::record();
    assert!(l.words.len() * 8 <= gpu_layout::K_ROOM, "the layered kernel's {} words pass its room", l.words.len());
    let _ = writeln!(s, "/// The layered kernel: {} words, peak {} registers, workgroup 2^{}, {} bytes of storage.", l.words.len(), l.peak, l.wg_log2, l.shared_bytes);
    let _ = writeln!(s, "pub const L_REGS64: bool = {};\npub const L_WG_LOG2: u32 = {};\npub const L_SHARED: u32 = {};", l.regs64, l.wg_log2, l.shared_bytes);
    s.push_str("pub static L_WORDS: &[u64] = &[");
    for x in &l.words {
        let _ = write!(s, "{x},");
    }
    s.push_str("];\npub static L_CONSTS: &[u64] = &[");
    for x in &l.consts {
        let _ = write!(s, "{x},");
    }
    s.push_str("];\n");
    std::fs::write(std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("compose.rs"), s).unwrap();
}
