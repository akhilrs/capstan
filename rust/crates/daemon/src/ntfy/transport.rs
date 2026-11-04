//! How a notification reaches the server: the [`NtfyTransport`] seam and its curl implementation.
//!
//! The curl child gets its whole request on standard input as a configuration, so its argument list holds no URL, secret
//! or body, and its environment holds `PATH` and the CA locations only.

use super::redact::{self, userinfo_parts};
use crate::loops::StopFlag;
use capstan_config::{Ntfy, NtfyAuth, Secret};
use serde_json::{json, Map, Value};
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long past `--max-time` a curl child may live before it is killed.
const KILL_GRACE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(100);
const BODY_KEEP: usize = 2048;
const TAIL_KEEP: usize = 512;
const STATUS_MARKER: &str = "capstan-http-status:";
const RETRY_MARKER: &str = "capstan-retry-after:";
/// The most seconds a `Retry-After` can ask for.
pub const RETRY_AFTER_CAP_SECONDS: u64 = 60;
/// The variables whose values are CA locations (not secrets) and pass to curl.
const CA_VARIABLES: [&str; 3] = ["SSL_CERT_FILE", "SSL_CERT_DIR", "CURL_CA_BUNDLE"];

/// The ntfy JSON publish message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishRequest {
    pub topic: String,
    pub title: String,
    pub message: String,
    pub priority: Option<u8>,
    pub tags: Vec<String>,
    pub click: Option<String>,
}

impl PublishRequest {
    /// The JSON object text; it always starts with `{`.
    pub fn to_json(&self) -> String {
        let mut map = Map::new();
        map.insert("topic".into(), json!(self.topic));
        map.insert("title".into(), json!(self.title));
        map.insert("message".into(), json!(self.message));
        if let Some(priority) = self.priority {
            map.insert("priority".into(), json!(priority));
        }
        if !self.tags.is_empty() {
            map.insert("tags".into(), json!(self.tags));
        }
        if let Some(click) = &self.click {
            map.insert("click".into(), json!(click));
        }
        Value::Object(map).to_string()
    }
}

/// Why an attempt failed. `error` is already redacted and at most 200 characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportError {
    pub reason: String,
    pub http_status: Option<u16>,
    pub error: String,
    /// Seconds a 429 asked to wait, already capped.
    pub retry_after: Option<u64>,
}

impl TransportError {
    pub fn new(reason: &str, error: impl Into<String>) -> Self {
        Self {
            reason: reason.to_string(),
            http_status: None,
            error: error.into(),
            retry_after: None,
        }
    }
}

/// Reasons worth another attempt.
pub fn retriable(reason: &str) -> bool {
    matches!(
        reason,
        "timeout" | "connect_error" | "transport_error" | "http_429" | "http_5xx"
    )
}

/// One publish attempt. `Ok` is a 2xx answer; anything else is an error with its reason.
pub trait NtfyTransport: Send + Sync {
    fn publish(
        &self,
        req: &PublishRequest,
        secret: Option<&Secret>,
        timeout: Duration,
        stop: &StopFlag,
    ) -> Result<u16, TransportError>;
}

/// A value as a double-quoted curl configuration string; control characters are refused.
/// A value that holds a control character cannot be quoted.
#[derive(Debug, PartialEq, Eq)]
pub struct ControlChar;

pub fn curl_quote(value: &str) -> Result<String, ControlChar> {
    if value.chars().any(|c| (c as u32) < 0x20 || c as u32 == 0x7f) {
        return Err(ControlChar);
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        if c == '\\' || c == '"' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    Ok(out)
}

/// The host of a URL, without userinfo and port; IPv6 brackets are kept off.
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if let Some(inner) = host_port.strip_prefix('[') {
        return inner.split(']').next().unwrap_or("").to_string();
    }
    host_port.split(':').next().unwrap_or("").to_string()
}

