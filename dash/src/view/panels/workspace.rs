//! Workspace projects in the dash: the member strip of an integration set, the selected area under the pipeline and
//! the agents panel, and the per-repo commits of a report. None of it is drawn for a project without `[repos]`.
use super::{indent, resolve_cols, Col, Ctx};
use crate::model::{AgentRow, PipelineItem, SetMember, SetRow};
use crate::view::format::cell_width;
use crate::view::lines::{fit_line, plain, span, Style};
use crate::view::types::{ColorRole, Line, Span};

/// The glyph, colour role and word of a member's state (the shared vocabulary: `running` reads building, `merged`
/// reads built, the feed's own word is never shown raw).
pub fn member_state(m: &SetMember, ascii: bool) -> (&'static str, ColorRole, &'static str) {
    let pick = |unicode: &'static str, plain: &'static str| if ascii { plain } else { unicode };
    if m.dropped {
        return (pick("–", "-"), ColorRole::Dim, "dropped");
    }
    match m.state.as_str() {
        "merged" => (pick("○", "o"), ColorRole::Info, "built"),
        "confirmed" => (pick("●", "#"), ColorRole::Ok, "in main"),
        "conflicted" => (pick("▲", "!"), ColorRole::Warn, "conflicted"),
        "failed" => (pick("✕", "x"), ColorRole::Bad, "failed"),
        "discarded" => (pick("–", "-"), ColorRole::Dim, "discarded"),
        _ => (pick("◌", "."), ColorRole::Dim, "building"),
    }
}

fn digits(n: usize) -> usize {
    n.to_string().len()
}

/// The members a strip of `cap` cells shows, as `(start, end)`: all of them when they fit, else the first ones and a
/// trailing `+n`; when the next member is beyond them the strip starts two before it with a leading `+n`.
pub fn strip_window(count: usize, next: Option<usize>, cap: usize) -> (usize, usize) {
    if count <= cap {
        return (0, count);
    }
    for shown in (1..=cap).rev() {
        let start = match next {
            Some(n) if n >= shown => n.saturating_sub(2).min(count - shown),
            _ => 0,
        };
        let end = start + shown;
        let lead = if start > 0 { 1 + digits(start) } else { 0 };
        let trail = if end < count {
            1 + digits(count - end)
        } else {
            0
        };
        if lead + shown + trail <= cap {
            return (start, end);
        }
    }
    (0, 1)
}

/// The STATE cell of a set row: one glyph per member in merge order, the review subject bold.
pub fn strip_spans(ctx: &Ctx, set: &SetRow, cap: usize) -> Vec<Span> {
    let next = set.members.iter().position(|m| m.next);
    let (start, end) = strip_window(set.members.len(), next, cap);
    let dim = || Style::color(ctx.theme.color(ColorRole::Dim));
    let mut out = Vec::new();
    if start > 0 {
        out.push(span(format!("+{start}"), dim()));
    }
    for m in &set.members[start..end] {
        let (glyph, role, _) = member_state(m, ctx.g.ascii);
        let mut style = Style::color(ctx.theme.color(role));
        if m.review_subject {
            style = style.bold();
        }
        out.push(span(glyph, style));
    }
    if end < set.members.len() {
        out.push(span(format!("+{}", set.members.len() - end), dim()));
    }
    out
}

/// A repo name cut in the middle to `width` cells, keeping the start and the suffix.
pub fn cut_middle(name: &str, width: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= width || width < 3 {
        return name.chars().take(width).collect();
    }
    let head = ((width - 1) / 2).saturating_sub(1).max(1);
    let tail = width - 1 - head;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

fn short(sha: &Option<String>) -> Option<String> {
    sha.as_ref()
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(7).collect())
}

