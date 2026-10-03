//! Settings: the screen, live application, and each new option.

mod common;

use common::{Harness, TestRepo};
use ratatui::style::Color;
use spotter::config::{SETTINGS, Value};
use spotter::diffview::RowKind;
use spotter::ui::palette::{Background, Palette};

fn index(key: &str) -> usize {
    SETTINGS.iter().position(|s| s.key == key).unwrap()
}

/// Opens the settings screen with `key` selected.
fn select(h: &mut Harness, key: &str) {
    h.keys(",");
    for _ in 0..index(key) {
        h.keys("j");
    }
    assert_eq!(h.app.settings.unwrap().sel, index(key));
}

fn one_change() -> TestRepo {
    let r = TestRepo::new();
    let body: String = (1..=30).map(|i| format!("line {i}\n")).collect();
    r.commit_file("a.txt", &body, "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.commit_file(
        "a.txt",
        &body.replace("line 15\n", "line fifteen\n"),
        "change",
    );
    r
}

fn hunk_lines(h: &Harness) -> usize {
    let d = h.app.diff.as_ref().unwrap();
    d.patches[0].as_ref().unwrap().hunks[0].lines.len()
}

#[test]
fn context_lines_apply_live_and_save() {
    let r = one_change();
    let mut h = Harness::in_memory(&r);
    h.keys("j enter enter");
    assert_eq!(hunk_lines(&h), 3 + 2 + 3);
    select(&mut h, "diff.context_lines");
    h.keys("l l");
    assert_eq!(h.app.config.context_lines, 5);
    assert_eq!(hunk_lines(&h), 5 + 2 + 5, "the open diff reloads");
    assert_eq!(
        h.saved.last().unwrap(),
        &("diff.context_lines", Some(Value::Int(5)))
    );
    // d resets to the default and removes the key from the file.
    h.keys("d");
    assert_eq!(h.app.config.context_lines, 3);
    assert_eq!(h.saved.last().unwrap(), &("diff.context_lines", None));
    h.keys("esc");
    assert!(h.app.settings.is_none());
    assert!(h.app.diff.is_some(), "back to the diff");
}

#[test]
fn background_cycles_and_flips_the_palette() {
    let r = one_change();
    let mut h = Harness::in_memory(&r);
    assert_eq!(h.app.palette.background, Background::Dark);
    select(&mut h, "theme.background");
    h.keys("l"); // auto → dark
    h.keys("l"); // dark → light
    assert_eq!(h.app.config.background, "light");
    assert_eq!(h.app.palette.background, Background::Light);
    let screen = h.render(80, 24);
    assert!(screen.contains("Terminal background"), "{screen}");
    // Syntax themes cycle through the bundled set.
    h.keys("j l");
    assert_ne!(h.app.config.syntax_theme, None);
}

#[test]
fn git_config_overrides_win_but_still_save() {
    let r = one_change();
    r.git(&["config", "spotter.contextLines", "6"]);
    let mut h = Harness::in_memory(&r);
    assert_eq!(h.app.config.context_lines, 6);
    select(&mut h, "diff.context_lines");
    h.keys("l");
    assert_eq!(h.app.config.context_lines, 6, "git config still wins");
    assert_eq!(h.saved.len(), 1);
    assert!(h.app.toast.as_ref().unwrap().0.contains("overrides"));
    let screen = h.render(100, 30);
    assert!(
        screen.contains("overridden here by git config spotter.contextlines"),
        "{screen}"
    );
}

#[test]
fn text_settings_point_to_the_file() {
    let r = one_change();
    let mut h = Harness::in_memory(&r);
    select(&mut h, "editor.command");
    h.keys("l");
    assert!(h.saved.is_empty());
    assert!(h.app.toast.as_ref().unwrap().0.contains("press e"));
}

#[test]
fn settings_screen_snapshot() {
    let r = one_change();
    let mut h = Harness::in_memory(&r);
    h.keys(",");
    insta::assert_snapshot!("settings_80x24", h.render(80, 24));
}