/// Whether a `NO_PROXY` list names `host`: `*`, the host itself or a parent domain (a leading `.` or `*.` allowed).
pub fn no_proxy_covers(list: &str, host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    list.split(',').map(str::trim).any(|entry| {
        let entry = entry.to_ascii_lowercase();
        let entry = entry.trim_start_matches("*.").trim_start_matches('.');
        if entry == "*" {
            return true;
        }
        if entry.is_empty() {
            return false;
        }
        host == entry || host.ends_with(&format!(".{entry}"))
    })
}

pub type EnvReader = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The transport that runs curl.
pub struct CurlTransport {
    program: PathBuf,
    server: String,
    user: Option<String>,
    ca_file: Option<PathBuf>,
    env: EnvReader,
}

impl CurlTransport {
    /// For the server, user and CA file of `ntfy`; the program is `curl` on `PATH`.
    pub fn new(ntfy: &Ntfy) -> Self {
        let user = match &ntfy.auth {
            NtfyAuth::BasicEnv { user, .. } | NtfyAuth::BasicFile { user, .. } => {
                Some(user.clone())
            }
            _ => None,
        };
        Self {
            program: PathBuf::from("curl"),
            server: ntfy.server.clone(),
            user,
            ca_file: ntfy.ca_file.clone(),
            env: Arc::new(|name| std::env::var(name).ok()),
        }
    }

    /// Runs `path` instead of `curl` (a test seam).
    pub fn with_program(mut self, path: impl Into<PathBuf>) -> Self {
        self.program = path.into();
        self
    }

    /// Reads proxy and CA variables through `env` instead of the process environment (a test seam).
    pub fn with_env(mut self, env: EnvReader) -> Self {
        self.env = env;
        self
    }

    fn var(&self, upper: &str) -> Option<String> {
        (self.env)(upper)
            .or_else(|| (self.env)(&upper.to_ascii_lowercase()))
            .filter(|v| !v.is_empty())
    }

    /// The proxy for this server, read now: `HTTPS_PROXY` for https, `HTTP_PROXY` for http, none when `NO_PROXY` covers the host.
    fn proxy(&self) -> Option<(String, Option<String>)> {
        let name = if self.server.starts_with("https://") {
            "HTTPS_PROXY"
        } else {
            "HTTP_PROXY"
        };
        let proxy = self.var(name)?;
        let no_proxy = self.var("NO_PROXY");
        if let Some(list) = &no_proxy {
            if no_proxy_covers(list, &host_of(&self.server)) {
                return None;
            }
        }
        Some((proxy, no_proxy))
    }

    /// The configuration text curl reads on standard input, and the strings to mask in anything curl prints.
    fn config(
        &self,
        req: &PublishRequest,
        secret: Option<&Secret>,
    ) -> Result<(String, Vec<String>), TransportError> {
        let refused = |what: &str| {
            TransportError::new(
                "transport_error",
                format!("{what} holds a control character"),
            )
        };
        let q = |value: &str, what: &str| curl_quote(value).map_err(|_| refused(what));
        let mut masks = vec![self.server.clone()];
        let mut lines = vec![
            format!(
                "url = {}",
                q(&format!("{}/", self.server), "the server URL")?
            ),
            "request = \"POST\"".to_string(),
            "header = \"Content-Type: application/json\"".to_string(),
        ];
        if let Some(secret) = secret {
            masks.push(secret.expose().to_string());
            match &self.user {
                Some(user) => {
                    masks.push(format!("{user}:{}", secret.expose()));
                    lines.push(format!(
                        "user = {}",
                        q(&format!("{user}:{}", secret.expose()), "the credentials")?
                    ));
                }
                None => lines.push(format!(
                    "header = {}",
                    q(
                        &format!("Authorization: Bearer {}", secret.expose()),
                        "the token"
                    )?
                )),
            }
        }
        lines.push(format!(
            "data-binary = {}",
            q(&req.to_json(), "the message")?
        ));
        if let Some(ca) = &self.ca_file {
            lines.push(format!(
                "cacert = {}",
                q(&ca.to_string_lossy(), "the CA file path")?
            ));
        }
        if let Some((proxy, no_proxy)) = self.proxy() {
            masks.push(proxy.clone());
            masks.extend(userinfo_parts(&proxy));
            lines.push(format!("proxy = {}", q(&proxy, "the proxy URL")?));
            if let Some(list) = no_proxy {
                lines.push(format!("noproxy = {}", q(&list, "NO_PROXY")?));
            }
        }
        lines.push("output = \"-\"".to_string());
        // `\n` here is curl's own escape for a newline, not a quoted value.
        lines.push(format!(
            "write-out = \"\\n{RETRY_MARKER}%header{{retry-after}}\\n{STATUS_MARKER}%{{http_code}}\""
        ));
        Ok((lines.join("\n") + "\n", masks))
    }

