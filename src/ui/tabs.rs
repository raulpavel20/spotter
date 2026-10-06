//! The tabs: one per repository, when there are several. The active one
//! is a box that opens into the box below it.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::text::{self, spans_width};
use super::theme;
use crate::workspace::Workspace;

/// The top of the active tab, and the row of tabs.
pub const HEIGHT: u16 = 2;

/// Longer names are cut.
const MAX_NAME: usize = 24;

/// One tab's label: ` 1 api ◌ 3 `.
fn label(ws: &Workspace, i: usize) -> Vec<Span<'static>> {
    let t = &ws.tabs[i];
    let s = ws.summary(i);
    let gl = t.app.glyphs();
    let name = if i == ws.active || s.activity {
        theme::bold()
    } else if s.quiet() || s.loading {
        theme::dim()
    } else {
        Style::new()
    };
    let mut spans = vec![Span::raw(" ")];
    if i < 9 {
        spans.push(Span::styled(format!("{} ", i + 1), theme::dim()));
    }
    spans.push(Span::styled(text::truncate_end(&t.name, MAX_NAME), name));
    if s.dirty {
        spans.push(Span::raw(format!(" {}", gl.uncommitted)));
    }
    if s.to_review > 0 {
        spans.push(Span::raw(format!(" {}", s.to_review)));
    }
    if s.error {
        spans.push(Span::styled(format!(" {}", gl.error), theme::error()));
    }
    if s.attention {
        spans.push(Span::styled(
            " !",
            theme::banner().add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::raw(" "));
    spans
}

/// The tabs `start..end` to show in `room` columns: the active one always,
/// as many around it as fit, starting at `offset` when it can. Widths
/// include the active tab's box; a cut-off side takes a column for its
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
        widths[s..e].iter().sum::<usize>() + marks <= room
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

/// Draws the tabs in `area` (two rows). Returns the active tab's left and
/// right border columns, where it opens into the box below.
pub fn draw(f: &mut Frame, area: Rect, ws: &mut Workspace) -> Option<(u16, u16)> {
    let gl = ws.active_app().glyphs();
    let border = gl.border;
    let labels: Vec<Vec<Span<'static>>> = (0..ws.tabs.len()).map(|i| label(ws, i)).collect();
    let widths: Vec<usize> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| spans_width(l) + if i == ws.active { 2 } else { 0 })
        .collect();
    let room = area.width as usize;
    let (start, end) = window(&widths, ws.active, ws.bar_offset, room);
    ws.bar_offset = start;

    let edge = theme::dim();
    let mut spans = Vec::new();
    let mut x = 0;
    let mut tab = (0, 0);
    if start > 0 {
        spans.push(Span::styled(gl.more_left, edge));
        x += 1;
    }
    for (i, l) in labels.into_iter().enumerate().take(end).skip(start) {
        let w = spans_width(&l);
        if i == ws.active {
            tab = (x, x + w + 1);
            spans.push(Span::styled(border.vertical_left, edge));
            spans.extend(l);
            spans.push(Span::styled(border.vertical_right, edge));
            x += w + 2;
        } else {
            spans.extend(l);
            x += w;
        }
    }
    // The folder's name, when it fits beside every tab.
    let title = ws
        .title
        .as_ref()
        .map(|t| format!(" {t} "))
        .unwrap_or_default();
    let all = widths.iter().sum::<usize>();
    let right = if end < ws.tabs.len() {
        vec![Span::styled(gl.more_right, edge)]
    } else if !title.is_empty() && all + text::width(&title) < room {
        vec![Span::styled(title, theme::dim())]
    } else {
        Vec::new()
    };
    let row = Rect {
        y: area.y + 1,
        height: 1,
        ..area
    };
    f.render_widget(Paragraph::new(text::left_right(spans, right, room)), row);

    let last = area.width.saturating_sub(1) as usize;
    let (l, r) = (tab.0.min(last) as u16, tab.1.min(last) as u16);
    if r > l {
        let top = format!(
            "{}{}{}",
            border.top_left,
            border.horizontal_top.repeat((r - l - 1) as usize),
            border.top_right
        );
        let at = Rect {
            x: area.x + l,
            y: area.y,
            width: r - l + 1,
            height: 1,
        };
        f.render_widget(Paragraph::new(Line::styled(top, edge)), at);
    }
    Some((area.x + l, area.x + r))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_keeps_the_active_tab_in_view() {
        let w = [10, 10, 10, 10, 10];
        // Everything fits.
        assert_eq!(window(&w, 0, 0, 80), (0, 5));
        // 32 columns: three tabs, or two and the "more" marks.
        assert_eq!(window(&w, 0, 0, 32), (0, 3));
        assert_eq!(window(&w, 4, 0, 32), (2, 5));
        // Scrolling keeps the offset while the active tab is visible.
        assert_eq!(window(&w, 2, 1, 22), (1, 3));
        assert_eq!(window(&w, 2, 2, 22), (2, 4));
        assert_eq!(window(&w, 2, 2, 21), (2, 3));
        // Too narrow even for one: the active one alone.
        assert_eq!(window(&w, 3, 0, 5), (3, 4));
        assert_eq!(window(&[], 0, 0, 10), (0, 0));
    }
}
