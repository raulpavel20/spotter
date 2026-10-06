//! The tab bar: one tab per repository, when there are several.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::text::{self, spans_width};
use super::theme;
use crate::workspace::Workspace;

/// Longer names are cut.
const MAX_NAME: usize = 24;

/// One tab's label: ` 1 api ◌ 3 `.
fn label(ws: &Workspace, i: usize) -> Vec<Span<'static>> {
    let t = &ws.tabs[i];
    let s = ws.summary(i);
    let gl = t.app.glyphs();
    let active = i == ws.active;
    let base = if active {
        Style::new().add_modifier(Modifier::REVERSED)
    } else {
        Style::new()
    };
    let name = if active || s.activity {
        base.add_modifier(Modifier::BOLD)
    } else if s.quiet() || s.loading {
        base.add_modifier(Modifier::DIM)
    } else {
        base
    };
    let dim = base.add_modifier(Modifier::DIM);
    let mut spans = vec![Span::styled(" ", base)];
    if i < 9 {
        spans.push(Span::styled(format!("{} ", i + 1), dim));
    }
    spans.push(Span::styled(text::truncate_end(&t.name, MAX_NAME), name));
    if s.dirty {
        spans.push(Span::styled(format!(" {}", gl.uncommitted), base));
    }
    if s.to_review > 0 {
        spans.push(Span::styled(format!(" {}", s.to_review), base));
    }
    if s.error {
        spans.push(Span::styled(
            format!(" {}", gl.error),
            base.patch(theme::error()),
        ));
    }
    if s.attention {
        spans.push(Span::styled(
            " !",
            base.patch(theme::banner()).add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::styled(" ", base));
    spans
}

/// The tabs `start..end` to show in `room` columns: the active one always,
/// as many around it as fit, starting at `offset` when it can.
/// A separator goes between tabs; a cut-off side takes a column for its
/// marker.
pub fn window(widths: &[usize], active: usize, offset: usize, room: usize) -> (usize, usize) {
    const MORE: usize = 1;
    let n = widths.len();
    if n == 0 {
        return (0, 0);
    }
    let active = active.min(n - 1);
    let fits = |s: usize, e: usize| {
        let marks = MORE * (usize::from(s > 0) + usize::from(e < n));
        widths[s..e].iter().sum::<usize>() + (e - s - 1) + marks <= room
    };
    let mut start = offset.min(active);
    while start < active && !fits(start, active + 1) {
        start += 1;
    }
    let mut end = active + 1;
    while end < n && fits(start, end + 1) {
        end += 1;
    }
    while start > 0 && fits(start - 1, end) {
        start -= 1;
    }
    (start, end)
}

pub fn draw(f: &mut Frame, area: Rect, ws: &mut Workspace) {
    let gl = ws.active_app().glyphs();
    let labels: Vec<Vec<Span<'static>>> = (0..ws.tabs.len()).map(|i| label(ws, i)).collect();
    let widths: Vec<usize> = labels.iter().map(|l| spans_width(l)).collect();
    let title = ws
        .title
        .as_ref()
        .map(|t| format!(" {t} "))
        .unwrap_or_default();
    let room = area.width as usize;
    // The folder's name, when it fits beside every tab.
    let all = widths.iter().sum::<usize>() + widths.len().saturating_sub(1);
    let show_title = !title.is_empty() && all + 1 + text::width(&title) <= room;
    let (start, end) = window(&widths, ws.active, ws.bar_offset, room);
    ws.bar_offset = start;

    let sep = Span::styled(gl.border.vertical_left, theme::dim());
    let mut spans = Vec::new();
    if start > 0 {
        spans.push(Span::styled(gl.more_left, theme::dim()));
    }
    for (i, l) in labels.into_iter().enumerate().take(end).skip(start) {
        if i > start {
            spans.push(sep.clone());
        }
        spans.extend(l);
    }
    let right = if end < ws.tabs.len() {
        vec![Span::styled(gl.more_right, theme::dim())]
    } else if show_title {
        vec![Span::styled(title, theme::dim())]
    } else {
        Vec::new()
    };
    let line: Line<'static> = text::left_right(spans, right, room);
    f.render_widget(Paragraph::new(line), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_keeps_the_active_tab_in_view() {
        let w = [10, 10, 10, 10, 10];
        // Everything fits.
        assert_eq!(window(&w, 0, 0, 80), (0, 5));
        // 32 columns: two tabs and a separator, plus "more" marks.
        assert_eq!(window(&w, 0, 0, 32), (0, 2));
        assert_eq!(window(&w, 4, 0, 32), (3, 5));
        // Scrolling keeps the offset while the active tab is visible.
        assert_eq!(window(&w, 2, 1, 32), (1, 3));
        assert_eq!(window(&w, 2, 2, 32), (2, 4));
        assert_eq!(window(&w, 2, 2, 22), (2, 3));
        // Too narrow even for one: the active one alone.
        assert_eq!(window(&w, 3, 0, 5), (3, 4));
        assert_eq!(window(&[], 0, 0, 10), (0, 0));
    }
}