    fn command(&self, timeout: Duration) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args([
                "-q",
                "--silent",
                "--show-error",
                "--proto",
                "=http,https",
                "--max-time",
            ])
            .arg(timeout.as_secs().max(1).to_string())
            .args(["--config", "-"])
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        if let Some(path) = (self.env)("PATH") {
            command.env("PATH", path);
        }
        for name in CA_VARIABLES {
            if let Some(value) = (self.env)(name) {
                command.env(name, value);
            }
        }
        command
    }
}

/// What a reader thread kept: the first bytes and the last bytes.
struct Kept {
    head: Vec<u8>,
    tail: Vec<u8>,
}

fn read_kept<R: Read>(mut reader: R) -> Kept {
    let mut kept = Kept {
        head: Vec::new(),
        tail: Vec::new(),
    };
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = BODY_KEEP.saturating_sub(kept.head.len());
                kept.head.extend_from_slice(&chunk[..n.min(room)]);
                kept.tail.extend_from_slice(&chunk[..n]);
                if kept.tail.len() > TAIL_KEEP {
                    let cut = kept.tail.len() - TAIL_KEEP;
                    kept.tail.drain(..cut);
                }
            }
        }
    }
    kept
}

/// The status and `Retry-After` of the last two lines of what curl wrote; the final line must be the status marker.
fn parse_trailer(tail: &[u8]) -> Option<(u16, Option<u64>)> {
    let text = String::from_utf8_lossy(tail);
    let mut lines = text.rsplit('\n');
    let last = lines.next()?;
    let code = last.strip_prefix(STATUS_MARKER)?;
    if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let retry = lines
        .next()
        .and_then(|line| line.strip_prefix(RETRY_MARKER))
        .and_then(|v| v.trim().parse::<u64>().ok());
    Some((code.parse().ok()?, retry))
}

