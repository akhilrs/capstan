//! The Node hand-over is gone: nothing in this crate looks for a Node CLI or for a Node binary beside cstan, and the fallback it was
//! built on does not exist.
use std::path::Path;

fn files_below(directory: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_below(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn the_crate_never_mentions_the_node_hand_over() {
    // The needles are built from parts so that this file does not contain them.
    let needles = [
        ["CSTAN", "_NODE"].concat(),
        ["cstan", "-node"].concat(),
        ["hand_to", "_node"].concat(),
        ["enum ", "Fallback"].concat(),
        ["Fallback", "::"].concat(),
    ];
    let mut files = Vec::new();
    files_below(Path::new(env!("CARGO_MANIFEST_DIR")), &mut files);
    assert!(files.len() > 100, "the crate was not found");
    let mut found = Vec::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for needle in &needles {
            if text.contains(needle.as_str()) {
                found.push(format!("{} mentions {needle}", file.display()));
            }
        }
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}
