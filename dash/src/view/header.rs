//! The header: title border, health chip, workers meter, link state and the task summary (`view.ts`).
use super::border::{bottom_border, top_border, BorderColors, BottomBorder, Tab, TopBorder};
use super::format::{age, cell_width, task_summary, truncate};
use super::glyphs::Glyphs;
use super::graph::meter;
use super::layout::{MIN_SHARED_SUMMARY, SHARED_TASK_ROWS, WRAP_REASON_ROWS};
use super::lines::{fit_line, line_width, merge_spans, plain, span, Style};
use super::types::{ColorRole, Line, Link, Span, Theme, ViewState};
use crate::model::DashModel;

struct Chip {
    text: String,
    role: ColorRole,
    reason: String,
    reason_role: ColorRole,
}

fn chip(text: String, role: ColorRole, reason: String, reason_role: ColorRole) -> Chip {
    Chip {
        text,
        role,
        reason,
        reason_role,
    }
}

fn health_chip(model: &DashModel, view: &ViewState, g: &Glyphs) -> Chip {
    if view.link == Link::Down || view.link == Link::Toolarge {
        return chip(
            format!("{} NO LINK", g.link_down),
            ColorRole::Bad,
            if view.link == Link::Down {
                "controller not answering, retrying; showing the last good frame"
            } else {
                "status too large for the control socket; showing the last good frame"
            }
            .to_string(),
            ColorRole::Bad,
        );
    }
    let h = &model.header;
    let Some(sup) = &h.supervision else {
        return chip(
            "? UNKNOWN".to_string(),
            ColorRole::Dim,
            String::new(),
            ColorRole::Dim,
        );
    };
    if !sup.enabled {
        return chip(
            format!("{} SUPERVISION OFF", g.ended),
            ColorRole::Dim,
            "[supervision] enabled = false in capstan.toml".to_string(),
            ColorRole::Dim,
        );
    }
    let findings = format!(
        "{} open finding{}",
        sup.open_findings,
        if sup.open_findings == 1 { "" } else { "s" }
    );
    let Some(supervisor) = &sup.supervisor else {
        if h.workers == 0 {
            return chip(
                format!("{} SUPERVISION IDLE", g.idle),
                ColorRole::Dim,
                format!("starts with the next worker; {findings}"),
                ColorRole::Dim,
            );
        }
        return chip(
            format!("{} NO SUPERVISOR", g.idle),
            ColorRole::Warn,
            format!("supervision on, starts with the next worker; {findings}"),
            ColorRole::Dim,
        );
    };
    let check = match &sup.last_check {
        None => "no check yet".to_string(),
        Some(c) => format!(
            "check {} {} ago",
            c.state,
            age(
                Some(c.acked_at.as_deref().unwrap_or(&c.queued_at)),
                view.now_ms
            )
        ),
    };
    chip(
        format!("{} SUPERVISED", g.idle),
        ColorRole::Ok,
        format!(
            "{} {}; {check}; {findings}",
            supervisor.id, supervisor.state
        ),
        ColorRole::Dim,
    )
}

/// One full-width line above the header while the PM has stale mail: a glyph and text, bold, so it reads without colour.
pub fn pm_stale_banner(
    model: &DashModel,
    view: &ViewState,
    theme: &Theme,
    g: &Glyphs,
) -> Vec<Line> {
    let Some(mail) = model.header.pm_mail.as_ref().filter(|m| m.stale) else {
        return Vec::new();
    };
    let minutes = (mail.oldest_age_seconds / 60.0).floor();
    let text = format!(
        " {} PM MAIL STALE: {} message{} pending, oldest {} min; the PM has unread mail",
        g.attention,
        mail.pending,
        if mail.pending == 1 { "" } else { "s" },
        super::panels::num(minutes)
    );
    vec![fit_line(
        &[span(text, Style::color(theme.color(ColorRole::Bad)).bold())],
        view.size.columns as usize,
    )]
}

