//! The feed command (Rust only, operator only): `feed head|page|snapshot|waiting|agents|plans|item|hash`, the kernel's event
//! feed and paged snapshot (`areas::event_feed`, `areas::snapshot_feed`) over the control socket. The feed carries every
//! agent's message, prompt and command text, so agents do not get it. Each subcommand runs one closure on the kernel that
//! makes one kernel call, so no write falls between its reads; the handler reads nothing else.
//!
//! Subcommands take their optional values as flags (arguments are non-empty strings): `feed page --after <seq>:<k> --limit
//! <n> [--anchor <16 hex>]`, `feed recent --epoch <16 hex> --head <n> [--limit <n>]` (the newest outcomes at or below a head, at
//! most 30 reads a minute for the project), `feed waiting --limit <n> [--after <key>] [--kind <kind>]`, `feed agents|plans --limit <n>
//! [--after <key>]`, `feed item <section> <key>` and `feed hash <requestId>`.

use super::shared::{
    fail, map_kernel_error, ok, CommandCall, CommandEnv, CommandResponse, ErrorCode,
};
use super::HandlerMap;
use capstan_kernel::areas::event_feed::{self, Position};
use capstan_kernel::areas::snapshot_feed;
use capstan_kernel::{KernelError, KernelResult};
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::LazyLock;

/// Registers the handler of the route this module serves (feed).
pub fn register(map: &mut HandlerMap) {
    map.insert("feed", feed);
}

const USAGE: &str = "feed takes head, page --after <seq>:<k> --limit <n> [--anchor <hex16>], snapshot, recent --epoch <hex16> --head <n> [--limit <n>], waiting --limit <n> [--after <key>] [--kind <kind>], agents --limit <n> [--after <key>], plans --limit <n> [--after <key>], item <section> <key> or hash <requestId>";

/// The `--limit` of `feed recent` when none is given.
pub const RECENT_DEFAULT_LIMIT: u32 = 20;

/// What a message may not hold: control, format and line and paragraph separator characters.
static UNSAFE_CHARS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]").expect("the pattern is valid"));

fn invalid(message: &str) -> CommandResponse {
    let cleaned = UNSAFE_CHARS.replace_all(message, " ");
    fail(
        ErrorCode::InvalidRequest,
        cleaned.chars().take(200).collect::<String>(),
    )
}

fn usage() -> CommandResponse {
    invalid(USAGE)
}

/// A kernel failure of a feed call: input errors (every `Type`) are the caller's, the rest map as everywhere.
fn refused(error: &KernelError) -> CommandResponse {
    match error {
        KernelError::Type(message) => invalid(message),
        other => map_kernel_error(other),
    }
}

fn answer<T: Serialize>(result: KernelResult<T>) -> CommandResponse {
    match result {
        Ok(value) => ok(serde_json::to_value(value).expect("a feed result serializes")),
        Err(error) => refused(&error),
    }
}

