fn main() {
    let ld = std::path::Path::new("payloads/tidetest/payload.ld");
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    // Position-independent, as the clock links (payloads/clock/build.rs).
    for a in ["-pie", "--no-dynamic-linker", "-z", "norelro", "-z", "notext"] {
        println!("cargo:rustc-link-arg={a}");
    }
    println!("cargo:rerun-if-changed={}", ld.display());

    // Every station document in ferros_tide/stations/, and the catalog, bundled by name.
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let dir = manifest.join("../../ferros_tide/stations");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut docs: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.to_string_lossy().ends_with(".tide.vsf")).collect();
    docs.sort();
    let mut out = String::from("pub static STATIONS: &[(&str, &[u8])] = &[\n");
    for p in &docs {
        let id = p.file_name().unwrap().to_string_lossy().trim_end_matches(".tide.vsf").to_string();
        out += &format!("    ({id:?}, include_bytes!({:?})),\n", p.canonicalize().unwrap());
    }
    out += "];\n";
    out += &format!("pub static CATALOG: &[u8] = include_bytes!({:?});\n", dir.join("catalog.vsf").canonicalize().unwrap());
    std::fs::write(std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("stations.rs"), out).unwrap();
}
