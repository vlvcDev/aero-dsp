use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    let core = manifest.join("../../vendor/NeuralAmpModelerCore");
    let nam_sources = core.join("NAM");

    if !core.join("Dependencies/eigen/Eigen").is_dir() {
        panic!("NAM Core submodules are missing; run git submodule update --init --recursive");
    }

    let mut sources = Vec::new();
    collect_cpp(&nam_sources, &mut sources);
    sources.sort();

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++20")
        .define("NAM_SAMPLE_FLOAT", None)
        .define("NAM_ENABLE_A2_FAST", None)
        .define("NAM_DEFAULT_MAX_BUFFER_SIZE", "64")
        // NAM/Eigen must meet realtime deadlines even in `cargo run` dev
        // builds, and Eigen's internal asserts are costly in hot loops.
        .opt_level(3)
        .define("NDEBUG", None)
        .include(&core)
        .include(core.join("Dependencies/eigen"))
        .include(core.join("Dependencies/nlohmann"))
        .file(manifest.join("native/pedal_nam.cpp"))
        .files(&sources)
        .warnings(false)
        .cargo_metadata(false)
        .compile("pedal_nam");

    println!("cargo:rustc-link-search=native={}", output.display());
    println!("cargo:rustc-link-lib=static:+whole-archive=pedal_nam");
    println!("cargo:rustc-link-lib=dylib=stdc++");
    println!("cargo:rustc-link-lib=stdc++fs");
    println!("cargo:rerun-if-changed=native/pedal_nam.cpp");
    println!("cargo:rerun-if-changed=native/pedal_nam.h");
    println!("cargo:rerun-if-changed={}", nam_sources.display());
}

fn collect_cpp(directory: &Path, output: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("read NAM source directory") {
        let path = entry.expect("read NAM source entry").path();
        if path.is_dir() {
            collect_cpp(&path, output);
        } else if path.extension().is_some_and(|extension| extension == "cpp") {
            output.push(path);
        }
    }
}
