//! The daemon as a process: `cstan-daemon` started in a project directory, its standard output read line by line, and its
//! stop by SIGTERM. Used where a test needs the real binary (the lock, the socket groups).

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// A running `cstan-daemon`.
pub struct Daemon {
    pub child: Child,
    lines: Receiver<String>,
    /// Every line of standard output read so far.
    pub seen: Vec<String>,
}

/// The path of the binary cargo built for these tests.
pub fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_cstan-daemon")
}

impl Daemon {
    /// Starts `cstan-daemon` with `directory` as its working directory (and no launcher: `CAPSTAN_LAUNCH=off`).
    pub fn start(directory: &Path) -> Daemon {
        let mut child = Command::new(binary())
            .current_dir(directory)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("CAPSTAN_LAUNCH", "off")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("cstan-daemon starts");
        let stdout = child.stdout.take().expect("piped output");
        let (sender, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Daemon {
            child,
            lines,
            seen: Vec::new(),
        }
    }

    /// Reads output until a line satisfies `wanted`; returns it, or None when the process ended or `timeout` passed.
    pub fn wait_for_line(
        &mut self,
        timeout: Duration,
        wanted: impl Fn(&str) -> bool,
    ) -> Option<String> {
        let deadline = Instant::now() + timeout;
        if let Some(found) = self.seen.iter().find(|line| wanted(line)) {
            return Some(found.clone());
        }
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self
                .lines
                .recv_timeout(left.min(Duration::from_millis(100)))
            {
                Ok(line) => {
                    self.seen.push(line.clone());
                    if wanted(&line) {
                        return Some(line);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if Instant::now() >= deadline {
                        return None;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    /// Waits for the process to end; None when it is still running after `timeout`.
    pub fn wait_exit(&mut self, timeout: Duration) -> Option<i32> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.child.try_wait().expect("the child can be waited for") {
                Some(status) => return status.code().or(Some(-1)),
                None if Instant::now() >= deadline => return None,
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    /// Sends SIGTERM (to the child this helper started) and waits for its exit code.
    pub fn terminate(&mut self) -> Option<i32> {
        // SAFETY: the pid is a child this struct spawned and still owns.
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGTERM);
        }
        self.wait_exit(Duration::from_secs(15))
    }

    /// What the process wrote to standard error (read after it ended).
    pub fn stderr(&mut self) -> String {
        use std::io::Read;
        let mut text = String::new();
        if let Some(mut stderr) = self.child.stderr.take() {
            let _ = stderr.read_to_string(&mut text);
        }
        text
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
