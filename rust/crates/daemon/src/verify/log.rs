//! The log file of one verify run: every byte of output, capped. The head (up to half the cap) is written as it arrives;
//! after that only the final tail (the other half) is kept, in memory, and written with a marker when the run ends. A log
//! is therefore at most the cap plus one marker line.
use std::fs::File;
use std::io::Write;
use std::sync::{Arc, Mutex};

struct Inner {
    file: Option<File>,
    head_cap: usize,
    tail_cap: usize,
    head_len: usize,
    tail: Vec<u8>,
    cut: u64,
    failed: bool,
}

/// A cloneable writer onto one capped log; every clone feeds the same file.
#[derive(Clone)]
pub struct CappedLog {
    inner: Arc<Mutex<Inner>>,
}

impl CappedLog {
    pub fn new(file: File, max_bytes: usize) -> Self {
        let head_cap = max_bytes / 2;
        Self {
            inner: Arc::new(Mutex::new(Inner {
                file: Some(file),
                head_cap,
                tail_cap: max_bytes - head_cap,
                head_len: 0,
                tail: Vec::new(),
                cut: 0,
                failed: false,
            })),
        }
    }

    /// Appends `data`; false once the file could not be written (the run goes on without a log).
    pub fn append(&self, data: &[u8]) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if inner.failed {
            return false;
        }
        let mut rest = data;
        if inner.head_len < inner.head_cap {
            let room = inner.head_cap - inner.head_len;
            let (now, later) = rest.split_at(room.min(rest.len()));
            let wrote = inner
                .file
                .as_mut()
                .is_some_and(|file| file.write_all(now).is_ok());
            if !wrote {
                inner.failed = true;
                return false;
            }
            inner.head_len += now.len();
            rest = later;
        }
        if !rest.is_empty() {
            inner.tail.extend_from_slice(rest);
            if inner.tail.len() > inner.tail_cap {
                let excess = inner.tail.len() - inner.tail_cap;
                inner.tail.drain(..excess);
                inner.cut += excess as u64;
            }
        }
        true
    }

    /// Writes the kept tail (after a marker when output was cut) and closes the file.
    pub fn finish(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let Some(mut file) = inner.file.take() else {
            return;
        };
        if inner.failed {
            return;
        }
        if inner.cut > 0 {
            let _ = writeln!(file, "\n[... {} bytes cut ...]", inner.cut);
        }
        let _ = file.write_all(&inner.tail);
        let _ = file.flush();
    }
}

impl Write for CappedLog {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        // A failing log never stops the run: the bytes are dropped.
        self.append(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file() -> (tempfile::TempDir, std::path::PathBuf, File) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.log");
        let file = File::create(&path).unwrap();
        (dir, path, file)
    }

    #[test]
    fn short_output_is_kept_whole() {
        let (_dir, path, file) = temp_file();
        let log = CappedLog::new(file, 100);
        log.append(b"hello\n");
        log.append(b"world\n");
        log.finish();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "hello\nworld\n");
    }

    #[test]
    fn long_output_keeps_the_head_and_the_tail_with_a_marker() {
        let (_dir, path, file) = temp_file();
        let log = CappedLog::new(file, 100);
        for i in 0..100u8 {
            log.append(&[b'a' + i % 26; 10]);
        }
        log.finish();
        let text = std::fs::read(path).unwrap();
        assert!(text.len() <= 100 + 40, "{}", text.len());
        assert!(text.starts_with(b"aaaaaaaaaabbbbbbbbbb"));
        assert!(String::from_utf8_lossy(&text).contains("bytes cut"));
    }

    #[test]
    fn output_that_just_fits_has_no_marker() {
        let (_dir, path, file) = temp_file();
        let log = CappedLog::new(file, 100);
        log.append(&[b'x'; 100]);
        log.finish();
        assert_eq!(std::fs::read(path).unwrap(), vec![b'x'; 100]);
    }
}
