//! Syntax colors, changed-word tints and the explorer (diff view upgrade).

mod common;

use common::{Harness, TestRepo};
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use spotter::app::DiffFocus;
use spotter::diffview::RowKind;
use spotter::ui::palette::Palette;

/// Position of the first cell of `needle` on the row containing `row_has`.
fn find(buf: &Buffer, row_has: &str, needle: &str) -> (u16, u16) {
    let area = buf.area;
    for y in 0..area.height {
        let line: String = (0..area.width)
            .map(|x| buf[(x, y)].symbol().to_owned())
            .collect();
        if line.contains(row_has) {
            let idx = line
                .find(needle)
                .unwrap_or_else(|| panic!("{needle} not in {line:?}"));
            let col = line[..idx].chars().count() as u16;
            return (col, y);
        }
    }
    panic!("no row contains {row_has:?}");
}

fn rust_repo() -> TestRepo {
    let r = TestRepo::new();
    r.commit_file(
        "src/a.rs",
        "fn total() -> u32 {\n    let total = price * qty;\n    total\n}\n",
        "base",
    );
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.commit_file(
        "src/a.rs",
        "fn total() -> u32 {\n    let total = price * quantity;\n    total\n}\n",
        "rename qty",
    );
    r
}

#[test]
fn tints_rows_and_changed_words() {
    let r = rust_repo();
    let mut h = Harness::in_memory(&r);
    h.keys("j enter enter");
    let pal = Palette::default();
    let buf = h.render_buffer(100, 16);
    let (x, y) = find(&buf, "quantity", "quantity");
    assert_eq!(buf[(x, y)].bg, pal.plus_emph, "changed word");
    let (x, y) = find(&buf, "quantity", "price");
    assert_eq!(buf[(x, y)].bg, pal.plus, "rest of the + row");
    assert_eq!(buf[(99, y)].bg, pal.plus, "tint runs to the edge");
    let (x, y) = find(&buf, "price * qty", "qty");
    assert_eq!(buf[(x, y)].bg, pal.minus_emph);
    let (x, y) = find(&buf, "fn total", "fn");
    assert_eq!(buf[(x, y)].bg, Color::Reset, "context has no tint");
}

#[test]
fn syntax_colors_arrive_for_visible_files() {
    let r = rust_repo();
    let mut h = Harness::in_memory(&r).with_syntax();
    h.keys("j enter enter");
    let buf = h.render_buffer(100, 16);
    let (x, y) = find(&buf, "fn total", "fn");
    let keyword = buf[(x, y)].fg;
    let (x2, _) = find(&buf, "fn total", "total");
    assert_ne!(keyword, Color::Reset, "keywords are colored");
    assert_ne!(keyword, buf[(x2, y)].fg, "keyword and name differ");
    // Tints and syntax combine on changed lines.
    let (x, y) = find(&buf, "quantity", "quantity");
    assert_eq!(buf[(x, y)].bg, Palette::default().plus_emph);
    assert_ne!(buf[(x, y)].fg, Color::Reset);
}

fn three_files() -> TestRepo {
    let r = TestRepo::new();
    r.commit_file("seed.txt", "s\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    for name in ["alpha.rs", "beta.rs", "gamma.rs"] {
        let body: String = (0..30).map(|i| format!("// {name} line {i}\n")).collect();
        r.write(name, body);
    }
    r.commit("three files");
    r
}

