//! Embeds the repository's migrations/*.sql into the crate. The migrations are read from the directory at build time
//! (no copies) and cargo is told to rerun this script when the directory or any file in it changes, so a new or changed
//! migration is picked up by the next build.
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let directory = manifest.join("..").join("..").join("..").join("migrations");
    let directory = directory
        .canonicalize()
        .unwrap_or_else(|e| panic!("migrations directory {}: {e}", directory.display()));
    println!("cargo:rerun-if-changed={}", directory.display());

    let mut found: Vec<(u32, String)> = Vec::new();
    for entry in fs::read_dir(&directory).expect("read migrations directory") {
        let entry = entry.expect("read migrations entry");
        let name = entry
            .file_name()
            .into_string()
            .unwrap_or_else(|n| panic!("migration file name is not UTF-8: {n:?}"));
        if !name.ends_with(".sql") {
            continue;
        }
        println!("cargo:rerun-if-changed={}", entry.path().display());
        let digits: String = name.chars().take_while(char::is_ascii_digit).collect();
        let version: u32 = digits
            .parse()
            .unwrap_or_else(|_| panic!("migration {name} must start with its version number"));
        assert!(
            name[digits.len()..].starts_with('_'),
            "migration {name} must be <version>_<description>.sql"
        );
        found.push((version, name));
    }
    found.sort();
    for (index, (version, name)) in found.iter().enumerate() {
        assert!(
            *version as usize == index + 1,
            "migrations must be numbered 1..N without gaps or repeats; found {name} at position {}",
            index + 1
        );
    }

    let mut out = String::from("&[\n");
    for (version, name) in &found {
        let path = directory.join(name);
        let _ = writeln!(
            out,
            "    ({version}, {name:?}, include_bytes!({:?})),",
            path.display().to_string()
        );
    }
    out.push_str("]\n");
    let target = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("migrations.rs");
    fs::write(target, out).expect("write migrations.rs");
}
