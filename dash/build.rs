//! Exposes the version the binary reports: the repository's package.json version when the crate is built inside the
//! repository, else the crate's own.
use std::fs;

fn package_json_version() -> Option<String> {
    let text = fs::read_to_string("../package.json").ok()?;
    let rest = text.split("\"version\"").nth(1)?;
    let start = rest.find('"')? + 1;
    let end = start + rest[start..].find('"')?;
    Some(rest[start..end].to_string())
}

fn main() {
    println!("cargo:rerun-if-changed=../package.json");
    println!("cargo:rerun-if-changed=build.rs");
    let version = package_json_version()
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_default());
    println!("cargo:rustc-env=CSTAN_DASH_VERSION={version}");
}
