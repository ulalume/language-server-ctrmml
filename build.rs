use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("missing CARGO_MANIFEST_DIR"));
    let default_cmd_dir = manifest_dir.join("ctrmml-cmd");
    let cmd_dir = env::var("CTRMML_CMD_DIR")
        .map(PathBuf::from)
        .unwrap_or(default_cmd_dir);

    if !cmd_dir.exists() {
        panic!("ctrmml-cmd directory not found: {}", cmd_dir.display());
    }

    println!("cargo:rerun-if-changed={}", cmd_dir.join("CMakeLists.txt").display());
    println!("cargo:rerun-if-changed={}", cmd_dir.join("src/ctrmml_cmd_c_api.h").display());
    println!("cargo:rerun-if-changed={}", cmd_dir.join("src/ctrmml_cmd_c_api.cpp").display());

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("missing OUT_DIR"));
    let build_id = hex_hash(cmd_dir.to_string_lossy().as_bytes());
    let build_dir = out_dir.join(format!("ctrmml-cmd-build-{}", build_id));
    if let Some(cmake_home) = read_cmake_home(&build_dir) {
        if cmake_home != cmd_dir {
            let _ = std::fs::remove_dir_all(&build_dir);
        }
    }

    let mut configure = Command::new("cmake");
    configure
        .arg("-S")
        .arg(&cmd_dir)
        .arg("-B")
        .arg(&build_dir)
        .arg("-DCMAKE_BUILD_TYPE=Release");
    run_cmd(&mut configure, "cmake configure");

    let mut build = Command::new("cmake");
    build.arg("--build").arg(&build_dir).arg("--target").arg("ctrmml-cmd-lib");
    if cfg!(windows) {
        build.arg("--config").arg("Release");
    }
    run_cmd(&mut build, "cmake build");

    let search_paths = vec![
        build_dir.clone(),
        build_dir.join("Release"),
        build_dir.join("_deps/ctrmml-build"),
        build_dir.join("_deps/ctrmml-build/Release"),
        build_dir.join("_deps/libvgm-build/bin"),
        build_dir.join("_deps/libvgm-build/bin/Release"),
    ];

    for path in &search_paths {
        println!("cargo:rustc-link-search=native={}", path.display());
    }

    let mut libs = Vec::new();
    if let Some(name) = find_lib(&search_paths, "ctrmml-cmd-lib") {
        libs.push(name);
    }
    if let Some(name) = find_lib(&search_paths, "ctrmml") {
        libs.push(name);
    }

    for base in ["vgm-player", "vgm-emu", "vgm-audio", "vgm-utils"] {
        if let Some(name) = find_lib(&search_paths, base) {
            libs.push(name);
        }
    }

    let mut seen = HashSet::new();
    for lib in libs {
        if seen.insert(lib.clone()) {
            println!("cargo:rustc-link-lib=static={}", lib);
        }
    }

    if cfg!(target_os = "macos") {
        println!("cargo:rustc-link-lib=iconv");
        println!("cargo:rustc-link-lib=z");
        println!("cargo:rustc-link-lib=c++");
    } else if cfg!(target_os = "linux") {
        println!("cargo:rustc-link-lib=z");
        println!("cargo:rustc-link-lib=stdc++");
    }
}

fn run_cmd(cmd: &mut Command, label: &str) {
    let status = cmd.status().unwrap_or_else(|e| panic!("failed to run {}: {}", label, e));
    if !status.success() {
        panic!("{} failed with status {}", label, status);
    }
}

fn find_lib(search_paths: &[PathBuf], base: &str) -> Option<String> {
    for dir in search_paths {
        if let Some(name) = find_lib_in_dir(dir, base) {
            return Some(name);
        }
    }
    None
}

fn find_lib_in_dir(dir: &Path, base: &str) -> Option<String> {
    if !dir.exists() {
        return None;
    }

    let static_name = format!("lib{}.a", base);
    if dir.join(&static_name).exists() {
        return Some(base.to_string());
    }

    let windows_name = format!("{}.lib", base);
    if dir.join(&windows_name).exists() {
        return Some(base.to_string());
    }

    if cfg!(windows) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("lib") {
                    continue;
                }
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if stem.starts_with(base) {
                    return Some(stem.to_string());
                }
            }
        }
    }

    None
}

fn hex_hash(data: &[u8]) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    use std::hash::Hasher;
    hasher.write(data);
    format!("{:x}", hasher.finish())
}

fn read_cmake_home(build_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let cache_path = build_dir.join("CMakeCache.txt");
    let cache = std::fs::read_to_string(cache_path).ok()?;
    for line in cache.lines() {
        if let Some(rest) = line.strip_prefix("CMAKE_HOME_DIRECTORY:INTERNAL=") {
            return Some(std::path::PathBuf::from(rest));
        }
    }
    None
}
