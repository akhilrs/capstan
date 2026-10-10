//! Writing a script and executing it without the "Text file busy" race: a file that is still open for writing cannot be
//! executed (ETXTBSY), and a parallel test's fork can hold a copy of the descriptor until it execs. So a script is written
//! under a temporary name in its own directory, given its mode, synced and closed, and only then renamed into place; and a
//! test that executes such a file holds [`exclusive`] around the spawn (the pattern of `herdr/tests/runner.rs`).

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

static EXEC: Mutex<()> = Mutex::new(());

/// One executing test at a time per test binary.
pub fn exclusive() -> MutexGuard<'static, ()> {
    EXEC.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `text` at `path` with `mode`, written under a temporary name and renamed into place once closed.
pub fn write_script(path: &Path, text: &str, mode: u32) {
    let directory = path.parent().expect("a script has a directory");
    std::fs::create_dir_all(directory).unwrap();
    let temp = directory.join(format!(
        ".{}.tmp{}",
        path.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    {
        let mut file = std::fs::File::create(&temp).unwrap();
        file.write_all(text.as_bytes()).unwrap();
        file.sync_all().unwrap();
    }
    std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(mode)).unwrap();
    std::fs::rename(&temp, path).unwrap();
}
