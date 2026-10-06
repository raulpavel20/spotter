//! Drawing. Everything here reads `App` and writes to the frame; the only
//! state it touches is scroll offsets that keep the cursor visible.

pub mod commit;
pub mod diff;
pub mod explorer;
pub mod files;
pub mod glyphs;
pub mod header;
pub mod help;
pub mod palette;
pub mod prompt;
pub mod settings;
pub mod tabs;
pub mod text;
pub mod theme;
pub mod timeline;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::app::App;
use crate::workspace::Workspace;

pub fn draw(f: &mut Frame, app: &mut App) {
    draw_in(f, f.area(), app);
}

/// Several repositories: the tabs, then the active one's screens.
pub fn draw_workspace(f: &mut Frame, ws: &mut Workspace) {
    let area = f.area();
    if !ws.show_bar() {
        draw_in(f, area, ws.active_app_mut());
        return;
    }
    let [strip, rest] =
        Layout::vertical([Constraint::Length(tabs::HEIGHT), Constraint::Min(0)]).areas(area);
    let gap = tabs::draw(f, strip, ws);
    draw_screen(f, rest, ws.active_app_mut(), gap);
}

/// One repository's screens, in `area`.
pub fn draw_in(f: &mut Frame, area: Rect, app: &mut App) {
    draw_screen(f, area, app, None);
}

/// `gap`: where the active tab opens into the box at the top.
fn draw_screen(f: &mut Frame, area: Rect, app: &mut App, gap: Option<(u16, u16)>) {
    if app.diff.is_some() {
        diff::draw(f, area, app, gap);
        diff::draw_drawer(f, area, app);
    } else {
        draw_main(f, area, app, gap);
    }
    // Settings open as a panel over whatever screen is showing.
    if app.settings.is_some() {
        settings::draw(f, area, app);
    }
    commit::draw(f, area, app);
    commit::draw_push(f, area, app);
    // A password prompt goes over everything.
    prompt::draw(f, area, app);
    if app.help {
        help::draw(
            f,
            area,
            app.diff.is_some(),
            app.label.is_some(),
            app.glyphs().border,
        );
    }
}

fn draw_main(f: &mut Frame, area: Rect, app: &mut App, gap: Option<(u16, u16)>) {
    // The header and banner in a box; a column of room before its right
    // border.
    let banner = header::banner_lines(app, area.width.saturating_sub(3));
    let [top, body, foot] = Layout::vertical([
        Constraint::Length(3 + banner.len() as u16),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    let inner = header::frame(f, top, app, gap);
    let [head, ban] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    header::draw(f, head, app);
    header::draw_banner(f, ban, banner);
    let wide = !app.narrow_at(area.width);
    if wide {
        let pct = app.config.timeline_width as u32;
        let tl_w = (area.width as u32 * pct / 100).clamp(30, area.width as u32 - 20) as u16;
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
    header::draw_footer(f, foot, app);
}