#[test]
fn explorer_side_panel_on_wide_panes() {
    let r = three_files();
    let mut h = Harness::in_memory(&r);
    h.render(160, 40);
    // Wide panes open diffs with the explorer showing, focus on the diff.
    h.keys("j enter enter");
    assert!(h.app.explorer_open);
    assert_eq!(h.app.diff_focus, DiffFocus::Diff);
    h.keys("tab");
    assert_eq!(h.app.diff_focus, DiffFocus::Explorer);
    insta::assert_snapshot!("explorer_side_160x40", h.render(160, 40));
    // Moving in the explorer moves the diff.
    h.keys("j j");
    assert_eq!(h.app.diff.as_ref().unwrap().current_file(), Some(2));
    h.keys("enter");
    assert!(h.app.explorer_open, "the side panel stays open");
    assert_eq!(h.app.diff_focus, DiffFocus::Diff);
    // Moving in the diff moves the explorer's marker.
    h.keys("{");
    assert_eq!(h.app.diff.as_ref().unwrap().current_file(), Some(1));
    let screen = h.render(160, 40);
    assert!(screen.contains("│▸  A  beta.rs"), "{screen}");
    // Space in the explorer marks the current file viewed and folds it.
    h.keys("tab space");
    let d = h.app.diff.as_ref().unwrap();
    assert!(h.app.is_viewed(&d.files[1]));
    assert!(d.is_folded(1));
    assert_eq!(d.current_file(), Some(1));
    assert_eq!(h.app.diff_focus, DiffFocus::Explorer);
    // Enter on a folded file opens it again.
    h.keys("enter");
    assert!(!h.app.diff.as_ref().unwrap().is_folded(1));
    h.keys("f");
    assert!(!h.app.explorer_open);
    assert_eq!(h.app.diff_focus, DiffFocus::Diff);
}

#[test]
fn explorer_drawer_on_narrow_panes() {
    let r = three_files();
    let mut h = Harness::in_memory(&r);
    h.render(80, 24);
    h.keys("j enter enter f");
    insta::assert_snapshot!("explorer_drawer_80x24", h.render(80, 24));
    h.keys("G enter");
    assert_eq!(h.app.diff.as_ref().unwrap().current_file(), Some(2));
    assert!(
        !h.app.explorer_open,
        "the drawer closes after picking a file"
    );
    // Esc closes it too.
    h.keys("f esc");
    assert!(!h.app.explorer_open);
}

#[test]
fn diff_keys_still_work_from_the_explorer() {
    let r = three_files();
    r.commit_file("delta.rs", "// d\n", "fourth file");
    let mut h = Harness::in_memory(&r);
    h.render(160, 40);
    h.keys("j enter enter tab p");
    // `p` (older commit) went through to the diff, and the explorer stays.
    let d = h.app.diff.as_ref().unwrap();
    assert_eq!(d.files.len(), 3);
    assert!(h.app.explorer_open);
    assert_eq!(h.app.diff_focus, DiffFocus::Explorer);
}

#[test]
fn large_file_highlighting_is_bounded() {
    let r = TestRepo::new();
    let before: String = (0..10_000).map(|i| format!("let v{i} = {i};\n")).collect();
    r.commit_file("big.rs", &before, "init");
    let after: String = (0..10_000)
        .map(|i| {
            if i % 2 == 0 {
                format!("let v{i} = {i} + 1;\n")
            } else {
                format!("let v{i} = {i};\n")
            }
        })
        .collect();
    r.commit_file("big.rs", &after, "change");
    let mut h = Harness::in_memory(&r).with_syntax();
    h.render(160, 48);
    let start = std::time::Instant::now();
    h.keys("j enter enter enter");
    let d = h.app.diff.as_ref().unwrap();
    assert!(d.highlight(0).is_some(), "highlighted");
    assert!(start.elapsed().as_secs() < 10, "{:?}", start.elapsed());
    // Scrolling stays cheap once colors are in.
    let t = std::time::Instant::now();
    for _ in 0..100 {
        h.keys("pgdn");
        h.render(160, 48);
    }
    let per_frame = t.elapsed() / 100;
    assert!(per_frame.as_millis() < 25, "{per_frame:?} per frame");
}

#[test]
fn reloads_rerequest_highlighting() {
    let r = TestRepo::new();
    r.commit_file("a.rs", "fn a() {}\n", "base");
    r.write("a.rs", "fn a() { 1 }\n");
    let mut h = Harness::in_memory(&r).with_syntax();
    // Drop highlight replies on the floor, as a superseded request would be.
    h.keys("enter enter");
    let key = h.app.diff.as_ref().unwrap().keys[0].clone();
    h.app.diff.as_mut().unwrap().highlights.clear();
    h.app.diff.as_mut().unwrap().hl_requested.insert(key);
    r.write("b.rs", "fn b() {}\n");
    h.refresh(spotter::msg::RefreshKind::Worktree);
    let d = h.app.diff.as_ref().unwrap();
    assert_eq!(d.files.len(), 2);
    assert!(d.highlight(0).is_some(), "re-requested after the reload");
}

