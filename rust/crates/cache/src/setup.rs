//! Setup-reuse snapshots: a copy of the configured paths of a prepared worktree, stored under the setup entry by a key
//! derived from the dependency files.
use std::fs;
use std::path::{Component, Path, PathBuf};

use capstan_config::{sha256, SetupReuse};

use crate::error::{CacheError, Result};
use crate::fsutil::{mkdir_private, nanos, remove_tree, touch};
use crate::runtime::{Runtime, SETUP_ENTRY};

/// The snapshot key: a hash of the platform and of the content of every key file. `None` when no key file exists.
pub fn setup_key(worktree: &Path, reuse: &SetupReuse) -> Result<Option<String>> {
    let mut text = format!("{}-{}\n", std::env::consts::OS, std::env::consts::ARCH);
    let mut any = false;
    for file in &reuse.key_files {
        relative(file)?;
        match fs::read(worktree.join(file)) {
            Ok(bytes) => {
                any = true;
                text.push_str(&format!("{file}\0{}\0", bytes.len()));
                text.push_str(&String::from_utf8_lossy(&bytes));
                text.push('\n');
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                text.push_str(&format!("{file}\0missing\n"))
            }
            Err(e) => return Err(CacheError::io(format!("read {file}"), e)),
        }
    }
    Ok(any.then(|| sha256(&text)))
}

fn relative(path: &str) -> Result<PathBuf> {
    let p = Path::new(path);
    let ok = !path.is_empty() && p.components().all(|c| matches!(c, Component::Normal(_)));
    if ok {
        Ok(p.to_path_buf())
    } else {
        Err(CacheError::Invalid(format!(
            "{path} must be a relative path without .. segments"
        )))
    }
}

fn key_ok(key: &str) -> Result<()> {
    if !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        Ok(())
    } else {
        Err(CacheError::Invalid(
            "a snapshot key is letters, digits, underscore and dash".into(),
        ))
    }
}

/// Fails when a directory on the way to `base/rel` is a symlink (the copy would leave the tree).
fn no_symlink_parent(base: &Path, rel: &Path) -> Result<()> {
    let mut current = base.to_path_buf();
    let count = rel.components().count();
    for component in rel.components().take(count.saturating_sub(1)) {
        current.push(component);
        if fs::symlink_metadata(&current).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(CacheError::Invalid(format!(
                "{} is a symlink",
                current.display()
            )));
        }
    }
    Ok(())
}

fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(src)?, dst)
    } else if meta.is_dir() {
        fs::create_dir(dst)?;
        for child in fs::read_dir(src)? {
            let child = child?;
            copy_tree(&child.path(), &dst.join(child.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(src, dst).map(|_| ())
    }
}

fn copy_paths(from: &Path, to: &Path, paths: &[String]) -> Result<u32> {
    let mut copied = 0;
    for path in paths {
        let rel = relative(path)?;
        let source = from.join(&rel);
        no_symlink_parent(from, &rel)?;
        if fs::symlink_metadata(&source).is_err() {
            continue;
        }
        no_symlink_parent(to, &rel)?;
        if let Some(parent) = to.join(&rel).parent() {
            fs::create_dir_all(parent).map_err(|e| CacheError::io("create parent", e))?;
        }
        remove_tree(&to.join(&rel)).map_err(|e| CacheError::io(format!("replace {path}"), e))?;
        copy_tree(&source, &to.join(&rel))
            .map_err(|e| CacheError::io(format!("copy {path}"), e))?;
        copied += 1;
    }
    Ok(copied)
}

impl Runtime {
    /// Copies the snapshot `key` into `dest_worktree`; `false` when there is none.
    pub fn restore_snapshot(
        &self,
        key: &str,
        dest_worktree: &Path,
        paths: &[String],
    ) -> Result<bool> {
        key_ok(key)?;
        let snapshot = self.entry_dir(SETUP_ENTRY).join(key);
        if !snapshot.is_dir() {
            return Ok(false);
        }
        let copied = copy_paths(&snapshot, dest_worktree, paths)?;
        let _ = touch(&snapshot.join(".last-used"));
        Ok(copied > 0)
    }

    /// Stores `paths` of `src_worktree` as snapshot `key`: copy to a temporary name, then rename. An existing key wins.
    pub fn store_snapshot(&self, key: &str, src_worktree: &Path, paths: &[String]) -> Result<()> {
        key_ok(key)?;
        paths.iter().try_for_each(|p| relative(p).map(|_| ()))?;
        let setup = self.entry_dir(SETUP_ENTRY);
        let target = setup.join(key);
        if target.exists() {
            return Ok(());
        }
        mkdir_private(&setup)?;
        let tmp = setup.join(format!("{key}.tmp-{}-{}", std::process::id(), nanos()));
        mkdir_private(&tmp)?;
        let result = copy_paths(src_worktree, &tmp, paths).and_then(|copied| {
            if copied == 0 {
                return Ok(false);
            }
            touch(&tmp.join(".last-used")).map_err(|e| CacheError::io("touch snapshot", e))?;
            Ok(true)
        });
        match result {
            Ok(true) => match fs::rename(&tmp, &target) {
                Ok(()) => Ok(()),
                Err(_) if target.exists() => {
                    let _ = remove_tree(&tmp);
                    Ok(())
                }
                Err(e) => {
                    let _ = remove_tree(&tmp);
                    Err(CacheError::io("store snapshot", e))
                }
            },
            Ok(false) => remove_tree(&tmp).map_err(|e| CacheError::io("clean up", e)),
            Err(e) => {
                let _ = remove_tree(&tmp);
                Err(e)
            }
        }
    }
}
