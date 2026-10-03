//! The full-screen diff view: the whole target as one scroll.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::files::{path_label, suffix};
use super::header::{Hint, draw_footer_hints, hint};
use super::palette::Palette;
use super::text::{self, left_right, styled_segs};
use super::theme;
use super::timeline::target_label;
use crate::app::{App, Glyph};
use crate::diffview::{DiffView, Row, RowKind};
use crate::highlight::Run;
use crate::model::{FileChange, LineKind, MODE_GITLINK, Stats, Status};

/// Footer hints for the diff view, for the current file's state.
fn diff_hints(app: &App, d: &DiffView) -> Vec<Hint> {
    let cur = d.current_file();
    let viewed = cur.is_some_and(|i| app.is_viewed(&d.files[i]));
    let folded = cur.is_some_and(|i| d.is_folded(i));
    let view = if viewed { "unview" } else { "viewed" };
    if app.explorer_open && app.diff_focus == crate::app::DiffFocus::Explorer {
        return vec![
            hint("j/k", "file", 1),
            hint("enter", if folded { "expand & read" } else { "read" }, 1),
            hint("space", view, 1),
            hint("e", "edit", 2),
            hint("→/tab", "diff", 2),
            hint("f", "close", 2),
            hint("esc", "back", 3),
        ];
    }
    let is_commit = matches!(d.target, crate::model::TargetId::Commit(_));
    let is_merge = match &d.target {
        crate::model::TargetId::Commit(sha) => app
            .snap
            .as_ref()
            .and_then(|s| s.commit(sha))
            .is_some_and(|c| c.is_merge()),
        _ => false,
    };
    let mut h = vec![
        hint("]/[", "hunk", 2),
        hint("}/{", "file", 2),
        hint("enter", if folded { "expand" } else { "collapse" }, 1),
        hint("space", view, 1),
    ];
    if app.explorer_open {
        h.push(hint("←/tab", "files", 2));
        h.push(hint("f", "hide files", 3));
    } else {
        h.push(hint("f", "files", 2));
    }
    h.push(hint(
        "z",
        if d.wrap.is_some() { "no wrap" } else { "wrap" },
        2,
    ));
    h.push(hint("e", "edit", 2));
    if is_merge {
        h.push(hint("m", "merge diff mode", 2));
    }
    if is_commit {
        h.push(hint("n/p", "commit", 3));
    }
    h.push(hint(
        "W",
        if d.opts.ignore_ws {
            "show whitespace"
        } else {
            "hide whitespace"
        },
        3,
    ));
    h.push(hint("esc", "back", 1));
    h
}

pub fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let [head, rule1, body, rule2, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let tab = app.config.tab_width;
    let gl = app.glyphs();
    let split = super::explorer::split(app, body);
    {
        let d = app.diff.as_mut().expect("diff open");
        d.viewport = body.height.max(1) as usize;
        d.clamp_scroll();
    }
    // The side panel's box spans the rule rows, so it lines up with the
    // lines framing the diff.
    let side = split.explorer.filter(|_| !split.drawer);
    if let Some(ex) = side {
        let ex = Rect {
            y: rule1.y,
            height: body.height + 2,
            ..ex
        };
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
        Span::styled(g.symbol(gl).to_owned(), theme::glyph(g)),
        Span::raw(format!(" {viewed}/{n} viewed ")),
    ];
    f.render_widget(
        Paragraph::new(left_right(left, right, head.width as usize)),
        head,
    );

    // The rules above and below the diff frame it. With the explorer open
    // they act like its border: bright while the diff has focus, dim while
    // the explorer has it.
    let (rule1, rule2) = match side {
        Some(_) => (
            Rect {
                x: body.x,
                width: body.width,
                ..rule1
            },
            Rect {
                x: body.x,
                width: body.width,
                ..rule2
            },
        ),
        None => (rule1, rule2),
    };
    let frame = if app.explorer_open {
        theme::border(app.diff_focus == crate::app::DiffFocus::Diff)
    } else {
        theme::dim()
    };
    let rule = gl.rule.repeat(rule1.width as usize);
    f.render_widget(Paragraph::new(Line::styled(rule.clone(), frame)), rule1);
    match &d.error {
        Some(e) => f.render_widget(
            Paragraph::new(Line::styled(
                text::truncate_end(&format!("{} {e}", gl.error), rule2.width as usize),
                theme::error(),
            )),
            rule2,
        ),
        None => f.render_widget(Paragraph::new(Line::styled(rule, frame)), rule2),
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

    draw_footer_hints(f, foot, app, diff_hints(app, d));
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

fn collapse_text(app: &App, file: &FileChange) -> String {
    let reason = file.collapse.map(|c| c.label()).unwrap_or("collapsed");
    format!(
        "{} collapsed ({reason}) · enter to expand",
        app.glyphs().collapsed
    )
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
    let gl = app.glyphs();
    // Notes and @@ lines start where code starts.
    let indent = d.gutter_cols(app.config.line_numbers) + 1;
    let note = |s: String| -> Line<'static> {
        Line::from(vec![
            Span::raw(" ".repeat(indent)),
            Span::styled(
                text::truncate_end(&s, width.saturating_sub(indent)),
                theme::dim(),
            ),
        ])
    };
    match row.kind {
        RowKind::Blank => Line::default(),
        RowKind::Separator => Line::styled(gl.rule.repeat(width), theme::dim()),
        RowKind::FileHeader => {
            let viewed = app.is_viewed(file);
            let folded = d.is_folded(row.file);
            let mut left = vec![
                // ▾ open, ▸ folded (Enter toggles).
                Span::styled(
                    format!(" {} ", if folded { gl.folded } else { gl.open }),
                    theme::dim(),
                ),
                Span::styled(
                    if viewed {
                        format!("{} ", gl.viewed)
                    } else {
                        "  ".into()
                    },
                    theme::viewed(),
                ),
                Span::styled(
                    format!("{} ", file.status.letter()),
                    theme::status(file.status),
                ),
                Span::styled(path_label(file), theme::bold()),
            ];
            if let Some(sfx) = suffix(file, false, gl.collapsed) {
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
            let room = width.saturating_sub(indent);
            Line::from(vec![
                Span::raw(" ".repeat(indent)),
                Span::styled(text::truncate_end(&header, room), theme::hunk()),
            ])
        }
        RowKind::Line(..) | RowKind::Wrap(..) => code_row(app, d, row, width, tab),
        RowKind::Collapsed => note(collapse_text(app, file)),
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
            } else if d.opts.ignore_ws && file.status == Status::Modified {
                "only whitespace changes · W shows them"
            } else if file.status == Status::Unmerged {
                "unmerged · resolve the conflict to see a diff"
            } else {
                "no content changes"
            }
            .into(),
        ),
    }
}

