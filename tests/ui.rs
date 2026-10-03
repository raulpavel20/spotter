//! UI snapshots at 80×24 and 160×48, rendered from real repos.

mod common;

use std::time::Instant;

use common::{Harness, TestRepo};

/// A small feature branch, like the README's example.
fn demo() -> TestRepo {
    let r = TestRepo::new();
    r.commit_file(
        "src/inventory.gd",
        "extends Node\n\nfunc _ready():\n\tpass\n",
        "Initial project",
    );
    r.git(&["checkout", "-q", "-b", "feature/inventory"]);
    r.commit_file(
        "src/item.gd",
        "class_name Item\nvar name := \"\"\nvar count := 0\n",
        "Data model",
    );
    let ui: String = (1..=30)
        .map(|i| format!("func slot_{i}():\n\tpass\n"))
        .collect();
    r.commit_file("src/inventory_ui.gd", &ui, "Inventory UI");
    r.write(
        "src/tooltip.gd",
        "extends Control\n\nfunc _ready():\n\tvar label = Label.new()\n\tlabel.text = item.name\n\tadd_child(label)\n",
    );
    r.write("assets/theme.tres", "[gd_resource type=\"Theme\"]\n");
    r.commit("Add tooltips");
    r.write(
        "src/tooltip.gd",
        "extends Control\n\nfunc _ready():\n\tvar label = Label.new()\n\tlabel.text = item.display_name\n\tlabel.tooltip_text = item.description\n\tadd_child(label)\n",
    );
    r.commit("Fix item drag");
    r.write(
        "src/inventory.gd",
        "extends Node\n\nfunc _ready():\n\tpass\n\n# uncommitted tweak\n",
    );
    r.write("TODO.md", "notes\n");
    r.write("Cargo.lock", "lock\n");
    r
}

#[test]
fn main_screen_narrow_and_wide() {
    let r = demo();
    let mut h = Harness::in_memory(&r);
    insta::assert_snapshot!("main_80x24", h.render(80, 24));
    h.keys("j");
    insta::assert_snapshot!("main_160x48_commit_selected", h.render(160, 48));
}

#[test]
fn diff_view_narrow_and_wide() {
    let r = demo();
    let mut h = Harness::in_memory(&r);
    // "Add tooltips": two files.
    h.keys("j j enter enter");
    assert!(h.app.diff.is_some());
    insta::assert_snapshot!("diff_80x24", h.render(80, 24));
    h.keys("}");
    insta::assert_snapshot!("diff_160x48_second_file", h.render(160, 48));
}

#[test]
fn uncommitted_diff_with_collapsed_and_untracked() {
    let r = demo();
    let mut h = Harness::in_memory(&r);
    h.keys("enter enter");
    insta::assert_snapshot!("diff_uncommitted_80x24", h.render(80, 24));
}

#[test]
fn viewed_glyphs_and_help() {
    let r = demo();
    let mut h = Harness::in_memory(&r);
    // Mark one of two files in "Add tooltips" viewed: ◐.
    h.keys("j j enter space esc");
    insta::assert_snapshot!("partly_viewed_80x24", h.render(80, 24));
    h.keys("?");
    insta::assert_snapshot!("help_80x24", h.render(80, 24));
}

#[test]
fn large_diff_scrolls_without_lag() {
    let r = TestRepo::new();
    let before: String = (0..10_000).map(|i| format!("line {i}\n")).collect();
    r.commit_file("big.txt", &before, "init");
    let after: String = (0..10_000)
        .map(|i| {
            if i % 2 == 0 {
                format!("line {i} changed\n")
            } else {
                format!("line {i}\n")
            }
        })
        .collect();
    r.commit_file("big.txt", &after, "change every other line");
    let mut h = Harness::in_memory(&r);
    h.keys("j enter enter");
    // Collapsed as large (> 1500 lines): expand it.
    h.keys("enter");
    let rows = h.app.diff.as_ref().unwrap().rows.len();
    assert!(rows > 10_000, "rows = {rows}");
    h.render(160, 48);
    let start = Instant::now();
    let frames = 200;
    for _ in 0..frames {
        h.keys("pgdn");
        h.render(160, 48);
    }
    let per_frame = start.elapsed() / frames;
    // Only visible rows are drawn, so this stays flat with diff size.
    assert!(per_frame.as_millis() < 25, "{per_frame:?} per frame");
}
