//! Embeds the version `cstan --version` prints: CSTAN_VERSION when set, else the one in the repository's VERSION file,
//! else this crate's own version (a build outside the repository), so every build reports the same number.
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=CSTAN_VERSION");
    let file = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"))
        .join("../../../VERSION");
    println!("cargo:rerun-if-changed={}", file.display());
    let version = match std::env::var("CSTAN_VERSION") {
        Ok(version) if !version.is_empty() => version,
        _ => match std::fs::read_to_string(&file) {
            Ok(text) if !text.trim().is_empty() => text.trim().to_string(),
            _ => std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION"),
        },
    };
    println!("cargo:rustc-env=CSTAN_FRONT_VERSION={version}");
}
