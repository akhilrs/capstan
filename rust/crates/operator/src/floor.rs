//! The approval floor: what an auto-approval window never answers. `check_floor` reads a Bash command line, a Write path or a
//! restart request and says whether a configured `operator.approval_floor` entry matches it.
//!
//! The matcher is pure, lexical and never panics. It is not a sandbox: a part it cannot parse is `Unchecked`, and callers
//! approve `Clear` and `Unchecked` alike in a window (logging the reason). Floor wins: every segment and every visible
//! word is scanned even when another part cannot be parsed, so one visible match gives `Floor`. Paths are normalised
//! lexically; symlinks are not followed.

use std::collections::HashSet;

use crate::policy::words;

pub const PUSH_REWRITE_REFSPEC: &str = "@push-rewrite-refspec";
pub const DELETE_OUTSIDE_PROJECT: &str = "@delete-outside-project";
pub const WRITE_OUTSIDE_PROJECT: &str = "@write-outside-project";
pub const CONTROLLER_RESTART: &str = "@controller-restart";

/// Quoted words past this many are not re-tokenised; the rest of the command is Unchecked.
const MAX_QUOTED_WORDS: usize = 16;
/// Characters of one quoted word that are re-tokenised.
const MAX_QUOTED_CHARS: usize = 1024;
const TOO_LONG: &str = "too long to scan";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FloorAction {
    Command { text: String, cwd: String },
    Restart,
    Write { path: String, cwd: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloorContext {
    pub entries: Vec<String>,
    /// The project root for the Operator, the worktree for a worker.
    pub own_root: String,
    /// The project root and every Capstan worktree.
    pub protected_roots: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FloorVerdict {
    Clear,
    Floor { entry: String, detail: String },
    Unchecked { reason: String },
}

pub fn check_floor(action: &FloorAction, ctx: &FloorContext) -> FloorVerdict {
    let mut checker = Checker::new(ctx);
    match action {
        FloorAction::Restart => {
            checker.flag(CONTROLLER_RESTART, "restart of the controller".to_string());
        }
        FloorAction::Write { path, cwd } => {
            let cwd = normalize(cwd, None);
            if checker.outside(path, cwd.as_ref(), false) {
                checker.flag(
                    WRITE_OUTSIDE_PROJECT,
                    format!("write to {path} outside the project"),
                );
            }
        }
        FloorAction::Command { text, cwd } => {
            let cwd = normalize(cwd, None);
            let segs = lex(text, true);
            checker.segments(&segs, cwd);
        }
    }
    if let Some((entry, detail)) = checker.hit {
        FloorVerdict::Floor { entry, detail }
    } else if let Some(reason) = checker.unchecked {
        FloorVerdict::Unchecked { reason }
    } else {
        FloorVerdict::Clear
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Paths

type Path = Vec<String>;

fn normalize(raw: &str, cwd: Option<&Path>) -> Option<Path> {
    let mut out: Path = if raw.starts_with('/') {
        Vec::new()
    } else {
        cwd?.clone()
    };
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other.to_string()),
        }
    }
    Some(out)
}

fn has_prefix(path: &[String], prefix: &[String]) -> bool {
    path.len() >= prefix.len() && path[..prefix.len()] == *prefix
}

fn has_glob(part: &str) -> bool {
    part.contains(['*', '?', '['])
}

/// The directory before the first part holding a glob character (the whole path without one).
fn glob_dir(raw: &str) -> String {
    let parts: Vec<&str> = raw.split('/').take_while(|part| !has_glob(part)).collect();
    let joined = parts.join("/");
    if raw.starts_with('/') && !joined.starts_with('/') {
        format!("/{joined}")
    } else {
        joined
    }
}

fn basename(token: &str) -> &str {
    let trimmed = token.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(slash) => &trimmed[slash + 1..],
        None => trimmed,
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Lexer

struct Word {
    text: String,
    /// Re-tokenise this quoted word that holds whitespace (sh -c, env -S, ssh).
    splice: bool,
}

struct Seg {
    words: Vec<Word>,
    /// Targets of output redirections that name a file.
    writes: Vec<String>,
    unchecked: Option<String>,
    piped: bool,
}

impl Seg {
    fn new(piped: bool) -> Seg {
        Seg {
            words: Vec::new(),
            writes: Vec::new(),
            unchecked: None,
            piped,
        }
    }

    fn is_empty(&self) -> bool {
        self.words.is_empty() && self.writes.is_empty() && self.unchecked.is_none()
    }
}

#[derive(Clone, Copy)]
enum Pending {
    Read,
    Write,
    /// `>&`: a file unless the target is a descriptor number or `-`.
    Dup,
}

struct Lexer {
    segs: Vec<Seg>,
    cur: Seg,
    buf: String,
    started: bool,
    quoted: bool,
    spaced: bool,
    pending: Option<Pending>,
    top: bool,
    quoted_words: usize,
}

impl Lexer {
    fn mark(&mut self, why: &str) {
        if self.cur.unchecked.is_none() {
            self.cur.unchecked = Some(why.to_string());
        }
    }

    fn quoted_char(&mut self, c: char) {
        self.buf.push(c);
        if c.is_whitespace() {
            self.spaced = true;
        }
        if c == '\n' {
            self.mark("newline");
        }
    }

    fn end_word(&mut self) {
        if !self.started {
            return;
        }
        let text = std::mem::take(&mut self.buf);
        let (quoted, spaced) = (self.quoted, self.spaced);
        self.started = false;
        self.quoted = false;
        self.spaced = false;
        let mut splice = false;
        if self.top && quoted {
            self.quoted_words += 1;
            if self.quoted_words > MAX_QUOTED_WORDS {
                self.mark(TOO_LONG);
            } else if spaced {
                splice = true;
                if text.chars().count() > MAX_QUOTED_CHARS {
                    self.mark(TOO_LONG);
                }
            }
        }
        match self.pending.take() {
            Some(Pending::Read) => {}
            Some(Pending::Dup) if text == "-" || text.chars().all(|c| c.is_ascii_digit()) => {}
            Some(Pending::Dup) | Some(Pending::Write) => self.cur.writes.push(text),
            None => self.cur.words.push(Word { text, splice }),
        }
    }

    fn separate(&mut self, piped: bool) {
        self.end_word();
        self.pending = None;
        let old = std::mem::replace(&mut self.cur, Seg::new(piped));
        if !old.is_empty() {
            self.segs.push(old);
        }
    }

    /// A descriptor number just before a redirection (`2>`) belongs to it, not to the command.
    fn drop_fd(&mut self) {
        if self.started
            && !self.quoted
            && !self.buf.is_empty()
            && self.buf.chars().all(|c| c.is_ascii_digit())
        {
            self.buf.clear();
            self.started = false;
        } else {
            self.end_word();
        }
    }

    fn run(mut self, chars: &[char]) -> Vec<Seg> {
        let n = chars.len();
        let at = |i: usize| chars.get(i).copied();
        let mut i = 0;
        while i < n {
            let c = chars[i];
            i += 1;
            match c {
                '\'' => {
                    self.started = true;
                    self.quoted = true;
                    loop {
                        let Some(d) = at(i) else {
                            self.mark("unbalanced quote");
                            break;
                        };
                        i += 1;
                        if d == '\'' {
                            break;
                        }
                        self.quoted_char(d);
                    }
                }
                '"' => {
                    self.started = true;
                    self.quoted = true;
                    loop {
                        let Some(d) = at(i) else {
                            self.mark("unbalanced quote");
                            break;
                        };
                        i += 1;
                        match d {
                            '"' => break,
                            '\\' => match at(i) {
                                Some(e) => {
                                    i += 1;
                                    if !matches!(e, '"' | '\\' | '$' | '`') {
                                        self.quoted_char('\\');
                                    }
                                    self.quoted_char(e);
                                }
                                None => {
                                    self.quoted_char('\\');
                                    self.mark("unbalanced quote");
                                    break;
                                }
                            },
                            '$' if at(i) == Some('(') => {
                                self.mark("command substitution");
                                self.quoted_char(d);
                            }
                            '`' => {
                                self.mark("command substitution");
                                self.quoted_char(d);
                            }
                            _ => self.quoted_char(d),
                        }
                    }
                }
                '\\' => {
                    self.mark("backslash");
                    self.started = true;
                    match at(i) {
                        Some('\n') => {
                            i += 1;
                            self.mark("newline");
                        }
                        Some(e) => {
                            i += 1;
                            self.buf.push(e);
                        }
                        None => {}
                    }
                }
                '$' => {
                    if at(i) == Some('(') {
                        self.mark("command substitution");
                    }
                    self.started = true;
                    self.buf.push(c);
                }
                '`' => {
                    self.mark("command substitution");
                    self.started = true;
                    self.buf.push(c);
                }
                '<' | '>' => {
                    if at(i) == Some('(') {
                        self.mark("process substitution");
                        self.end_word();
                        i += 1;
                    } else if c == '<' && at(i) == Some('<') {
                        self.mark("here document");
                        while at(i) == Some('<') {
                            i += 1;
                        }
                        self.end_word();
                    } else {
                        self.drop_fd();
                        if c == '>' {
                            if at(i) == Some('>') {
                                i += 1;
                            }
                            if at(i) == Some('|') {
                                i += 1;
                            }
                            if at(i) == Some('&') {
                                i += 1;
                                self.pending = Some(Pending::Dup);
                            } else {
                                self.pending = Some(Pending::Write);
                            }
                        } else {
                            if at(i) == Some('&') {
                                i += 1;
                            }
                            self.pending = Some(Pending::Read);
                        }
                    }
                }
                '&' => {
                    if at(i) == Some('>') {
                        self.end_word();
                        i += 1;
                        if at(i) == Some('>') {
                            i += 1;
                        }
                        self.pending = Some(Pending::Write);
                    } else {
                        if at(i) == Some('&') {
                            i += 1;
                        }
                        self.separate(false);
                    }
                }
                '|' => {
                    if at(i) == Some('|') {
                        i += 1;
                        self.separate(false);
                    } else {
                        if at(i) == Some('&') {
                            i += 1;
                        }
                        self.separate(true);
                    }
                }
                ';' => self.separate(false),
                '\n' => {
                    self.mark("newline");
                    self.separate(false);
                    self.mark("newline");
                }
                c if c.is_whitespace() => self.end_word(),
                _ => {
                    self.started = true;
                    self.buf.push(c);
                }
            }
        }
        self.end_word();
        if !self.cur.is_empty() {
            let last = std::mem::replace(&mut self.cur, Seg::new(false));
            self.segs.push(last);
        }
        self.segs
    }
}

fn lex(text: &str, top: bool) -> Vec<Seg> {
    let chars: Vec<char> = text.chars().collect();
    Lexer {
        segs: Vec::new(),
        cur: Seg::new(false),
        buf: String::new(),
        started: false,
        quoted: false,
        spaced: false,
        pending: None,
        top,
        quoted_words: 0,
    }
    .run(&chars)
}

// ---------------------------------------------------------------------------------------------------------------------
// Matcher

/// Programs the built-in predicates look at.
const BUILTIN_PROGRAMS: [&str; 23] = [
    "git", "rm", "rmdir", "find", "shred", "unlink", "mv", "cp", "tee", "dd", "truncate", "ln",
    "chmod", "chown", "chgrp", "install", "rsync", "touch", "mkdir", "sed", "perl", "cstan",
    "xargs",
];
const SHELLS: [&str; 9] = [
    "sh", "bash", "zsh", "dash", "ksh", "csh", "tcsh", "fish", "ash",
];
const KEYWORDS: [&str; 17] = [
    "for", "while", "until", "if", "case", "select", "function", "do", "then", "else", "elif",
    "fi", "done", "esac", "in", "{", "!",
];
const EXEC_FLAGS: [&str; 4] = ["-exec", "-execdir", "-ok", "-okdir"];

struct Checker<'a> {
    ctx: &'a FloorContext,
    own: Path,
    protected: Vec<Path>,
    programs: HashSet<String>,
    hit: Option<(String, String)>,
    unchecked: Option<String>,
    depth: usize,
}

impl<'a> Checker<'a> {
    fn new(ctx: &'a FloorContext) -> Checker<'a> {
        let mut programs: HashSet<String> =
            BUILTIN_PROGRAMS.iter().map(|p| (*p).to_string()).collect();
        for entry in &ctx.entries {
            if entry.starts_with('@') {
                continue;
            }
            if let Some(first) = words(entry).first() {
                programs.insert(basename(first).to_string());
            }
        }
        Checker {
            ctx,
            own: normalize(&ctx.own_root, None).unwrap_or_default(),
            protected: ctx
                .protected_roots
                .iter()
                .filter_map(|root| normalize(root, None))
                .collect(),
            programs,
            hit: None,
            unchecked: None,
            depth: 0,
        }
    }

    fn note(&mut self, reason: &str) {
        if self.depth == 0 && self.unchecked.is_none() {
            self.unchecked = Some(reason.to_string());
        }
    }

    /// Record a match when the predicate (or entry) is configured.
    fn flag(&mut self, entry: &str, detail: String) {
        if self.hit.is_none() && self.ctx.entries.iter().any(|e| e == entry) {
            self.hit = Some((entry.to_string(), detail));
        }
    }

    /// At or below own_root and not at or below another protected root (own_root and its ancestors excepted).
    /// `strict` also refuses own_root itself.
    fn inside(&self, path: &[String], strict: bool) -> bool {
        if !has_prefix(path, &self.own) || (strict && path.len() == self.own.len()) {
            return false;
        }
        !self
            .protected
            .iter()
            .any(|root| has_prefix(path, root) && !has_prefix(&self.own, root))
    }

    fn outside(&self, raw: &str, cwd: Option<&Path>, strict: bool) -> bool {
        // An expansion the shell makes at run time (a variable, a substitution, a brace list, a leading ~) is unknown.
        if raw.starts_with('~') || (raw != "{}" && raw.contains(['$', '`', '{'])) {
            return true;
        }
        match normalize(&glob_dir(raw), cwd) {
            Some(path) => !self.inside(&path, strict),
            None => true,
        }
    }

    fn cwd_outside(&self, cwd: Option<&Path>) -> bool {
        match cwd {
            Some(path) => !self.inside(path, false),
            None => true,
        }
    }

    fn segments(&mut self, segs: &[Seg], mut cwd: Option<Path>) {
        for seg in segs {
            if let Some(reason) = &seg.unchecked {
                self.note(reason);
            }
            self.segment(seg, cwd.as_ref());
            if self.hit.is_some() {
                return;
            }
            cwd = after_cd(seg, cwd);
        }
    }

    fn segment(&mut self, seg: &Seg, cwd: Option<&Path>) {
        for target in &seg.writes {
            if target != "/dev/null" && self.outside(target, cwd, false) {
                self.flag(
                    WRITE_OUTSIDE_PROJECT,
                    format!("redirect to {target} outside the project"),
                );
            }
        }
        let ws: Vec<&str> = seg.words.iter().map(|w| w.text.as_str()).collect();
        if let Some(index) = self.head(&ws, seg.piped) {
            if self.programs.contains(basename(ws[index])) {
                self.check_run(&ws[index..], cwd);
            }
        }
        // The visible scan: matching restarts at each word naming a floor program. A run ends at the next such word
        // (a find run takes one `-exec` target along), so the work stays linear.
        let starts: Vec<usize> = (0..ws.len())
            .filter(|&i| self.programs.contains(basename(ws[i])))
            .collect();
        for (k, &start) in starts.iter().enumerate() {
            let mut end = starts.get(k + 1).copied().unwrap_or(ws.len());
            if end < ws.len() && basename(ws[start]) == "find" && EXEC_FLAGS.contains(&ws[end - 1])
            {
                end += 1;
            }
            self.check_run(&ws[start..end], cwd);
        }
        if self.depth == 0 {
            for word in seg.words.iter().filter(|w| w.splice) {
                let text: String = word.text.chars().take(MAX_QUOTED_CHARS).collect();
                let inner = lex(&text, false);
                self.depth += 1;
                self.segments(&inner, cwd.cloned());
                self.depth -= 1;
            }
        }
    }

    /// Skip assignments and wrappers; note why the segment cannot be read. Returns the program's index.
    fn head(&mut self, ws: &[&str], piped: bool) -> Option<usize> {
        let mut i = 0;
        loop {
            while ws.get(i).is_some_and(|w| is_assignment(w)) {
                i += 1;
            }
            let word = *ws.get(i)?;
            if KEYWORDS.contains(&word) || word.starts_with('(') {
                self.note("loop or compound command");
                return None;
            }
            match skip_wrapper(ws, i) {
                Some(Ok(next)) => i = next,
                Some(Err(())) => {
                    self.note("wrapper option not understood");
                    return None;
                }
                None => break,
            }
        }
        let word = ws[i];
        let program = basename(word);
        let args = &ws[i + 1..];
        if SHELLS.contains(&program) {
            if piped {
                self.note("pipe into a shell");
            }
            if !args.is_empty() {
                self.note("shell with a script");
            }
        } else if matches!(program, "eval" | "exec" | "source" | "." | "xargs") {
            self.note("runs a command it builds");
        } else if is_interpreter(program) && args.iter().any(|a| inline_script_flag(a)) {
            self.note("inline script");
        }
        Some(i)
    }

    fn check_run(&mut self, run: &[&str], cwd: Option<&Path>) {
        if self.hit.is_some() || run.is_empty() {
            return;
        }
        let program = basename(run[0]);
        let mut start = 1;
        let mut gcwd: Option<Path> = cwd.cloned();
        if program == "git" {
            match self.git_globals(&run[1..], cwd) {
                Ok((used, dir)) => {
                    start = 1 + used;
                    gcwd = dir;
                }
                Err(reason) => self.note(&reason),
            }
        }
        let toks = &run[start..];
        let forms = forms_of(toks);
        for entry in &self.ctx.entries {
            if entry.starts_with('@') {
                continue;
            }
            let ew = words(entry);
            let Some((first, rest)) = ew.split_first() else {
                continue;
            };
            if basename(first) == program && rest.iter().all(|w| forms.contains(*w)) {
                self.hit = Some((entry.clone(), format!("{program} matches the floor entry")));
                return;
            }
        }
        match program {
            "git" => self.check_git(toks, &forms, gcwd.as_ref()),
            "rm" => {
                if ["-r", "-R", "--recursive"]
                    .iter()
                    .any(|f| forms.contains(*f))
                {
                    self.delete_paths("recursive rm", &positionals(toks), cwd, true);
                }
            }
            "rmdir" | "shred" | "unlink" => {
                self.delete_paths(program, &positionals(toks), cwd, true)
            }
            "find" => self.check_find(toks, cwd),
            "mv" | "tee" | "touch" | "mkdir" | "truncate" | "chmod" | "chown" | "chgrp" => {
                self.write_paths(program, &positionals(toks), cwd);
            }
            "cp" | "install" | "rsync" | "ln" => {
                let mut targets: Vec<&str> =
                    positionals(toks).last().copied().into_iter().collect();
                for (i, t) in toks.iter().enumerate() {
                    if *t == "-t" || *t == "--target-directory" {
                        targets.extend(toks.get(i + 1).copied());
                    } else if let Some(v) = t.strip_prefix("--target-directory=") {
                        targets.push(v);
                    }
                }
                self.write_paths(program, &targets, cwd);
            }
            "dd" => {
                let targets: Vec<&str> =
                    toks.iter().filter_map(|t| t.strip_prefix("of=")).collect();
                self.write_paths("dd", &targets, cwd);
            }
            "sed" => {
                if toks
                    .iter()
                    .any(|t| sed_in_place(t) || *t == "--in-place" || t.starts_with("--in-place="))
                {
                    let mut pos = positionals(toks);
                    if !toks.iter().any(|t| *t == "-e" || *t == "-f") && !pos.is_empty() {
                        pos.remove(0);
                    }
                    self.write_paths("sed -i", &pos, cwd);
                }
            }
            "perl" => {
                if toks
                    .iter()
                    .any(|t| t.starts_with("-i") || (is_letter_bundle(t) && t.contains('i')))
                {
                    let mut pos: Vec<&str> = Vec::new();
                    let mut skip = false;
                    for t in toks {
                        if skip {
                            skip = false;
                        } else if t.starts_with('-') {
                            skip = !t.starts_with("--") && t.ends_with('e');
                        } else {
                            pos.push(t);
                        }
                    }
                    self.write_paths("perl -i", &pos, cwd);
                }
            }
            "cstan" => {
                let pos = positionals(toks);
                let restart = matches!(pos.as_slice(), ["restart", ..] | ["stop", ..])
                    || matches!(pos.as_slice(), ["daemon", "stop" | "restart", ..]);
                if restart {
                    self.flag(CONTROLLER_RESTART, "cstan restart or stop".to_string());
                }
            }
            _ => {}
        }
    }

    fn delete_paths(&mut self, what: &str, paths: &[&str], cwd: Option<&Path>, strict: bool) {
        for path in paths {
            if self.outside(path, cwd, strict) {
                self.flag(
                    DELETE_OUTSIDE_PROJECT,
                    format!("{what} of {path} outside the project"),
                );
            }
        }
    }

    fn write_paths(&mut self, what: &str, paths: &[&str], cwd: Option<&Path>) {
        for path in paths {
            if self.outside(path, cwd, false) {
                self.flag(
                    WRITE_OUTSIDE_PROJECT,
                    format!("{what} of {path} outside the project"),
                );
            }
        }
    }

    fn check_find(&mut self, toks: &[&str], cwd: Option<&Path>) {
        let mut rest = toks;
        while let [first, tail @ ..] = rest {
            if matches!(*first, "-H" | "-L" | "-P") {
                rest = tail;
            } else {
                break;
            }
        }
        let mut paths: Vec<&str> = rest
            .iter()
            .take_while(|t| !t.starts_with('-') && **t != "(" && **t != "!")
            .copied()
            .collect();
        if paths.is_empty() {
            paths.push(".");
        }
        let deleting = rest.contains(&"-delete")
            || rest.windows(2).any(|w| {
                EXEC_FLAGS.contains(&w[0])
                    && matches!(basename(w[1]), "rm" | "shred" | "unlink" | "rmdir")
            });
        if deleting {
            self.delete_paths("find deleting", &paths, cwd, false);
        }
    }

    /// The git global options before the subcommand: how many tokens they take and the directory the subcommand runs
    /// in. An error is a reason the command cannot be read.
    fn git_globals(
        &mut self,
        toks: &[&str],
        cwd: Option<&Path>,
    ) -> Result<(usize, Option<Path>), String> {
        let mut dir: Option<Path> = cwd.cloned();
        let mut i = 0;
        while let Some(&tok) = toks.get(i) {
            if !tok.starts_with('-') {
                break;
            }
            let (name, inline) = match tok.split_once('=') {
                Some((n, v)) if tok.starts_with("--") => (n, Some(v)),
                _ => (tok, None),
            };
            i += 1;
            match name {
                "-C" => {
                    let value = *toks.get(i).ok_or("git -C without a directory")?;
                    i += 1;
                    dir = if value.starts_with('$') || value.starts_with('~') {
                        None
                    } else {
                        normalize(value, dir.as_ref())
                    };
                }
                "-c" => {
                    let value = *toks.get(i).ok_or("git -c without a setting")?;
                    i += 1;
                    if value.to_lowercase().starts_with("alias.") {
                        return Err("git alias".to_string());
                    }
                }
                "--git-dir" | "--work-tree" | "--namespace" => {
                    let value = match inline {
                        Some(v) => v,
                        None => {
                            let v = *toks.get(i).ok_or("git option without a value")?;
                            i += 1;
                            v
                        }
                    };
                    if name != "--namespace" && self.outside(value, dir.as_ref(), false) {
                        self.flag(
                            DELETE_OUTSIDE_PROJECT,
                            format!("git {name} {value} outside the project"),
                        );
                    }
                }
                "--exec-path"
                | "--no-pager"
                | "-p"
                | "-P"
                | "--paginate"
                | "--bare"
                | "--no-replace-objects"
                | "--literal-pathspecs"
                | "--no-optional-locks" => {}
                _ => return Err("unknown git option".to_string()),
            }
        }
        Ok((i, dir))
    }

    fn check_git(&mut self, toks: &[&str], forms: &HashSet<String>, gcwd: Option<&Path>) {
        let Some(at) = toks.iter().position(|t| !t.starts_with('-')) else {
            return;
        };
        let rest = &toks[at + 1..];
        match toks[at] {
            "push" => {
                for t in rest.iter().filter(|t| !t.starts_with('-')) {
                    if t.starts_with('+') || t.starts_with(':') || t.ends_with(':') {
                        self.flag(PUSH_REWRITE_REFSPEC, format!("git push of the refspec {t}"));
                    }
                }
            }
            "worktree" if rest.first() == Some(&"remove") => {
                self.flag(DELETE_OUTSIDE_PROJECT, "git worktree remove".to_string());
            }
            sub @ ("reset" | "checkout" | "restore" | "clean" | "stash") => {
                let destructive = match sub {
                    "reset" => forms.contains("--hard"),
                    "stash" => matches!(rest.first(), Some(&"drop") | Some(&"clear")),
                    _ => true,
                };
                if destructive && self.cwd_outside(gcwd) {
                    self.flag(
                        DELETE_OUTSIDE_PROJECT,
                        format!("git {sub} outside the project"),
                    );
                }
                if sub == "clean" {
                    self.delete_paths("git clean", &positionals(rest), gcwd, false);
                }
            }
            _ => {}
        }
    }
}

/// `cd` and `pushd` with one literal path move the cwd for the following segments; any other cd, and popd, lose it.
fn after_cd(seg: &Seg, cwd: Option<Path>) -> Option<Path> {
    match seg.words.first().map(|w| w.text.as_str()) {
        Some("cd") | Some("pushd") => match seg.words.as_slice() {
            [_, arg] if is_literal_dir(&arg.text) => normalize(&arg.text, cwd.as_ref()),
            _ => None,
        },
        Some("popd") => None,
        _ => cwd,
    }
}

fn is_literal_dir(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('-')
        && !text.starts_with('~')
        && !text.contains(['$', '*', '?', '[', '`', '{'])
}

fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            let mut chars = name.chars();
            chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

/// `-i`, `-i.bak`, `-Ei`, `-ni` and `-Ei.bak`: an in-place flag among the leading letters of a single-dash token.
fn sed_in_place(token: &str) -> bool {
    match token.strip_prefix('-') {
        Some(rest) if !rest.starts_with('-') => rest
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .any(|c| c == 'i'),
        _ => false,
    }
}

fn is_interpreter(program: &str) -> bool {
    ["python", "perl", "ruby", "node"]
        .iter()
        .any(|name| program.starts_with(name))
}

fn inline_script_flag(arg: &str) -> bool {
    if arg == "--eval" || arg == "--print" {
        return true;
    }
    is_letter_bundle(arg) && (arg.ends_with('c') || arg.ends_with('e'))
}

/// `-abc`: a single dash and letters only.
fn is_letter_bundle(token: &str) -> bool {
    match token.strip_prefix('-') {
        Some(rest) => !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphabetic()),
        None => false,
    }
}

/// The tokens that are not options (everything after `--` counts).
fn positionals<'a>(toks: &[&'a str]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut literal = false;
    for tok in toks {
        if literal || !tok.starts_with('-') {
            out.push(*tok);
        } else if *tok == "--" {
            literal = true;
        }
    }
    out
}

