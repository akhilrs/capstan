//! cstan-release: what scripts/release.sh asks of the commit history.
//!
//!   cstan-release plan --root <dir> [--version X.Y.Z]            the current and next version, one key=value per line
//!   cstan-release section --root <dir> --version X.Y.Z --date D  the CHANGELOG section for that release
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use capstan_release::{
    commits_since, compare_versions, dedupe_commits, last_release_tag, next_version,
    render_changelog_section,
};

fn fail(message: &str) -> ExitCode {
    eprintln!("release: {message}");
    ExitCode::from(1)
}

struct Args {
    root: PathBuf,
    version: Option<String>,
    date: Option<String>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut out = Args { root: PathBuf::from("."), version: None, date: None };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--root" => out.root = PathBuf::from(value("--root")?),
            "--version" => out.version = Some(value("--version")?),
            "--date" => out.date = Some(value("--date")?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(out)
}

fn current_version(root: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(root.join("VERSION"))
        .map_err(|e| format!("cannot read {}/VERSION: {e}", root.display()))?;
    Ok(text.trim().to_string())
}

fn plan(args: &Args) -> Result<(), String> {
    let current = current_version(&args.root)?;
    let last = last_release_tag(&args.root);
    let (commits, ignored) = commits_since(&args.root, last.as_deref())?;
    if !ignored.is_empty() {
        eprintln!(
            "release: warning: ignoring {} merge or non-conforming commit(s):\n  {}",
            ignored.len(),
            ignored.join("\n  ")
        );
    }
    let mut next = next_version(&current, &commits)?;
    if let Some(wanted) = &args.version {
        let well_formed = wanted.split('.').count() == 3
            && wanted.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
        if !well_formed {
            return Err(format!("--version needs X.Y.Z, got '{wanted}'"));
        }
        if compare_versions(wanted, &current)?.is_le() {
            return Err(format!("--version {wanted} must be higher than the current {current}"));
        }
        next = Some(capstan_release::Next { version: wanted.clone(), level: "override" });
    }
    let Some(next) = next else {
        return Err(format!(
            "nothing to release since {}: no feat, fix, perf, revert or breaking commits",
            last.as_deref().unwrap_or("the first commit")
        ));
    };
    let listed = dedupe_commits(&commits).len();
    println!("current={current}");
    println!("next={}", next.version);
    println!("level={}", next.level);
    println!("last_tag={}", last.unwrap_or_default());
    println!("counted={}", commits.len());
    println!("listed={listed}");
    Ok(())
}

fn section(args: &Args) -> Result<(), String> {
    let version = args.version.as_deref().ok_or("section needs --version")?;
    let date = args.date.as_deref().ok_or("section needs --date")?;
    let last = last_release_tag(&args.root);
    let (commits, _) = commits_since(&args.root, last.as_deref())?;
    print!("{}", render_changelog_section(version, date, &commits));
    Ok(())
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = argv.split_first() else {
        return fail("usage: cstan-release plan|section --root <dir> [--version X.Y.Z] [--date D]");
    };
    let result = parse(rest).and_then(|args| match command.as_str() {
        "plan" => plan(&args),
        "section" => section(&args),
        other => Err(format!("unknown command {other}")),
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => fail(&message),
    }
}
