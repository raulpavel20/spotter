//! Symbols used across the UI, with a plain-ASCII alternative
//! (`theme.ascii`) for fonts or terminals that render them badly.

use ratatui::symbols::border;

#[derive(Debug, Clone, Copy)]
pub struct Glyphs {
    pub uncommitted: &'static str,
    pub unviewed: &'static str,
    pub partial: &'static str,
    pub viewed: &'static str,
    pub total: &'static str,
    /// The selected row in a list.
    pub cursor: &'static str,
    /// File header markers: expanded / collapsed.
    pub open: &'static str,
    pub folded: &'static str,
    /// A horizontal rule.
    pub rule: &'static str,
    /// The timeline's Σ separator joins the border on both sides.
    pub tee_left: &'static str,
    pub tee_right: &'static str,
    pub collapsed: &'static str,
    /// The sign column of a wrapped line's continuation rows.
    pub wrap: &'static str,
    pub live: &'static str,
    pub polling: &'static str,
    pub error: &'static str,
    pub border: border::Set<'static>,
}

pub const UNICODE: Glyphs = Glyphs {
    uncommitted: "◌",
    unviewed: "●",
    partial: "◐",
    viewed: "✓",
    total: "Σ",
    cursor: "▸",
    open: "▾",
    folded: "▸",
    rule: "─",
    tee_left: "├",
    tee_right: "┤",
    collapsed: "⋯",
    wrap: "↪",
    live: "●",
    polling: "◌",
    error: "✕",
    border: border::PLAIN,
};

pub const ASCII: Glyphs = Glyphs {
    uncommitted: "o",
    unviewed: "*",
    partial: "~",
    viewed: "v",
    total: "S",
    cursor: ">",
    open: "-",
    folded: "+",
    rule: "-",
    tee_left: "+",
    tee_right: "+",
    collapsed: "...",
    wrap: ">",
    live: "*",
    polling: "o",
    error: "x",
    border: border::Set {
        top_left: "+",
        top_right: "+",
        bottom_left: "+",
        bottom_right: "+",
        vertical_left: "|",
        vertical_right: "|",
        horizontal_top: "-",
        horizontal_bottom: "-",
    },
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_set_is_ascii() {
        let g = ASCII;
        let all = [
            g.uncommitted,
            g.unviewed,
            g.partial,
            g.viewed,
            g.total,
            g.cursor,
            g.open,
            g.folded,
            g.rule,
            g.tee_left,
            g.tee_right,
            g.collapsed,
            g.wrap,
            g.live,
            g.polling,
            g.error,
            g.border.top_left,
            g.border.horizontal_top,
            g.border.vertical_left,
        ];
        assert!(all.iter().all(|s| s.is_ascii() && !s.is_empty()));
    }
}
