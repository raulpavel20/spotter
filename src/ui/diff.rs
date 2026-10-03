//! The full-screen diff view: the whole target as one scroll.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::files::{path_label, suffix};
use super::header::draw_footer_with;
use super::text::{self, left_right, styled_segs};
use super::theme;
use super::timeline::target_label;
use crate::app::{App, Glyph};
use crate::diffview::{DiffView, Row, RowKind};
use crate::model::{FileChange, LineKind, MODE_GITLINK, Stats, Status};

const HINTS: &[(&str, &str)] = &[
    ("]/[", "hunk"),
    ("}/{", "file"),
    ("space", "viewed+next"),
    ("n/p", "commit"),
    ("e", "edit"),
    ("esc", "back"),
];

pub fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let [head, rule1, body, rule2, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let tab = app.tab_width;
    {
        let d = app.diff.as_mut().expect("diff open");
        d.viewport = body.height.max(1) as usize;
        d.clamp_scroll();
    }
    let app = &*app;
    let d = app.diff.as_ref().expect("diff open");

    // Header: target · file (i/n), and the viewed count.
    let n = d.files.len();
    let cur = d.current_file().unwrap_or(0);
    let viewed = app.viewed_count(&d.files);
    let mut left = vec![
        Span::raw(" "),
        Span::styled(target_label(app, &d.target, true), theme::bold()),
    ];
    if let Some(file) = d.files.get(cur) {
        left.push(Span::styled(" · ", theme::dim()));
        left.push(Span::raw(text::display(&file.path)));
        left.push(Span::styled(format!(" ({}/{n})", cur + 1), theme::dim()));
    }
    let g = Glyph::of(viewed, n);
    let right = vec![
        Span::styled(g.symbol().to_owned(), theme::glyph(g)),
        Span::raw(format!(" {viewed}/{n} viewed ")),
    ];
    f.render_widget(
        Paragraph::new(left_right(left, right, head.width as usize)),
        head,
    );

    let rule = "─".repeat(area.width as usize);
    f.render_widget(
        Paragraph::new(Line::styled(rule.clone(), theme::dim())),
        rule1,
    );
    match &d.error {
        Some(e) => f.render_widget(
            Paragraph::new(Line::styled(
                text::truncate_end(&format!("✕ {e}"), area.width as usize),
                theme::error(),
            )),
            rule2,
        ),
        None => f.render_widget(Paragraph::new(Line::styled(rule, theme::dim())), rule2),
    }

    let width = body.width as usize;
    let lines: Vec<Line<'static>> = d
        .rows
        .iter()
        .enumerate()
        .skip(d.scroll)
        .take(body.height as usize)
        .map(|(i, row)| render_row(app, d, row, i == d.cursor, width, tab))
        .collect();
    f.render_widget(Paragraph::new(lines), body);

    draw_footer_with(f, foot, app, HINTS);
}

fn gutter_blank(d: &DiffView) -> String {
    " ".repeat(2 * d.gutter + 4)
}

fn collapse_text(file: &FileChange) -> String {
    let reason = file.collapse.map(|c| c.label()).unwrap_or("collapsed");
    format!("⋯ collapsed ({reason}) · enter to expand")
}

fn binary_text(d: &DiffView, row: &Row) -> String {
    let file = &d.files[row.file];
    let p = d.patches[row.file].as_ref();
    let size = |s: Option<u64>| s.map(text::human_size).unwrap_or_else(|| "?".into());
    let (old, new) = (p.and_then(|p| p.old_size), p.and_then(|p| p.new_size));
    match file.status {
        Status::Added | Status::Untracked => format!("Binary file · {}", size(new.or(file.size))),
        Status::Deleted => format!("Binary file · {} (deleted)", size(old)),
        _ => format!("Binary file · {} → {}", size(old), size(new)),
    }
}