pub fn header_lines(model: &DashModel, view: &ViewState, theme: &Theme, g: &Glyphs) -> Vec<Line> {
    let w = view.size.columns as usize;
    let rows = view.size.rows as usize;
    let h = &model.header;
    let color = |role: ColorRole| theme.color(role);
    let border = color(ColorRole::BorderHeader);
    let chip = health_chip(model, view, g);
    let mut right_tabs: Vec<Tab> = Vec::new();
    if view.paused {
        right_tabs.push(Tab::new("PAUSED", color(ColorRole::Warn)));
    }
    right_tabs.push(Tab::new(view.clock.as_str(), color(ColorRole::Fg)));
    right_tabs.push(Tab::new(
        format!("- {}s +", view.interval_seconds),
        color(ColorRole::Info),
    ));
    let mut left_tabs = vec![
        Tab::new(h.project_id.as_str(), color(ColorRole::Fg)),
        Tab::new(format!("run {}", h.run_state), color(ColorRole::Fg)),
    ];
    if let Some(pause) = &h.run_pause {
        left_tabs.push(Tab::new(
            format!(
                "PAUSED {}: {}",
                age(Some(&pause.paused_at), view.now_ms),
                pause.reason
            ),
            color(ColorRole::Warn),
        ));
    }
    if let Some(minutes) = h.full_auto_minutes {
        left_tabs.push(Tab::new(
            format!("FULL AUTO {minutes}m"),
            color(ColorRole::Bad),
        ));
    }
    if let Some(grants) = h.grants {
        left_tabs.push(Tab::new(format!("grants {grants}"), color(ColorRole::Warn)));
    }
    let top = top_border(
        w,
        &TopBorder {
            number: None,
            title: " cstan dash ".to_string(),
            left_tabs,
            tabs: right_tabs,
            focused: false,
        },
        g,
        &BorderColors {
            border: border.clone(),
            title: color(ColorRole::Bright),
        },
    );
    let cw = w.saturating_sub(4);
    let meter_cells = if w >= 120 { 12 } else { 8 };
    let link_text = if view.link == Link::Ok {
        format!(
            "{} {}{}",
            g.link_up,
            if w >= 100 { "answering " } else { "" },
            view.link_age
        )
    } else {
        format!("{} {} ago", g.link_down, view.link_age)
    };
    let mut right: Vec<Span> = match h.worker_limit {
        None => vec![span(
            format!("workers {}", h.workers),
            Style::color(color(ColorRole::Fg)),
        )],
        Some(limit) => {
            let mut spans = vec![span("workers ", Style::color(color(ColorRole::Fg)))];
            spans.extend(meter(h.workers, limit, meter_cells, g, theme));
            spans.push(span(
                format!(" {}/{}", h.workers, limit),
                Style::color(color(ColorRole::Fg)),
            ));
            spans
        }
    };
    right.push(plain("   "));
    right.push(span(
        link_text,
        Style::color(color(if view.link == Link::Ok {
            ColorRole::Info
        } else {
            ColorRole::Bad
        })),
    ));
    let right_width = line_width(&right);
    let chip_cells = cell_width(&chip.text);
    let chip_span = span(chip.text.as_str(), Style::color(color(chip.role)).bold());
    let room = cw as i64 - chip_cells as i64 - 2 - right_width as i64 - 1;
    let wrap = cell_width(&chip.reason) as i64 > room && rows >= WRAP_REASON_ROWS;
    let edge_span = || span(g.edge.v, Style::color(border.clone()));
    let content = |line: Vec<Span>| -> Line {
        let mut parts = vec![edge_span(), plain(" ")];
        parts.extend(fit_line(&line, cw));
        parts.push(plain(" "));
        parts.push(edge_span());
        merge_spans(parts)
    };
    let summary = task_summary(h.tasks.as_deref(), cw);
    let summary_role = color(if h.tasks.as_ref().is_some_and(|t| t.is_empty()) {
        ColorRole::Dim
    } else {
        ColorRole::Fg
    });
    let short = rows < SHARED_TASK_ROWS;
    let reason_span = |text: String| -> Span { span(text, Style::color(color(chip.reason_role))) };
    // On a short terminal the task summary shares the last strip row; `avail` is what that row has left after the chip
    // and the right side.
    let shared = |reason: &str, avail: i64| -> Option<Vec<Span>> {
        if !short || summary.is_empty() {
            return None;
        }
        let gap: i64 = if reason.is_empty() { 0 } else { 2 };
        let summary_cells = cell_width(&summary) as i64;
        let summary_width = summary_cells.min((MIN_SHARED_SUMMARY as i64).max(avail.div_euclid(2)));
        if avail - gap - summary_width < 0
            || summary_width < summary_cells.min(MIN_SHARED_SUMMARY as i64)
        {
            return None;
        }
        let reason_text = truncate(reason, (avail - gap - summary_width) as usize);
        let reason_cells = cell_width(&reason_text) as i64;
        let summary_room = avail - reason_cells - if reason_cells == 0 { 0 } else { 2 };
        let summary_text = truncate(&summary, summary_room.max(0) as usize);
        Some(vec![
            reason_span(reason_text),
            plain(if reason_cells == 0 { "" } else { "  " }),
            span(summary_text, Style::color(summary_role.clone())),
        ])
    };
    let wrapped_share = if wrap {
        shared(&chip.reason, cw as i64 - 2)
    } else {
        None
    };
    let flat_share = if wrap || chip.reason.is_empty() {
        None
    } else {
        shared(&chip.reason, room.max(0))
    };
    let strip: Vec<Line> = if wrap {
        let second = wrapped_share.clone().unwrap_or_else(|| {
            vec![span(
                truncate(&chip.reason, cw.saturating_sub(2)),
                Style::color(color(chip.reason_role)),
            )]
        });
        let mut first = vec![
            chip_span,
            plain(" ".repeat(cw.saturating_sub(chip_cells + right_width).max(1))),
        ];
        first.extend(right.iter().cloned());
        let mut second_line = vec![plain("  ")];
        second_line.extend(second);
        vec![content(first), content(second_line)]
    } else {
        let middle = flat_share.clone().unwrap_or_else(|| {
            vec![span(
                truncate(&chip.reason, room.max(0) as usize),
                Style::color(color(chip.reason_role)),
            )]
        });
        let pad = (cw as i64
            - chip_cells as i64
            - if chip.reason.is_empty() { 0 } else { 2 }
            - line_width(&middle) as i64
            - right_width as i64)
            .max(1) as usize;
        let mut line = vec![
            chip_span,
            plain(if chip.reason.is_empty() { "" } else { "  " }),
        ];
        line.extend(middle);
        line.push(plain(" ".repeat(pad)));
        line.extend(right.iter().cloned());
        vec![content(line)]
    };
    let shared_row = wrapped_share.is_some() || flat_share.is_some();
    let tasks: Vec<Line> = if summary.is_empty() || shared_row {
        Vec::new()
    } else {
        vec![content(vec![span(
            summary.as_str(),
            Style::color(summary_role.clone()),
        )])]
    };
    let mut out = vec![top];
    out.extend(strip);
    out.extend(tasks);
    out.push(bottom_border(
        w,
        &BottomBorder {
            focused: false,
            ..BottomBorder::default()
        },
        g,
        &border,
    ));
    out
}
