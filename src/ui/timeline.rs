//! The timeline panel: ◌ Uncommitted, commits, and a pinned Σ row.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::glyphs::Glyphs;
use super::text::left_right;
use super::theme;
use crate::app::{App, Focus};
use crate::model::{FileChange, Stats, TargetId};

pub fn needed_height(app: &App) -> u16 {
    let hint = app
        .snap
        .as_ref()
        .is_some_and(|s| s.base.hint.is_some() && s.commits.is_empty());
    let rows = 1 + app.commits().len() + usize::from(hint) + if app.has_total() { 2 } else { 0 };
    (rows + 2).min(u16::MAX as usize) as u16
}

/// `+a` / `-d` columns, aligned across the panel.
pub struct StatCols {
    plus: usize,
    minus: usize,
}

impl StatCols {
    pub fn new<'a>(all: impl Iterator<Item = &'a Stats>) -> Self {
        let mut c = StatCols { plus: 0, minus: 0 };
        for s in all {
            c.plus = c.plus.max(plus(s).len());
            c.minus = c.minus.max(minus(s).len());
        }
        c
    }

    pub fn spans(&self, s: &Stats) -> Vec<Span<'static>> {
        if self.plus + self.minus == 0 {
            return Vec::new();
        }
        vec![
            Span::styled(format!("{:>w$}", plus(s), w = self.plus), theme::add()),
            Span::raw(" "),
            Span::styled(format!("{:<w$}", minus(s), w = self.minus), theme::del()),
        ]
    }
}

fn plus(s: &Stats) -> String {
    if s.added > 0 {
        format!("+{}", s.added)
    } else {
        String::new()
    }
}

fn minus(s: &Stats) -> String {
    if s.deleted > 0 {
        format!("-{}", s.deleted)
    } else {
        String::new()
    }
}

fn files_label(n: usize) -> String {
    format!("{n} file{}", if n == 1 { "" } else { "s" })
}

pub fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Focus::Timeline;
    let gl = app.glyphs();
    let block = Block::bordered()
        .border_set(gl.border)
        .title(" Timeline ")
        .border_style(theme::border(focused));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let Some(snap) = app.snap.as_ref() else {
        f.render_widget(
            Paragraph::new(Line::styled(" loading…", theme::dim())),
            inner,
        );
        return;
    };
    let width = inner.width as usize;

    let unc = Stats::of(&snap.uncommitted);
    let commit_stats: Vec<Stats> = snap.commits.iter().map(|c| Stats::of(&c.files)).collect();
    let total = snap.total.as_deref().map(Stats::of);
    let cols = StatCols::new(
        [&unc]
            .into_iter()
            .chain(&commit_stats)
            .chain(total.as_ref()),
    );

    let sigma = total.is_some() && inner.height >= 3;
    let list_h = inner.height as usize - if sigma { 2 } else { 0 };
    let hint = snap.base.hint.clone().filter(|_| snap.commits.is_empty());

    // Rows above Σ: ◌, then commits (or the hint).
    let n_rows = 1 + snap.commits.len();
    let sel = app.sel;
    if sel < n_rows {
        if sel < app.tl_offset {
            app.tl_offset = sel;
        } else if sel >= app.tl_offset + list_h {
            app.tl_offset = sel + 1 - list_h;
        }
    }
    app.tl_offset = app.tl_offset.min(n_rows.saturating_sub(list_h));
    let app = &*app;
    let snap = app.snap.as_ref().expect("snapshot");

    let mut lines: Vec<Line<'static>> = Vec::new();
    for row in app.tl_offset..n_rows {
        if lines.len() >= list_h {
            break;
        }
        let selected = row == sel;
        let mut left = vec![cursor(selected, gl)];
        let mut right = Vec::new();
        if row == 0 {
            left.push(Span::styled(
                format!("{} ", gl.uncommitted),
                theme::banner(),
            ));
            left.push(Span::raw("Uncommitted"));
            if snap.uncommitted.is_empty() {
                right.push(Span::styled("clean", theme::dim()));
            } else {
                right.push(Span::styled(files_label(unc.files), theme::dim()));
                right.push(Span::raw("  "));
                right.extend(cols.spans(&unc));
            }
        } else {
            let c = &snap.commits[row - 1];
            let g = app.glyph(&c.files);
            left.push(Span::styled(format!("{} ", g.symbol(gl)), theme::glyph(g)));
            left.push(Span::styled(c.short().to_owned(), theme::sha()));
            left.push(Span::raw(" "));
            left.push(Span::raw(c.subject.clone()));
            if c.is_merge() {
                left.push(Span::styled(" (merge)", theme::dim()));
            }
            right.extend(cols.spans(&commit_stats[row - 1]));
        }
        right.push(Span::raw(" "));
        lines.push(style_row(left_right(left, right, width), selected, focused));
        if row == 0 {
            if let Some(h) = &hint {
                if lines.len() < list_h {
                    lines.push(Line::styled(format!("   {h}"), theme::dim()));
                }
            }
        }
    }
    let list_area = Rect {
        height: list_h as u16,
        ..inner
    };
    f.render_widget(Paragraph::new(lines), list_area);

    if sigma {
        let y = inner.y + list_h as u16;
        let buf = f.buffer_mut();
        let style = theme::border(focused);
        buf.set_string(area.x, y, gl.tee_left, style);
        buf.set_string(inner.x, y, gl.rule.repeat(width), style);
        buf.set_string(area.x + area.width - 1, y, gl.tee_right, style);

        let total_files = snap.total.as_deref().unwrap_or_default();
        let t = total.expect("total stats");
        let selected = Some(sel) == app.sigma_row();
        let mut left = vec![
            cursor(selected, gl),
            Span::styled(format!("{} ", gl.total), theme::bold()),
        ];
        left.push(Span::raw(if snap.opts.include_wt {
            "Branch total"
        } else {
            "Branch total (committed)"
        }));
        let mut right = vec![
            Span::styled(files_label(total_files.len()), theme::dim()),
            Span::raw("  "),
        ];
        right.extend(cols.spans(&t));
        right.push(Span::raw(" "));
        let line = style_row(left_right(left, right, width), selected, focused);
        f.render_widget(
            Paragraph::new(line),
            Rect {
                y: y + 1,
                height: 1,
                ..inner
            },
        );
    }
}

fn cursor(selected: bool, gl: &Glyphs) -> Span<'static> {
    Span::raw(if selected { gl.cursor } else { " " })
}

pub fn style_row(line: Line<'static>, selected: bool, focused: bool) -> Line<'static> {
    match (selected, focused) {
        (true, true) => line.style(theme::cursor()),
        (true, false) => line.style(theme::bold()),
        _ => line,
    }
}

/// Short label for a target, used in panel titles and the diff header.
pub fn target_label(app: &App, id: &TargetId, with_subject: bool) -> String {
    match id {
        TargetId::Uncommitted => "Uncommitted".into(),
        TargetId::Total => "Branch total".into(),
        TargetId::Commit(sha) => {
            let short = &sha[..sha.len().min(7)];
            match app.snap.as_ref().and_then(|s| s.commit(sha)) {
                Some(c) if with_subject => format!("{short} {}", c.subject),
                _ => short.to_owned(),
            }
        }
    }
}

pub fn files_stats(files: &[FileChange]) -> Stats {
    Stats::of(files)
}