/// Each token; `--name=value` also as `--name`; a single-dash letter bundle also as its letters.
fn forms_of(toks: &[&str]) -> HashSet<String> {
    let mut forms = HashSet::new();
    for tok in toks {
        forms.insert((*tok).to_string());
        if tok.starts_with("--") {
            if let Some((name, _)) = tok.split_once('=') {
                forms.insert(name.to_string());
            }
        } else if is_letter_bundle(tok) && tok.len() > 2 {
            for letter in tok[1..].chars() {
                forms.insert(format!("-{letter}"));
            }
        }
    }
    forms
}

/// The wrapper at `ws[i]`: `None` when it is not one, else the index after its options or `Err` for an option it does
/// not know.
fn skip_wrapper(ws: &[&str], i: usize) -> Option<Result<usize, ()>> {
    let name = basename(ws[i]);
    let mut j = i + 1;
    let option = |j: usize| ws.get(j).filter(|t| t.starts_with('-')).copied();
    match name {
        "sudo" => {
            while let Some(tok) = option(j) {
                match tok {
                    "-u" | "-g" => j += 2,
                    _ if is_letter_bundle(tok) && tok[1..].chars().all(|c| "EHnis".contains(c)) => {
                        j += 1
                    }
                    _ => return Some(Err(())),
                }
            }
        }
        "env" => {
            while let Some(tok) = ws.get(j) {
                match *tok {
                    "-i" => j += 1,
                    "-u" => j += 2,
                    t if t.starts_with('-') => return Some(Err(())),
                    t if is_assignment(t) => j += 1,
                    _ => break,
                }
            }
        }
        "nice" => {
            while let Some(tok) = option(j) {
                match tok {
                    "-n" => j += 2,
                    _ if tok.len() > 1 && tok[1..].chars().all(|c| c.is_ascii_digit()) => j += 1,
                    _ => return Some(Err(())),
                }
            }
        }
        "timeout" => {
            while let Some(tok) = option(j) {
                match tok {
                    "-s" | "-k" => j += 2,
                    "--foreground" => j += 1,
                    _ => return Some(Err(())),
                }
            }
            j += 1;
        }
        "nohup" | "command" | "time" => {
            if option(j).is_some() {
                return Some(Err(()));
            }
        }
        _ => return None,
    }
    Some(Ok(j.min(ws.len())))
}
