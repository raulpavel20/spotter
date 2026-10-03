//! The full-screen diff view: the whole target as one scroll.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::files::{path_label, suffix};
use super::header::draw_footer_with;
use super::palette::Palette;
use super::text::{self, left_right, styled_segs};
use super::theme;
use super::timeline::target_label;
use crate::app::{App, Glyph};
use crate::diffview::{DiffView, Row, RowKind};
use crate::highlight::Run;
use crate::model::{FileChange, LineKind, MODE_GITLINK, Stats, Status};

const HINTS: &[(&str, &str)] = &[
    ("]/[", "hunk"),
    ("}/{", "file"),
    ("f", "files"),
    ("space", "viewed"),
    ("n/p", "commit"),
    ("e", "edit"),
    ("esc", "back"),
];

const EXPLORER_HINTS: &[(&str, &str)] = &[
    ("enter", "go to file"),
    ("space", "viewed"),
    ("tab", "diff"),
    ("f", "close"),
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
    let split = super::explorer::split(app, body);
    {
        let d = app.diff.as_mut().expect("diff open");
        d.viewport = body.height.max(1) as usize;
        d.clamp_scroll();
    }
    if let Some(ex) = split.explorer.filter(|_| !split.drawer) {
        super::explorer::draw(f, ex, app, false);
    }
    let body = split.diff;
    let app_ref = &*app;
    let d = app_ref.diff.as_ref().expect("diff open");
    let app = app_ref;

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
    let mut lines: Vec<Line<'static>> = d
        .rows
        .iter()
        .skip(d.scroll)
        .take(body.height as usize)
        .map(|row| render_row(app, d, row, width, tab))
        .collect();
    // Once a file's header scrolls off, pin it to the top line.
    if let (Some(i), Some(first)) = (d.sticky(), lines.first_mut()) {
        let header = Row {
            file: i,
            kind: RowKind::FileHeader,
        };
        *first = render_row(app, d, &header, width, tab).patch_style(theme::sticky());
    }
    f.render_widget(Paragraph::new(lines), body);

    let explorer_focused = app.explorer_open && app.diff_focus == crate::app::DiffFocus::Explorer;
    draw_footer_with(
        f,
        foot,
        app,
        if explorer_focused {
            EXPLORER_HINTS
        } else {
            HINTS
        },
    );
}

/// The explorer drawer, drawn over the diff on narrow panes.
pub fn draw_drawer(f: &mut Frame, area: Rect, app: &mut App) {
    let body = Rect {
        y: area.y + 2,
        height: area.height.saturating_sub(4),
        ..area
    };
    let split = super::explorer::split(app, body);
    if let (Some(ex), true) = (split.explorer, split.drawer) {
        super::explorer::draw(f, ex, app, true);
    }
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

fn render_row(app: &App, d: &DiffView, row: &Row, width: usize, tab: usize) -> Line<'static> {
    let file = &d.files[row.file];
    let note = |s: String| -> Line<'static> {
        Line::from(vec![
            Span::raw(gutter_blank(d)),
            Span::styled(
                text::truncate_end(&s, width.saturating_sub(2 * d.gutter + 4)),
                theme::dim(),
            ),
        ])
    };
    match row.kind {
        RowKind::Blank => Line::default(),
        RowKind::Separator => Line::styled("─".repeat(width), theme::dim()),
        RowKind::FileHeader => {
            let viewed = app.is_viewed(file);
            let folded = d.is_folded(row.file);
            let mut left = vec![
                // ▾ open, ▸ folded (Enter toggles).
                Span::styled(if folded { " ▸ " } else { " ▾ " }, theme::dim()),
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
            // Viewed and folded: out of the way.
            if viewed && folded {
                line.patch_style(theme::dim())
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
            let room = width.saturating_sub(2 * d.gutter + 4);
            Line::from(vec![
                Span::raw(gutter_blank(d)),
                Span::styled(text::truncate_end(&header, room), theme::hunk()),
            ])
        }
        RowKind::Line(h, l) => {
            let Some(p) = d.patches[row.file].as_ref() else {
                return Line::default();
            };
            let line = &p.hunks[h].lines[l];
            let pal = app.palette;
            let num = |n: Option<u32>| match n {
                Some(n) => format!("{n:>w$}", w = d.gutter),
                None => " ".repeat(d.gutter),
            };
            let (sign, sign_style, row_bg, emph_bg) = match line.kind {
                LineKind::Add => ("+", theme::add(), Some(pal.plus), Some(pal.plus_emph)),
                LineKind::Del => ("-", theme::del(), Some(pal.minus), Some(pal.minus_emph)),
                LineKind::Context => (" ", Style::new(), None, None),
                LineKind::NoNewline => (" ", theme::dim(), None, None),
            };
            let base = row_bg.map_or(Style::new(), |bg| Style::new().bg(bg));
            let gutter = format!(" {} {} ", num(line.old), num(line.new));
            let mut spans = vec![
                Span::styled(gutter, theme::dim()),
                Span::styled(format!("{sign} "), sign_style.patch(base)),
            ];
            let room = width.saturating_sub(2 * d.gutter + 5);
            let segs = if line.kind == LineKind::NoNewline {
                text::slice(&text::segments(&line.text, tab), 0, room)
            } else {
                let runs = d
                    .highlight(row.file)
                    .map(|hl| hl.line(h, l))
                    .unwrap_or_default();
                let mut style_at = code_styler(&pal, base, runs, &line.emph, emph_bg);
                let segs = text::segments_with(&line.text, tab, &mut style_at);
                text::slice(&segs, d.hscroll, room)
            };
            let base_text = if line.kind == LineKind::NoNewline {
                theme::dim()
            } else {
                base
            };
            let body = styled_segs(segs, base_text, theme::dim());
            let used = text::spans_width(&body);
            spans.extend(body);
            if row_bg.is_some() && used < room {
                // Tint the whole row, not just the text.
                spans.push(Span::styled(" ".repeat(room - used), base));
            }
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

fn run_style(pal: &Palette, r: &Run) -> Style {
    let mut st = Style::new();
    if let Some(c) = pal.theme_color(r.fg[0], r.fg[1], r.fg[2], r.fg[3]) {
        st = st.fg(c);
    }
    if r.bold {
        st = st.add_modifier(Modifier::BOLD);
    }
    if r.italic {
        st = st.add_modifier(Modifier::ITALIC);
    }
    if r.underline {
        st = st.add_modifier(Modifier::UNDERLINED);
    }
    st
}

/// Style for each byte offset of a code line: the row tint, the syntax
/// color, then the changed-word tint. Offsets arrive in increasing order,
/// so both range lists are walked with a cursor.
fn code_styler<'a>(
    pal: &'a Palette,
    base: Style,
    runs: &'a [Run],
    emph: &'a [(u32, u32)],
    emph_bg: Option<Color>,
) -> impl FnMut(usize) -> Style + 'a {
    let (mut ri, mut ei) = (0, 0);
    move |off: usize| {
        let mut st = base;
        while ri < runs.len() && (runs[ri].end as usize) <= off {
            ri += 1;
        }
        if let Some(r) = runs.get(ri).filter(|r| r.start as usize <= off) {
            st = st.patch(run_style(pal, r));
        }
        while ei < emph.len() && (emph[ei].1 as usize) <= off {
            ei += 1;
        }
        if let (Some(&(a, _)), Some(bg)) = (emph.get(ei), emph_bg) {
            if a as usize <= off {
                st = st.bg(bg);
            }
        }
        st
    }
}