fn kill_group(child: &mut std::process::Child) {
    // SAFETY: the child was started with its own process group, so -pid names the group this transport created.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

impl NtfyTransport for CurlTransport {
    fn publish(
        &self,
        req: &PublishRequest,
        secret: Option<&Secret>,
        timeout: Duration,
        stop: &StopFlag,
    ) -> Result<u16, TransportError> {
        let (config, masks) = self.config(req, secret)?;
        let scrub = |text: &str| redact::scrub(text, &masks);
        let mut child = match self.command(timeout).spawn() {
            Ok(child) => child,
            Err(_) => {
                return Err(TransportError::new(
                    "transport_unavailable",
                    "curl could not be started",
                ))
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            // A child that does not read its configuration is judged by how it exits.
            let _ = stdin.write_all(config.as_bytes());
        }
        let stdout = child
            .stdout
            .take()
            .map(|s| std::thread::spawn(move || read_kept(s)));
        let stderr = child
            .stderr
            .take()
            .map(|s| std::thread::spawn(move || read_kept(s)));
        let deadline = Instant::now() + timeout + KILL_GRACE;
        let exit = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(_) => break None,
            }
            if stop.stopped() {
                kill_group(&mut child);
                join(stdout);
                join(stderr);
                return Err(TransportError::new("stopped", "the controller is stopping"));
            }
            if Instant::now() >= deadline {
                kill_group(&mut child);
                join(stdout);
                join(stderr);
                return Err(TransportError::new(
                    "timeout",
                    "curl did not finish in time",
                ));
            }
            stop.sleep(POLL);
        };
        let out = join(stdout);
        let err = join(stderr);
        let err_text = scrub(&String::from_utf8_lossy(&err.head));
        let body_text = || scrub(&String::from_utf8_lossy(&out.head));
        let Some(exit) = exit else {
            kill_group(&mut child);
            return Err(TransportError::new(
                "transport_error",
                "curl could not be waited for",
            ));
        };
        match exit.code() {
            Some(0) => {}
            Some(1 | 2 | 4) => return Err(TransportError::new("transport_unavailable", err_text)),
            Some(28) => return Err(TransportError::new("timeout", err_text)),
            Some(6 | 7) => return Err(TransportError::new("connect_error", err_text)),
            _ => return Err(TransportError::new("transport_error", err_text)),
        }
        let Some((status, retry_after)) = parse_trailer(&out.tail) else {
            return Err(TransportError::new(
                "transport_error",
                "curl did not report an HTTP status",
            ));
        };
        let reason = match status {
            200..=299 => return Ok(status),
            401 => "unauthorized",
            403 => "forbidden",
            429 => "http_429",
            300..=399 => "http_3xx",
            400..=499 => "http_4xx",
            500..=599 => "http_5xx",
            _ => "transport_error",
        };
        let text = if err_text.is_empty() {
            body_text()
        } else {
            err_text
        };
        Err(TransportError {
            reason: reason.to_string(),
            http_status: (status != 0).then_some(status),
            error: redact::clip(&format!("HTTP {status} {text}")),
            retry_after: (status == 429)
                .then_some(retry_after.map(|s| s.min(RETRY_AFTER_CAP_SECONDS)))
                .flatten(),
        })
    }
}

fn join(handle: Option<std::thread::JoinHandle<Kept>>) -> Kept {
    handle.and_then(|h| h.join().ok()).unwrap_or(Kept {
        head: Vec::new(),
        tail: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_escapes_and_refuses() {
        assert_eq!(curl_quote("a\"b\\c").unwrap(), "\"a\\\"b\\\\c\"");
        assert!(curl_quote("a\nb").is_err());
        assert!(curl_quote("a\u{7f}").is_err());
        assert_eq!(curl_quote("héllo 🎉").unwrap(), "\"héllo 🎉\"");
    }

    #[test]
    fn hosts_and_no_proxy() {
        assert_eq!(host_of("https://u:p@Ntfy.example:8443/x"), "Ntfy.example");
        assert_eq!(host_of("http://[::1]:80"), "::1");
        assert!(no_proxy_covers(
            "localhost, .example.com",
            "ntfy.example.com"
        ));
        assert!(no_proxy_covers("EXAMPLE.com", "example.com"));
        assert!(!no_proxy_covers("example.com", "badexample.com"));
        assert!(no_proxy_covers("*", "anything"));
    }

    #[test]
    fn the_trailer_must_be_the_last_line() {
        assert_eq!(
            parse_trailer(b"body\ncapstan-retry-after:5\ncapstan-http-status:429"),
            Some((429, Some(5)))
        );
        assert_eq!(
            parse_trailer(
                b"x\ncapstan-http-status:200\ncapstan-retry-after:\ncapstan-http-status:204"
            ),
            Some((204, None))
        );
        assert_eq!(parse_trailer(b"capstan-http-status:200\nmore"), None);
        assert_eq!(parse_trailer(b"nothing"), None);
    }
}
