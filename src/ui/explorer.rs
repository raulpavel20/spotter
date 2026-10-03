//! The file explorer inside the diff view: a side panel on wide panes, a
//! drawer over the diff on narrow ones.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Clear, Paragraph};

use super::files::{file_lines, keep_visible};
use super::theme;
use crate::app::{App, DiffFocus};

/// Where the explorer goes, and the area left for the diff.
pub struct Split {
    pub explorer: Option<Rect>,
    pub diff: Rect,
    pub drawer: bool,
}

pub fn split(app: &App, body: Rect) -> Split {
    if !app.explorer_open {
        return Split {
            explorer: None,
            diff: body,
            drawer: false,
        };
    }
    if app.narrow() {
        let w = body.width.saturating_sub(10).clamp(1, 44);
        Split {
            explorer: Some(Rect { width: w, ..body }),
            diff: body,
            drawer: true,
        }
    } else {
        let w = (body.width as u32 * 30 / 100).clamp(28, 50) as u16;
        let w = w.min(body.width.saturating_sub(20));
        Split {
            explorer: Some(Rect { width: w, ..body }),
            diff: Rect {
                x: body.x + w,
                width: body.width - w,
                ..body
            },
            drawer: false,
        }
    }
}

pub fn draw(f: &mut Frame, area: Rect, app: &mut App, drawer: bool) {
    let focused = app.diff_focus == DiffFocus::Explorer;
    let viewed = {
        let d = app.diff.as_ref().expect("diff open");
        app.viewed_count(&d.files)
    };
    let Some(d) = app.diff.as_mut() else { return };
    let n = d.files.len();
    let block = Block::bordered()
        .title(format!(" Files · {viewed}/{n} viewed "))
        .border_style(theme::border(focused));
    let inner = block.inner(area);
    if drawer {
        f.render_widget(Clear, area);
    }
    f.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    // The explorer always marks the diff's current file.
    let sel = d.current_file().unwrap_or(0);
    let h = inner.height as usize;
    d.explorer_offset = keep_visible(sel, d.explorer_offset, h, n);
    let offset = d.explorer_offset;
    let app = &*app;
    let d = app.diff.as_ref().expect("diff open");
    let lines = file_lines(app, &d.files, sel, offset, h, inner.width as usize, focused);
    f.render_widget(Paragraph::new(lines), inner);
}
