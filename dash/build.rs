//! Exposes the version the binary reports: CSTAN_VERSION when set, else the repository's VERSION file when the crate is
//! built inside the repository, else the crate's own version.
use std::fs;

fn repository_version() -> Option<String> {
    let text = fs::read_to_string("../VERSION").ok()?;
    let version = text.trim();
    (!version.is_empty()).then(|| version.to_string())
}

fn main() {
    println!("cargo:rerun-if-env-changed=CSTAN_VERSION");
    println!("cargo:rerun-if-changed=../VERSION");
    println!("cargo:rerun-if-changed=build.rs");
    let version = std::env::var("CSTAN_VERSION")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(repository_version)
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_default());
    println!("cargo:rustc-env=CSTAN_DASH_VERSION={version}");
}