/// The long summary of a set (`set.summary.*`).
fn long_summary(set: &SetRow) -> String {
    let count = |state: &str| {
        set.members
            .iter()
            .filter(|m| !m.dropped && m.state == state)
            .count()
    };
    let built = count("merged") + count("confirmed");
    let first = |state: &str| {
        set.members
            .iter()
            .find(|m| !m.dropped && m.state == state)
            .map(|m| m.repo.as_str())
            .unwrap_or("")
    };
    let live = set.live;
    match set.word.as_str() {
        "discarded" => match count("confirmed") {
            0 => "discarded · nothing reached main".to_string(),
            kept => format!("discarded · {kept} of {} stay in main", set.members.len()),
        },
        "conflicted" => format!("conflicted in {}", first("conflicted")),
        "failed" => format!("failed in {}", first("failed")),
        "in main" => format!("in main · all {live} repos"),
        "partly in main" => format!("{} of {live} in main", count("confirmed")),
        "built" => "built · none in main yet".to_string(),
        _ => format!("building · {built} of {live} built"),
    }
}

/// Rows the selected area of `item` wants, borders aside; 0 for an item with nothing to expand.
pub fn area_rows_wanted(item: &PipelineItem) -> usize {
    if let Some(set) = &item.set {
        // divider, title, column header, the members, the gate, a hint line
        return 3 + set.members.len() + usize::from(set.gate.is_some()) + 1;
    }
    if !item.commits.is_empty() {
        return 2;
    }
    0
}

