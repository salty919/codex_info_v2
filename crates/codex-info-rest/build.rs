use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR"));
    let root_manifest = manifest_dir.join("../../Cargo.toml");
    println!("cargo:rerun-if-changed={}", root_manifest.display());

    let manifest = fs::read_to_string(&root_manifest).unwrap_or_else(|error| {
        panic!(
            "read distribution manifest {}: {error}",
            root_manifest.display()
        )
    });
    let mut in_package = false;
    let mut version = None;
    for line in manifest.lines() {
        let line = line.trim();
        if line == "[package]" {
            in_package = true;
            continue;
        }
        if in_package && line.starts_with('[') {
            break;
        }
        if in_package && line.starts_with("version") {
            let value = line
                .split_once('=')
                .map(|(_, value)| value.trim().trim_matches('"'))
                .filter(|value| !value.is_empty());
            version = value.map(str::to_owned);
            break;
        }
    }
    let version = version.expect("root package version is required");
    println!("cargo:rustc-env=CODEX_INFO_PRODUCT_VERSION={version}");
}