/// A code line, or one row of it when it wraps: the gutter, the sign (or
/// `↪` on continuation rows), then the code, tinted to the edge on added
/// and deleted lines.
fn code_row(app: &App, d: &DiffView, row: &Row, width: usize, tab: usize) -> Line<'static> {
    let (h, l, part) = match row.kind {
        RowKind::Line(h, l) => (h, l, 0),
        RowKind::Wrap(h, l, part) => (h, l, part),
        _ => return Line::default(),
    };
    let Some(p) = d.patches[row.file].as_ref() else {
        return Line::default();
    };
    let line = &p.hunks[h].lines[l];
    let pal = app.palette;
    let line_numbers = app.config.line_numbers;
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
    let (gutter, sign, sign_style) = if part > 0 {
        let cols = d.gutter_cols(line_numbers);
        (" ".repeat(cols), app.glyphs().wrap, theme::dim())
    } else if line_numbers {
        (
            format!(" {} {} ", num(line.old), num(line.new)),
            sign,
            sign_style,
        )
    } else {
        (" ".into(), sign, sign_style)
    };
    let mut spans = vec![
        Span::styled(gutter, theme::dim()),
        Span::styled(format!("{sign} "), sign_style.patch(base)),
    ];
    let room = d.code_width(width, line_numbers);
    let wrapped = d.wraps.get(&(row.file, h, l));
    let indent = wrapped.filter(|_| part > 0).map_or(0, |w| w.indent);
    let segs = if line.kind == LineKind::NoNewline {
        text::slice(&text::segments(&line.text, tab), 0, room)
    } else {
        let runs = d
            .highlight(row.file)
            .filter(|_| app.config.syntax)
            .map(|hl| hl.line(h, l))
            .unwrap_or_default();
        let emph: &[(u32, u32)] = if app.config.word_highlights {
            &line.emph
        } else {
            &[]
        };
        let mut style_at = code_styler(&pal, base, runs, emph, emph_bg);
        match wrapped {
            // Only this row's bytes, so a huge line isn't laid out again
            // for every row it wraps into.
            Some(w) => {
                let (col, from) = if part == 0 {
                    (0, 0)
                } else {
                    w.starts[part - 1]
                };
                let to = w.starts.get(part).map_or(line.text.len(), |&(_, b)| b);
                let segs =
                    text::segments_from(&line.text[from..to], tab, col, |off| style_at(from + off));
                text::slice(&segs, 0, room.saturating_sub(indent))
            }
            None => {
                let segs = text::segments_with(&line.text, tab, &mut style_at);
                text::slice(&segs, d.hscroll, room)
            }
        }
    };
    let base_text = if line.kind == LineKind::NoNewline {
        theme::dim()
    } else {
        base
    };
    if indent > 0 {
        spans.push(Span::styled(" ".repeat(indent), base));
    }
    let body = styled_segs(segs, base_text, theme::dim());
    let used = indent + text::spans_width(&body);
    spans.extend(body);
    if row_bg.is_some() && used < room {
        // Tint the whole row, not just the text.
        spans.push(Span::styled(" ".repeat(room - used), base));
    }
    Line::from(spans)
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
        if let (Some(&(a, _)), Some(bg)) = (emph.get(ei), emph_bg)
            && a as usize <= off
        {
            st = st.bg(bg);
        }
        st
    }
}
