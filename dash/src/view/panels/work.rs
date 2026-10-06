//! The work items panel.
use super::{
    assemble, counter, indent, resolve_cols, state_span, table_header, table_row, Col, Ctx,
    PanelSpec, RowSpec,
};
use crate::view::border::{thumb_range, Tab};
use crate::view::layout::window_of;
use crate::view::lines::{span, Style};
use crate::view::types::{ColorRole, Line, PanelId};

pub fn panel(ctx: &Ctx) -> Vec<Line> {
    let (model, view, theme) = (ctx.model, ctx.view, ctx.theme);
    let cw = ctx.content();
    let rows = &model.work;
    let body0 = ctx.body();
    let header = usize::from(body0 >= 3);
    let cols = resolve_cols(
        cw,
        vec![
            Col::new("id", "WORK ITEM", 14),
            Col::new("state", "STATE", 20),
            Col::new("title", "TITLE", 10).flex(),
        ],
    );
    let capacity = body0.saturating_sub(header);
    let win = window_of(rows.len(), view.selected.work, capacity);
    let mut body: Vec<Line> = Vec::new();
    if header > 0 {
        body.push(indent(&table_header(&cols, theme), ctx.w.saturating_sub(2)));
    }
    for (i, r) in rows[win.start..win.end].iter().enumerate() {
        let fg = Style::color(theme.color(ColorRole::Fg));
        body.push(table_row(
            ctx,
            &cols,
            RowSpec {
                selected: ctx.focused && win.start + i == view.selected.work,
                changed: ctx.highlighted(&format!("w:{}", r.id)),
                dim: false,
                cells: vec![
                    ("id", vec![span(r.work_item_id.as_str(), fg.clone())]),
                    ("state", state_span(ctx, &r.state)),
                    ("title", vec![span(r.title.as_str(), fg)]),
                ],
            },
        ));
    }
    assemble(
        ctx,
        PanelSpec {
            id: PanelId::Work,
            tabs: vec![Tab::new("v1", theme.color(ColorRole::Dim))],
            bottom_left: Vec::new(),
            bottom_right: counter(ctx, view.selected.work, rows.len(), win),
            body,
            thumb: thumb_range(rows.len(), capacity, win.start, capacity),
            thumb_top: header,
        },
    )
}
