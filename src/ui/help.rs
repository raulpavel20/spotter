//! The `?` overlay.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::theme;

const EVERYWHERE: &[(&str, &str)] = &[
    ("q / esc", "back · q on the main screen quits"),
    ("tab", "switch panel"),
    ("w / b", "jump to uncommitted / branch total"),
    ("u", "oldest commit with unviewed files"),
    ("n / p", "newer / older commit"),
    ("i", "branch total: toggle uncommitted changes"),
    ("m", "merge: remerge-diff / first-parent"),
    ("ctrl-l", "refresh and redraw"),
    (", · ?", "settings · this help"),
];

const PANELS: &[(&str, &str)] = &[
    ("j / k", "move"),
    ("g / G", "top / bottom"),
    ("enter", "timeline: files · files: open diff"),
    ("space", "files: toggle viewed"),
    ("r", "toggle all files of the target viewed"),
    ("e", "files: open in editor"),
];

const DIFF: &[(&str, &str)] = &[
    ("j/k g/G", "scroll, top/bottom · ctrl-d/u half page"),
    ("] / [", "next / previous hunk"),
    ("} / {", "next / previous file"),
    ("f · ←/tab", "file explorer · move into it (→ back)"),
    ("enter", "collapse / expand the file at the top"),
    ("space", "viewed (collapses) + next · again: unview"),
    ("h / l · W", "scroll left / right · hide whitespace changes"),
    ("e", "open editor at the first change on screen"),
];

fn section(lines: &mut Vec<Line<'static>>, title: &str, keys: &[(&str, &str)]) {
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    lines.push(Line::styled(format!(" {title}"), theme::bold()));
    for (k, what) in keys {
        lines.push(Line::from(vec![
            Span::styled(format!("   {k:<9}"), theme::sha()),
            Span::raw(what.to_string()),
        ]));
    }
}

pub fn draw(
    f: &mut Frame,
    area: Rect,
    in_diff: bool,
    border: ratatui::symbols::border::Set<'static>,
) {
    let mut lines = Vec::new();
    section(&mut lines, "Everywhere", EVERYWHERE);
    if in_diff {
        section(&mut lines, "Diff view", DIFF);
    } else {
        section(&mut lines, "Timeline and files", PANELS);
    }
    lines.push(Line::default());
    lines.push(Line::styled(
        " the review loop: u, enter, then space space space…",
        theme::dim(),
    ));
    let w = 60.min(area.width.saturating_sub(2)).max(1);
    let h = (lines.len() as u16 + 2).min(area.height);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_set(border)
                .title(" Keys · any key closes "),
        ),
        popup,
    );
}