#[test]
fn pager_scrolling_tracks_the_file_at_the_top() {
    let r = three_files();
    let mut h = Harness::in_memory(&r);
    h.render(100, 20);
    h.keys("j enter enter");
    let d = h.app.diff.as_ref().unwrap();
    assert_eq!((d.scroll, d.current_file()), (0, Some(0)));
    // j scrolls the page (no hidden cursor).
    h.keys("j j j");
    let d = h.app.diff.as_ref().unwrap();
    assert_eq!(d.scroll, 3);
    assert_eq!(d.sticky(), Some(0));
    let screen = h.render(100, 20);
    // Below the header's box.
    let first_body_line = screen.lines().nth(3).unwrap();
    assert!(
        first_body_line.contains("alpha.rs"),
        "pinned header: {screen}"
    );
    // Scrolling into the next file switches the current file.
    h.keys("ctrl-d ctrl-d ctrl-d ctrl-d ctrl-d");
    let d = h.app.diff.as_ref().unwrap();
    assert_eq!(d.current_file(), Some(1));
    // The header, inside its box.
    assert!(
        h.render(100, 20)
            .lines()
            .nth(1)
            .unwrap()
            .contains("beta.rs (2/3)")
    );
}

#[test]
fn space_folds_viewed_files_and_enter_toggles() {
    let r = three_files();
    let mut h = Harness::configured(&r, |c| c.explorer_on_open = false);
    h.render(100, 30);
    h.keys("j enter enter space");
    let d = h.app.diff.as_ref().unwrap();
    assert!(d.is_folded(0), "viewed files collapse");
    assert_eq!(d.current_file(), Some(1), "and the view moves on");
    insta::assert_snapshot!("folded_viewed_100x30", h.render(100, 30));
    // Space on a viewed file unmarks and expands it.
    h.keys("{ space");
    let d = h.app.diff.as_ref().unwrap();
    assert!(!h.app.is_viewed(&d.files[0]));
    assert!(!d.is_folded(0));
    // Enter collapses and expands any file.
    h.keys("enter");
    assert!(h.app.diff.as_ref().unwrap().is_folded(0));
    h.keys("enter");
    assert!(!h.app.diff.as_ref().unwrap().is_folded(0));
}

