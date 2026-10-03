//! The files panel for the selected target.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::text::{self, left_right};
use super::theme;
use super::timeline::{StatCols, style_row, target_label};
use crate::app::{App, Focus};
use crate::model::{FileChange, MODE_GITLINK, MODE_SYMLINK, Stats, Status, TargetId};

/// `old → new (92%)` for renames, the path otherwise.
pub fn path_label(f: &FileChange) -> String {
    let new = text::display(&f.path);
    match (&f.old_path, f.status) {
        (Some(old), Status::Renamed(score) | Status::Copied(score)) => {
            format!("{} → {new} ({score}%)", text::display(old))
        }
        _ => new,
    }
}

/// Dim suffix after the path: file kind and, optionally, the collapsed
/// marker.
pub fn suffix(f: &FileChange, with_collapse: bool) -> Option<String> {
    let mut parts = Vec::new();
    if f.new_mode == MODE_SYMLINK || f.old_mode == MODE_SYMLINK {
        parts.push("symlink".to_owned());
    }
    if f.new_mode == MODE_GITLINK || f.old_mode == MODE_GITLINK {
        parts.push(
            if f.status == Status::Untracked {
                "nested repo"
            } else {
                "submodule"
            }
            .to_owned(),
        );
    }
    if f.old_mode != f.new_mode
        && f.old_mode != 0
        && f.new_mode != 0
        && f.status == Status::Modified
    {
        parts.push(format!("mode {:o} → {:o}", f.old_mode, f.new_mode));
    }
    if with_collapse && f.collapse.is_some() {
        parts.push("⋯ collapsed".to_owned());
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

pub fn draw(f: &mut Frame, area: Rect, app: &mut App, wide: bool) {
    let focused = app.focus == Focus::Files;
    let id = app.selected();
    let title = match &id {
        Some(id) => format!(" Files · {} ", target_label(app, id, wide)),
        None => " Files ".to_owned(),
    };
    let block = Block::bordered()
        .title(text::truncate_end(
            &title,
            area.width.saturating_sub(2) as usize,
        ))
        .border_style(theme::border(focused));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 || app.snap.is_none() {
        return;
    }
    let files = app.selected_files().to_vec();
    if files.is_empty() {
        let msg = match id {
            Some(TargetId::Uncommitted) => " working tree clean",
            _ => " no changes",
        };
        f.render_widget(Paragraph::new(Line::styled(msg, theme::dim())), inner);
        return;
    }
    let h = inner.height as usize;
    if app.file_sel < app.files_offset {
        app.files_offset = app.file_sel;
    } else if app.file_sel >= app.files_offset + h {
        app.files_offset = app.file_sel + 1 - h;
    }
    app.files_offset = app.files_offset.min(files.len().saturating_sub(h));

    let stats: Vec<Stats> = files
        .iter()
        .map(|f| Stats::of(std::slice::from_ref(f)))
        .collect();
    let cols = StatCols::new(stats.iter());
    let width = inner.width as usize;
    let lines: Vec<Line<'static>> = files
        .iter()
        .enumerate()
        .skip(app.files_offset)
        .take(h)
        .map(|(i, file)| {
            let selected = i == app.file_sel;
            let viewed = app.is_viewed(file);
            let mut left = vec![
                Span::raw(if selected { "▸" } else { " " }),
                Span::styled(if viewed { "✓ " } else { "  " }, theme::viewed()),
                Span::styled(
                    format!("{}  ", file.status.letter()),
                    theme::status(file.status),
                ),
            ];
            let mut right = Vec::new();
            if let Some(sfx) = suffix(file, true) {
                right.push(Span::styled(sfx, theme::dim()));
                right.push(Span::raw("  "));
            }
            if file.is_binary() {
                right.push(Span::styled("binary", theme::dim()));
            } else {
                right.extend(cols.spans(&stats[i]));
            }
            right.push(Span::raw(" "));
            // Paths are truncated from the left so the file name stays visible.
            let lw = text::spans_width(&left);
            let rw = text::spans_width(&right);
            let room = width.saturating_sub(lw + rw + 1);
            left.push(Span::raw(text::truncate_start(&path_label(file), room)));
            style_row(left_right(left, right, width), selected, focused)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}