struct Cell(&'static str, Vec<Span>);

fn divider(ctx: &Ctx, title: &str) -> Line {
    let dim = Style::color(ctx.theme.color(ColorRole::Dim));
    let head = format!("{} {title} ", ctx.g.rule.repeat(2));
    let used = cell_width(&head);
    let rest = ctx.content().saturating_sub(used);
    indent(
        &[span(format!("{head}{}", ctx.g.rule.repeat(rest)), dim)],
        ctx.w.saturating_sub(2),
    )
}

fn row_line(ctx: &Ctx, marker: Span, cols: &[Col], cells: Vec<Cell>) -> Line {
    let mut line: Line = vec![marker];
    for (i, c) in cols.iter().enumerate() {
        if i > 0 {
            line.push(plain(" "));
        }
        let value = cells
            .iter()
            .find(|Cell(k, _)| *k == c.key)
            .map(|Cell(_, v)| v.clone())
            .unwrap_or_default();
        line.extend(fit_line(&value, c.width));
    }
    fit_line(&line, ctx.w.saturating_sub(2))
}

/// The selected area of a set: its title, the members in merge order and the gate. `rows` is what the panel can give.
pub fn set_area(ctx: &Ctx, set: &SetRow, rows: usize) -> Vec<Line> {
    let theme = ctx.theme;
    let role = |r: ColorRole| Style::color(theme.color(r));
    let (fg, dim) = (role(ColorRole::Fg), role(ColorRole::Dim));
    let name_cut = 13;
    let any_cut = set
        .members
        .iter()
        .any(|m| m.repo.chars().count() > name_cut);
    let gate_rows = usize::from(set.gate.is_some());
    let reserve_hint = any_cut || set.members.len() + 3 + gate_rows > rows;
    let fixed = 3 + gate_rows + usize::from(reserve_hint);
    let capacity = rows.saturating_sub(fixed).max(1).min(set.members.len());
    let cursor = set
        .members
        .iter()
        .position(|m| m.next)
        .or_else(|| set.members.iter().position(|m| m.review_subject))
        .unwrap_or(0);
    let win = crate::view::layout::window_of(set.members.len(), cursor, capacity);
    let above = win.start;
    let below = set.members.len() - win.end;

    let mut out = vec![divider(ctx, "selected")];
    let mut title = vec![
        span(
            format!("set {} ", set.set_id.chars().take(8).collect::<String>()),
            Style::default().bold(),
        ),
        span(format!("{}  {}", set.branch, long_summary(set)), fg.clone()),
    ];
    if above > 0 {
        title.push(span(format!(" · ▲ {above} above"), dim.clone()));
    }
    if let Some(next) = set.members.iter().find(|m| m.next) {
        title.push(span(format!(" · next: {}", next.repo), fg.clone()));
    }
    out.push(indent(&title, ctx.w.saturating_sub(2)));

    let no_head = set
        .members
        .iter()
        .any(|m| short(&m.head_sha).is_none() && !m.dropped);
    let marks: Vec<String> = set
        .members
        .iter()
        .map(|m| {
            let mut parts: Vec<String> = Vec::new();
            if m.next {
                parts.push("next".into());
            }
            if m.review_subject {
                parts.push("review".into());
            }
            if m.attempt >= 1 {
                parts.push(format!("attempt {}", m.attempt + 1));
            }
            parts.join(" ")
        })
        .collect();
    let state_width = set
        .members
        .iter()
        .map(|m| 2 + member_state(m, ctx.g.ascii).2.chars().count())
        .max()
        .unwrap_or(10);
    let cols = resolve_cols(
        ctx.content(),
        vec![
            Col::new("n", "#", 2).right(),
            Col::new("repo", "REPO", name_cut),
            Col::new("state", "STATE", state_width),
            Col::new("head", "HEAD", if no_head { 11 } else { 7 }),
            Col::new("verify", "VERIFY", 6).flex(),
        ],
    );
    let header: Line = {
        let mut spans = vec![plain(" ")];
        for (i, c) in cols.iter().enumerate() {
            if i > 0 {
                spans.push(plain(" "));
            }
            let title = format!("{:<w$}", c.title, w = c.width);
            let title = if c.right {
                format!("{:>w$}", c.title, w = c.width)
            } else {
                title
            };
            spans.push(span(title, role(ColorRole::Fg).bold()));
        }
        fit_line(&spans, ctx.w.saturating_sub(2))
    };
    out.push(header);

    for (offset, m) in set.members[win.start..win.end].iter().enumerate() {
        let index = win.start + offset;
        let (glyph, state_role, word) = member_state(m, ctx.g.ascii);
        let mut verify_cell = vec![if m.dropped {
            span("outside the gate", dim.clone())
        } else {
            match &m.verify {
                Some(v) => {
                    let r = match v.state.as_str() {
                        "passed" => ColorRole::Ok,
                        "failed" => ColorRole::Bad,
                        _ => ColorRole::Dim,
                    };
                    span(v.state.clone(), role(r))
                }
                None => span("not run", dim.clone()),
            }
        }];
        if !marks[index].is_empty() {
            verify_cell.push(span(
                format!("  {}", marks[index]),
                role(ColorRole::Bright).bold(),
            ));
        }
        let row_dim = m.dropped;
        let name_style = if row_dim { dim.clone() } else { fg.clone() };
        let marker = if m.next {
            span(ctx.g.selected, role(ColorRole::Bright).bold())
        } else {
            plain(" ")
        };
        let cells = vec![
            Cell("n", vec![span((index + 1).to_string(), fg.clone())]),
            Cell(
                "repo",
                vec![span(cut_middle(&m.repo, name_cut), name_style)],
            ),
            Cell(
                "state",
                vec![span(
                    format!("{glyph} {word}"),
                    role(if row_dim { ColorRole::Dim } else { state_role }),
                )],
            ),
            Cell(
                "head",
                vec![match short(&m.head_sha) {
                    Some(sha) => span(sha, if row_dim { dim.clone() } else { fg.clone() }),
                    None if m.dropped => span(if ctx.g.ascii { "-" } else { "–" }, dim.clone()),
                    None => span("no head yet", dim.clone()),
                }],
            ),
            Cell("verify", verify_cell),
        ];
        out.push(row_line(ctx, marker, &cols, cells));
    }

    if reserve_hint {
        let mut parts: Vec<String> = Vec::new();
        if below > 0 {
            parts.push(format!("▼ {below} below"));
        }
        let hinted = set.members[win.start..win.end]
            .iter()
            .find(|m| m.next && m.repo.chars().count() > name_cut)
            .or_else(|| {
                set.members[win.start..win.end]
                    .iter()
                    .find(|m| m.repo.chars().count() > name_cut)
            });
        if let Some(m) = hinted {
            parts.push(format!("{} = {}", cut_middle(&m.repo, name_cut), m.repo));
        }
        out.push(indent(
            &[span(parts.join(" · "), dim.clone())],
            ctx.w.saturating_sub(2),
        ));
    }

    if let Some(gate) = &set.gate {
        let repos = gate.repos.join(", ");
        let (text, r) = match gate.state.as_str() {
            "passed" => (
                "gate passed: every repo's latest run passed".to_string(),
                ColorRole::Ok,
            ),
            "failed" => (format!("gate failed: {repos}"), ColorRole::Bad),
            _ => (format!("gate waiting: {repos}"), ColorRole::Warn),
        };
        out.push(indent(&[span(text, role(r))], ctx.w.saturating_sub(2)));
    }
    out.truncate(rows);
    out
}

/// The selected area of a multi-repo report: its commits, one line.
pub fn report_area(ctx: &Ctx, item: &PipelineItem) -> Vec<Line> {
    let theme = ctx.theme;
    let same_branch = item.commits.windows(2).all(|w| w[0].branch == w[1].branch);
    let mut text = item
        .commits
        .iter()
        .map(|c| {
            let sha: String = c.commit_sha.chars().take(7).collect();
            if same_branch {
                format!("{} {sha}", c.repo)
            } else {
                format!("{} {sha} on {}", c.repo, c.branch)
            }
        })
        .collect::<Vec<_>>()
        .join(" · ");
    if same_branch {
        if let Some(first) = item.commits.first().filter(|c| !c.branch.is_empty()) {
            text.push_str(&format!("  on {}", first.branch));
        }
    }
    let line = indent(
        &[span(
            crate::view::format::truncate(&text, ctx.content()),
            Style::color(theme.color(ColorRole::Fg)),
        )],
        ctx.w.saturating_sub(2),
    );
    vec![divider(ctx, "selected"), line]
}

/// Rows the agents panel's selected area wants for an agent that holds worktrees.
pub fn agent_area_rows_wanted(agent: &AgentRow) -> usize {
    if agent.worktrees.is_empty() {
        return 0;
    }
    2 + agent.worktrees.len() + usize::from(agent.change_folder.is_some())
}

/// The worktrees an agent holds and the folder that holds them.
pub fn agent_area(ctx: &Ctx, agent: &AgentRow, rows: usize) -> Vec<Line> {
    let theme = ctx.theme;
    let (fg, dim) = (
        Style::color(theme.color(ColorRole::Fg)),
        Style::color(theme.color(ColorRole::Dim)),
    );
    let branch = agent
        .worktrees
        .first()
        .map(|w| w.branch.clone())
        .unwrap_or_default();
    let mut out = vec![
        divider(ctx, "selected"),
        indent(
            &[
                span(format!("{} ", agent.agent_id), Style::default().bold()),
                span(format!("worktrees on {branch}"), fg.clone()),
            ],
            ctx.w.saturating_sub(2),
        ),
    ];
    let width = agent
        .worktrees
        .iter()
        .map(|w| w.repo.chars().count())
        .max()
        .unwrap_or(0)
        .max("change folder".len());
    for w in &agent.worktrees {
        out.push(indent(
            &[
                span(format!("{:<width$}  ", w.repo), fg.clone()),
                span(w.path.clone(), dim.clone()),
            ],
            ctx.w.saturating_sub(2),
        ));
    }
    if let Some(folder) = &agent.change_folder {
        out.push(indent(
            &[
                span(format!("{:<width$}  ", "change folder"), fg.clone()),
                span(folder.clone(), dim.clone()),
            ],
            ctx.w.saturating_sub(2),
        ));
    }
    out.truncate(rows);
    out
}