fn render_row(
    app: &App,
    d: &DiffView,
    row: &Row,
    is_cursor: bool,
    width: usize,
    tab: usize,
) -> Line<'static> {
    let file = &d.files[row.file];
    let note = |s: String| -> Line<'static> {
        let g = Span::styled(
            gutter_blank(d),
            if is_cursor {
                theme::cursor()
            } else {
                Style::new()
            },
        );
        Line::from(vec![
            g,
            Span::styled(
                text::truncate_end(&s, width.saturating_sub(2 * d.gutter + 4)),
                theme::dim(),
            ),
        ])
    };
    match row.kind {
        RowKind::Gap => Line::from(Span::styled(
            " ".repeat(width.min(1)),
            if is_cursor {
                theme::cursor()
            } else {
                Style::new()
            },
        )),
        RowKind::FileHeader => {
            let viewed = app.is_viewed(file);
            let mut left = vec![
                Span::styled(if is_cursor { "▸" } else { " " }, theme::bold()),
                Span::styled(if viewed { "✓ " } else { "  " }, theme::viewed()),
                Span::styled(
                    format!("{} ", file.status.letter()),
                    theme::status(file.status),
                ),
                Span::styled(path_label(file), theme::bold()),
            ];
            if let Some(sfx) = suffix(file, false) {
                left.push(Span::styled(format!("  {sfx}"), theme::dim()));
            }
            let s = Stats::of(std::slice::from_ref(file));
            let mut right = Vec::new();
            if file.is_binary() {
                right.push(Span::styled("binary", theme::dim()));
            } else {
                if s.added > 0 {
                    right.push(Span::styled(format!("+{}", s.added), theme::add()));
                }
                if s.deleted > 0 {
                    right.push(Span::raw(" "));
                    right.push(Span::styled(format!("-{}", s.deleted), theme::del()));
                }
            }
            right.push(Span::raw(" "));
            let line = left_right(left, right, width);
            if is_cursor {
                line.style(theme::cursor())
            } else {
                line
            }
        }
        RowKind::Meta(m) => {
            let meta = d.patches[row.file]
                .as_ref()
                .map(|p| text::display(&p.meta[m]))
                .unwrap_or_default();
            note(meta)
        }
        RowKind::Hunk(h) => {
            let header = d.patches[row.file]
                .as_ref()
                .map(|p| text::display(&p.hunks[h].header))
                .unwrap_or_default();
            let g = Span::styled(
                gutter_blank(d),
                if is_cursor {
                    theme::cursor()
                } else {
                    Style::new()
                },
            );
            let room = width.saturating_sub(2 * d.gutter + 4);
            Line::from(vec![
                g,
                Span::styled(text::truncate_end(&header, room), theme::hunk()),
            ])
        }
        RowKind::Line(h, l) => {
            let Some(p) = d.patches[row.file].as_ref() else {
                return Line::default();
            };
            let line = &p.hunks[h].lines[l];
            let num = |n: Option<u32>| match n {
                Some(n) => format!("{n:>w$}", w = d.gutter),
                None => " ".repeat(d.gutter),
            };
            let (sign, style) = match line.kind {
                LineKind::Add => ("+", theme::add()),
                LineKind::Del => ("-", theme::del()),
                LineKind::Context => (" ", Style::new()),
                LineKind::NoNewline => (" ", theme::dim()),
            };
            let gutter = format!(" {} {} ", num(line.old), num(line.new));
            let gstyle = if is_cursor {
                theme::cursor()
            } else {
                theme::dim()
            };
            let mut spans = vec![
                Span::styled(gutter, gstyle),
                Span::styled(format!("{sign} "), style),
            ];
            let room = width.saturating_sub(2 * d.gutter + 5);
            let segs = text::segments(&line.text, tab);
            let skip = if line.kind == LineKind::NoNewline {
                0
            } else {
                d.hscroll
            };
            spans.extend(styled_segs(
                text::slice(&segs, skip, room),
                style,
                theme::dim(),
            ));
            Line::from(spans)
        }
        RowKind::Collapsed => note(collapse_text(file)),
        RowKind::Binary => note(binary_text(d, row)),
        RowKind::Loading => note("loading…".into()),
        RowKind::TooLarge => note(format!(
            "Large file · {} · not shown",
            text::human_size(file.size.unwrap_or(0))
        )),
        RowKind::NoChanges => note(
            if file.new_mode == MODE_GITLINK && file.status == Status::Untracked {
                "untracked nested repository"
            } else if matches!(file.status, Status::Renamed(_) | Status::Copied(_)) {
                "renamed without changes"
            } else if file.status == Status::Unmerged {
                "unmerged · resolve the conflict to see a diff"
            } else {
                "no content changes"
            }
            .into(),
        ),
    }
}