#[test]
fn ascii_glyphs_replace_symbols() {
    let r = one_change();
    // A line long enough to wrap, so the continuation marker shows too.
    r.commit_file("long.txt", &format!("{}\n", "word ".repeat(40)), "long");
    let mut h = Harness::configured(&r, |c| {
        c.ascii = true;
        c.wrap_lines = true;
    });
    for screen in [h.render(80, 24), {
        h.keys("j enter enter");
        h.render(120, 24)
    }] {
        let bad: Vec<char> = screen
            .chars()
            .filter(|c| "◌●◐✓Σ▸▾─│┌┐└┘├┤✕⋯↪".contains(*c))
            .collect();
        assert!(bad.is_empty(), "{bad:?} in\n{screen}");
    }
    let d = h.app.diff.as_ref().unwrap();
    assert!(d.rows.iter().any(|r| matches!(r.kind, RowKind::Wrap(..))));
}

fn long_line() -> TestRepo {
    let r = TestRepo::new();
    r.commit_file("seed.txt", "s\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.commit_file("long.txt", &format!("{}TAIL\n", "word ".repeat(30)), "long");
    r
}

fn wrapped(h: &Harness) -> bool {
    h.app.diff.as_ref().unwrap().wrap.is_some()
}

#[test]
fn wrap_setting_sets_the_start_and_wins_over_z() {
    let r = long_line();
    let mut h = Harness::configured(&r, |c| c.wrap_lines = true);
    h.render(80, 24);
    h.keys("j enter enter");
    assert!(wrapped(&h));
    assert!(h.render(80, 24).contains("TAIL"));
    h.keys("z");
    assert!(!wrapped(&h), "z turns it off for the session");
    // Changing the setting afterwards wins over z.
    select(&mut h, "diff.wrap_lines");
    h.keys("l");
    assert!(!h.app.config.wrap_lines);
    h.keys("l esc");
    assert!(h.app.config.wrap_lines);
    assert!(wrapped(&h));
    assert_eq!(
        h.saved.last().unwrap(),
        &("diff.wrap_lines", Some(Value::Bool(true)))
    );
}

#[test]
fn whitespace_setting_wins_over_w() {
    let r = one_change();
    let mut h = Harness::in_memory(&r);
    h.keys("j enter enter W");
    assert!(h.app.diff_opts().ignore_ws);
    select(&mut h, "diff.ignore_whitespace");
    h.keys("l l");
    assert!(!h.app.config.ignore_whitespace);
    assert!(
        !h.app.diff_opts().ignore_ws,
        "the setting, not the earlier W, decides"
    );
}

#[test]
fn explorer_on_open_only_on_wide_panes() {
    let r = one_change();
    let mut h = Harness::in_memory(&r);
    h.render(160, 40);
    h.keys("j enter enter");
    assert!(h.app.explorer_open);
    h.keys("esc");
    h.app.explorer_open = false;
    h.render(80, 24);
    h.keys("enter");
    assert!(!h.app.explorer_open, "no drawer by itself");
    let mut h = Harness::configured(&r, |c| c.explorer_on_open = false);
    h.render(160, 40);
    h.keys("j enter enter");
    assert!(!h.app.explorer_open);
}

fn two_files() -> TestRepo {
    let r = TestRepo::new();
    r.commit_file("seed.txt", "s\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.write("a.txt", "a\n");
    r.write("b.txt", "b\n");
    r.commit("two files");
    r.commit_file("c.txt", "c\n", "newer");
    r
}

#[test]
fn collapse_viewed_off_keeps_files_open() {
    let r = two_files();
    let mut h = Harness::configured(&r, |c| {
        c.collapse_viewed = false;
        c.explorer_on_open = false;
    });
    h.keys("j j enter enter space");
    let d = h.app.diff.as_ref().unwrap();
    assert!(h.app.is_viewed(&d.files[0]));
    assert!(!d.is_folded(0));
    // Turning it on from the screen folds it right away.
    select(&mut h, "review.collapse_viewed");
    h.keys("l esc");
    assert!(h.app.diff.as_ref().unwrap().is_folded(0));
}

#[test]
fn space_continues_off_stops_at_the_end_of_a_commit() {
    let r = two_files();
    let mut h = Harness::configured(&r, |c| c.space_continues = false);
    h.keys("j j enter enter space space");
    let d = h.app.diff.as_ref().unwrap();
    assert_eq!(d.files.len(), 2, "still on the same commit");
    assert!(
        h.app
            .toast
            .as_ref()
            .unwrap()
            .0
            .starts_with("commit reviewed")
    );
    let mut h = Harness::in_memory(&r);
    h.keys("j j enter enter space space");
    assert_eq!(
        h.app.diff.as_ref().unwrap().files.len(),
        1,
        "moved to the newer commit"
    );
}

#[test]
fn w_hides_whitespace_only_changes() {
    let r = TestRepo::new();
    r.commit_file("a.py", "def f():\n    return 1\n", "base");
    r.commit_file("b.txt", "one\n", "b");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.write("a.py", "def f():\n        return 1\n");
    r.write("b.txt", "two\n");
    r.commit("reindent and change");
    let mut h = Harness::configured(&r, |c| c.explorer_on_open = false);
    h.keys("j enter enter");
    let d = h.app.diff.as_ref().unwrap();
    assert!(!d.patches[0].as_ref().unwrap().hunks.is_empty());
    h.keys("W");
    let d = h.app.diff.as_ref().unwrap();
    assert!(d.opts.ignore_ws);
    assert!(
        d.patches[0].as_ref().unwrap().hunks.is_empty(),
        "whitespace hidden"
    );
    assert!(!d.patches[1].as_ref().unwrap().hunks.is_empty());
    assert!(
        d.rows
            .iter()
            .any(|r| r.file == 0 && r.kind == RowKind::NoChanges)
    );
    assert!(h.render(100, 20).contains("only whitespace changes"));
    // Only for this session: nothing saved.
    assert!(h.saved.is_empty());
    h.keys("W");
    assert!(
        !h.app.diff.as_ref().unwrap().patches[0]
            .as_ref()
            .unwrap()
            .hunks
            .is_empty()
    );
}

#[test]
fn line_numbers_and_word_highlights_off() {
    let r = one_change();
    let mut h = Harness::configured(&r, |c| {
        c.line_numbers = false;
        c.word_highlights = false;
        c.explorer_on_open = false;
    });
    h.keys("j enter enter");
    let screen = h.render(100, 20);
    assert!(
        screen.lines().any(|l| l.starts_with(" + line fifteen")),
        "{screen}"
    );
    let buf = h.render_buffer(100, 20);
    let pal = Palette::default();
    let y = (0..20)
        .find(|&y| {
            (0..100)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .contains("fifteen")
        })
        .unwrap();
    let x = (0..100u16)
        .find(|&x| buf[(x, y)].symbol() == "f" && buf[(x + 1, y)].symbol() == "i")
        .unwrap();
    assert_eq!(buf[(x, y)].bg, pal.plus, "no word tint");
    assert_ne!(buf[(x, y)].bg, Color::Reset);
}

#[test]
fn collapse_threshold_setting() {
    let r = TestRepo::new();
    r.commit_file("seed.txt", "s\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.commit_file("mid.txt", &"x\n".repeat(150), "150 lines");
    let h = Harness::in_memory(&r);
    assert_eq!(h.app.commits()[0].files[0].collapse, None);
    let mut h = Harness::configured(&r, |c| c.collapse_lines = 100);
    assert!(h.app.commits()[0].files[0].collapse.is_some());
    // Changing it from the screen refreshes.
    select(&mut h, "diff.collapse_lines");
    h.keys("l");
    assert_eq!(h.app.config.collapse_lines, 200);
    assert_eq!(h.app.commits()[0].files[0].collapse, None);
}

#[test]
fn layout_breakpoint_setting() {
    let r = one_change();
    let mut h = Harness::configured(&r, |c| c.wide_breakpoint = 70);
    let screen = h.render(80, 24);
    // Side by side: the timeline and files share the first panel row.
    let first = screen.lines().nth(1).unwrap();
    assert!(
        first.contains("Timeline") && first.contains("Files"),
        "{screen}"
    );
}
