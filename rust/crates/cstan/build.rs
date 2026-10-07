//! Embeds the version `cstan __front-version` prints: CSTAN_VERSION when set, else the one in the repository's
//! package.json, so every build (the release script's and a plain `cargo build`) reports the same number.
use std::path::PathBuf;

fn version_from_package_json(text: &str) -> Option<String> {
    let at = text.find("\"version\"")? + "\"version\"".len();
    let rest = text[at..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_string())
}

fn main() {
    println!("cargo:rerun-if-env-changed=CSTAN_VERSION");
    let package = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"))
        .join("../../../package.json");
    println!("cargo:rerun-if-changed={}", package.display());
    let version = match std::env::var("CSTAN_VERSION") {
        Ok(version) if !version.is_empty() => version,
        _ => {
            let text = std::fs::read_to_string(&package)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", package.display()));
            version_from_package_json(&text).expect("package.json has no version")
        }
    };
    println!("cargo:rustc-env=CSTAN_FRONT_VERSION={version}");
}