/// The flags of a subcommand: each `--name value` once, only the names in `allowed`.
struct Flags(Vec<(&'static str, String)>);

impl Flags {
    fn parse(args: &[String], allowed: &[&'static str]) -> Result<Flags, CommandResponse> {
        if args.len() % 2 != 0 {
            return Err(usage());
        }
        let mut found: Vec<(&'static str, String)> = Vec::new();
        for pair in args.chunks(2) {
            let Some(name) = allowed.iter().find(|name| pair[0] == **name) else {
                return Err(usage());
            };
            if found.iter().any(|(seen, _)| seen == name) {
                return Err(invalid(&format!("{name} is given twice")));
            }
            found.push((name, pair[1].clone()));
        }
        Ok(Flags(found))
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(flag, _)| *flag == name)
            .map(|(_, value)| value.as_str())
    }

    fn required(&self, name: &str) -> Result<&str, CommandResponse> {
        self.get(name)
            .ok_or_else(|| invalid(&format!("{name} is needed")))
    }

    fn limit(&self) -> Result<u32, CommandResponse> {
        number(self.required("--limit")?, "--limit")
    }

    /// `--limit` of `recent`: 1 to 20, 20 when absent.
    fn recent_limit(&self) -> Result<u32, CommandResponse> {
        let limit = match self.get("--limit") {
            Some(text) => number(text, "--limit")?,
            None => RECENT_DEFAULT_LIMIT,
        };
        if (1..=event_feed::RECENT_MAX_LIMIT).contains(&limit) {
            Ok(limit)
        } else {
            Err(invalid(&format!(
                "--limit must be between 1 and {}",
                event_feed::RECENT_MAX_LIMIT
            )))
        }
    }
}

fn digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// A decimal number without sign or padding sign, in range of `T`.
fn number<T: std::str::FromStr>(text: &str, what: &str) -> Result<T, CommandResponse> {
    if !digits(text) {
        return Err(invalid(&format!("{what} is not a number")));
    }
    text.parse()
        .map_err(|_| invalid(&format!("{what} is out of range")))
}

fn position(text: &str) -> Result<Position, CommandResponse> {
    let Some((seq, k)) = text.split_once(':') else {
        return Err(invalid("--after must be <seq>:<k>"));
    };
    Ok(Position {
        seq: number(seq, "seq")?,
        k: number(k, "k")?,
    })
}

fn anchor(text: &str) -> Result<&str, CommandResponse> {
    if text.len() == 16 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(text)
    } else {
        Err(invalid("--anchor must be 16 lowercase hex characters"))
    }
}

fn feed(env: &CommandEnv<'_>, call: &CommandCall<'_>) -> Option<CommandResponse> {
    let Some((sub, rest)) = call.args.split_first() else {
        return Some(usage());
    };
    Some(match run(env, sub, rest) {
        Ok(response) | Err(response) => response,
    })
}

fn run(
    env: &CommandEnv<'_>,
    sub: &str,
    rest: &[String],
) -> Result<CommandResponse, CommandResponse> {
    let deps = env.deps;
    let kernel = &deps.kernel;
    Ok(match sub {
        "head" if rest.is_empty() => answer(kernel.run(event_feed::feed_head)),
        "snapshot" if rest.is_empty() => answer(kernel.run(snapshot_feed::snapshot_head)),
        "page" => {
            let flags = Flags::parse(rest, &["--after", "--limit", "--anchor"])?;
            let after = position(flags.required("--after")?)?;
            let limit = flags.limit()?;
            let anchor = flags
                .get("--anchor")
                .map(anchor)
                .transpose()?
                .map(str::to_string);
            answer(
                kernel
                    .run(move |core| event_feed::feed_page(core, after, limit, anchor.as_deref())),
            )
        }
        "recent" => {
            let flags = Flags::parse(rest, &["--epoch", "--head", "--limit"])?;
            let epoch = anchor(flags.required("--epoch")?)
                .map_err(|_| invalid("--epoch must be 16 lowercase hex characters"))?;
            let head: u64 = number(flags.required("--head")?, "--head")?;
            let limit = flags.recent_limit()?;
            // Flags are checked first, so an invalid request does not use the quota.
            let now = kernel.now_ms();
            let allowed = env
                .state
                .recent_limiter
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .allow(&deps.options.project.project_id, now);
            if !allowed {
                return Err(fail(
                    ErrorCode::Rejected,
                    "rate_limited: at most 30 recent reads a minute for this project; wait and retry",
                ));
            }
            let epoch = epoch.to_string();
            answer(kernel.run(move |core| event_feed::recent_page(core, &epoch, head, limit)))
        }
        "waiting" => {
            let flags = Flags::parse(rest, &["--limit", "--after", "--kind"])?;
            let limit = flags.limit()?;
            let after = flags.get("--after").map(str::to_string);
            let kind = flags.get("--kind").map(str::to_string);
            answer(kernel.run(move |core| {
                snapshot_feed::waiting_page(core, after.as_deref(), limit, kind.as_deref())
            }))
        }
        "agents" => {
            let flags = Flags::parse(rest, &["--limit", "--after"])?;
            let limit = flags.limit()?;
            let after = flags.get("--after").map(str::to_string);
            answer(
                kernel.run(move |core| snapshot_feed::agents_page(core, after.as_deref(), limit)),
            )
        }
        "plans" => {
            let flags = Flags::parse(rest, &["--limit", "--after"])?;
            let limit = flags.limit()?;
            let after = flags.get("--after").map(str::to_string);
            answer(kernel.run(move |core| snapshot_feed::plans_page(core, after.as_deref(), limit)))
        }
        "item" if rest.len() == 2 => {
            let (section, key) = (rest[0].clone(), rest[1].clone());
            let found = kernel.run({
                let (section, key) = (section.clone(), key.clone());
                move |core| snapshot_feed::snapshot_item(core, &section, &key)
            });
            answer(
                found.map(
                    |item: Option<Value>| json!({"section": section, "key": key, "item": item}),
                ),
            )
        }
        "hash" if rest.len() == 1 => {
            let request_id = rest[0].clone();
            let found = kernel.run({
                let request_id = request_id.clone();
                move |core| snapshot_feed::content_hash(core, &request_id)
            });
            answer(found.map(|hash| json!({"requestId": request_id, "contentHash": hash})))
        }
        _ => return Err(usage()),
    })
}
