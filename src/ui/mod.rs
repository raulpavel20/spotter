//! Drawing. Everything here reads `App` and writes to the frame; the only
//! state it touches is scroll offsets that keep the cursor visible.

pub mod diff;
pub mod files;
pub mod header;
pub mod help;
pub mod text;
pub mod theme;
pub mod timeline;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::app::App;

/// Below this width the timeline sits above the files (PLAN §4).
pub const WIDE: u16 = 100;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    if app.diff.is_some() {
        diff::draw(f, area, app);
    } else {
        draw_main(f, area, app);
    }
    if app.help {
        help::draw(f, area, app.diff.is_some());
    }
}

fn draw_main(f: &mut Frame, area: Rect, app: &mut App) {
    let banner = header::banner_lines(app, area.width);
    let [head, ban, body, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(banner.len() as u16),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    header::draw(f, head, app);
    header::draw_banner(f, ban, banner);
    let wide = area.width >= WIDE;
    if wide {
        let tl_w = (area.width as u32 * 45 / 100).clamp(40, 90) as u16;
        let [tl, fl] =
            Layout::horizontal([Constraint::Length(tl_w), Constraint::Min(0)]).areas(body);
        timeline::draw(f, tl, app);
        files::draw(f, fl, app, true);
    } else {
        let needed = timeline::needed_height(app);
        let cap = (body.height as u32 * 55 / 100).max(6) as u16;
        let min_files = 5.min(body.height.saturating_sub(needed.min(cap)));
        let tl_h = needed.min(cap).min(body.height.saturating_sub(min_files));
        let [tl, fl] = Layout::vertical([Constraint::Length(tl_h), Constraint::Min(0)]).areas(body);
        timeline::draw(f, tl, app);
        files::draw(f, fl, app, false);
    }
    header::draw_footer(f, foot, app, wide);
}