#[test]
fn separators_and_hunk_spacing_render() {
    let r = TestRepo::new();
    let body: String = (1..=40).map(|i| format!("line {i}\n")).collect();
    r.commit_file("a.txt", &body, "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    let changed = body
        .replace("line 3\n", "line three\n")
        .replace("line 30\n", "line thirty\n");
    r.write("a.txt", changed);
    r.write("b.txt", "new\n");
    r.commit("two hunks and a file");
    let mut h = Harness::in_memory(&r);
    h.keys("j enter enter");
    insta::assert_snapshot!("separators_100x40", h.render(100, 40));
}

#[test]
fn arrow_keys_move_between_explorer_and_diff() {
    let r = three_files();
    let mut h = Harness::in_memory(&r);
    h.render(160, 40);
    h.keys("j enter enter");
    assert!(h.app.explorer_open);
    assert_eq!(h.app.diff_focus, DiffFocus::Diff);
    h.keys("left");
    assert_eq!(h.app.diff_focus, DiffFocus::Explorer, "← at the left edge");
    // Arrows are directional: nothing is left of the explorer.
    h.keys("left h");
    assert_eq!(
        h.app.diff_focus,
        DiffFocus::Explorer,
        "← in the explorer stays put"
    );
    h.keys("right");
    assert_eq!(h.app.diff_focus, DiffFocus::Diff);
    h.keys("left");
    assert_eq!(h.app.diff_focus, DiffFocus::Explorer, "and back again");
    // Scrolled right, ← scrolls back first.
    h.keys("right l l");
    assert_eq!(h.app.diff.as_ref().unwrap().hscroll, 16);
    h.keys("left left");
    assert_eq!(h.app.diff_focus, DiffFocus::Diff);
    assert_eq!(h.app.diff.as_ref().unwrap().hscroll, 0);
    h.keys("left");
    assert_eq!(h.app.diff_focus, DiffFocus::Explorer);
    // With the explorer hidden, ← just stays put.
    h.keys("f left");
    assert!(!h.app.explorer_open);
    assert_eq!(h.app.diff_focus, DiffFocus::Diff);
}

#[test]
fn diff_frame_shows_focus_when_the_explorer_is_open() {
    use ratatui::style::Modifier;
    let r = three_files();
    let mut h = Harness::in_memory(&r);
    h.render(160, 40);
    h.keys("j enter enter");
    let dimmed = |h: &mut Harness| {
        let buf = h.render_buffer(160, 40);
        // The rule below the diff, at the right edge.
        let rule = buf[(159, 38)].modifier.contains(Modifier::DIM);
        // The explorer's border at its top-left corner, under the header.
        (rule, buf[(0, 3)].modifier.contains(Modifier::DIM))
    };
    assert_eq!(h.app.diff_focus, DiffFocus::Diff);
    assert_eq!(
        dimmed(&mut h),
        (false, true),
        "diff focused: bright frame, dim explorer"
    );
    h.keys("tab");
    assert_eq!(
        dimmed(&mut h),
        (true, false),
        "explorer focused: the reverse"
    );
}

/// A commit with one long, changed line of Rust.
fn long_line_repo(words: usize) -> TestRepo {
    let r = TestRepo::new();
    r.commit_file("calc.rs", "fn total() -> u32 {\n    1\n}\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    let sum = vec!["price"; words].join(" + ");
    r.commit_file(
        "calc.rs",
        &format!("fn total() -> u32 {{\n    {sum} + TAIL\n}}\n"),
        "long sum",
    );
    r
}

#[test]
fn z_wraps_long_lines_for_the_session() {
    let r = long_line_repo(20);
    let mut h = Harness::in_memory(&r);
    h.render(80, 24);
    h.keys("j enter enter");
    let cut = h.render(80, 24);
    assert!(!cut.contains("TAIL"), "{cut}");
    h.keys("z");
    let wrapped = h.render(80, 24);
    insta::assert_snapshot!("wrapped_80x24", wrapped);
    assert!(wrapped.contains("TAIL"));
    // Continuation rows: blank gutter, ↪, the line's indent.
    assert!(
        wrapped
            .lines()
            .any(|l| l.starts_with("         ↪     price")),
        "{wrapped}"
    );
    // Nothing to scroll sideways.
    h.keys("l");
    assert_eq!(h.app.diff.as_ref().unwrap().hscroll, 0);
    // It lasts for the session, not just this diff, and isn't saved.
    h.keys("esc enter");
    assert!(h.app.diff.as_ref().unwrap().wrap.is_some());
    h.keys("z");
    assert!(!h.render(80, 24).contains("TAIL"));
    assert!(h.rec.saved.is_empty());
    assert!(h.render(120, 24).contains("z wrap"));
    h.keys("z");
    assert!(h.render(120, 24).contains("z no wrap"));
}

#[test]
fn wrapped_lines_reflow_with_the_explorer() {
    let r = long_line_repo(30);
    let mut h = Harness::in_memory(&r);
    h.render(160, 30);
    h.keys("j enter enter z");
    let wraps = |h: &Harness| {
        let d = h.app.diff.as_ref().unwrap();
        d.rows
            .iter()
            .filter(|r| matches!(r.kind, RowKind::Wrap(..)))
            .count()
    };
    assert!(h.app.explorer_open);
    let beside = wraps(&h);
    h.keys("f");
    assert!(!h.app.explorer_open);
    assert!(wraps(&h) < beside, "more room, fewer rows");
    assert!(h.render(160, 30).contains("TAIL"));
    // A narrower terminal wraps into more rows.
    h.render(70, 30);
    assert!(wraps(&h) > beside);
}
