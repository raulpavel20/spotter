//! Key-driven behaviour that needs a real repository.

mod common;

use common::{Harness, TestRepo};
use spotter::diffview::RowKind;
use spotter::model::TargetId;

#[test]
fn huge_targets_load_files_on_demand() {
    let r = TestRepo::new();
    r.commit_file("seed.txt", "s\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    for name in ["a.txt", "b.txt", "c.txt"] {
        let body: String = (0..8_000).map(|i| format!("{name} {i}\n")).collect();
        r.write(name, body);
    }
    r.commit("three big files");
    let mut h = Harness::in_memory(&r);
    h.render(80, 24);
    h.keys("j enter enter");
    let d = h.app.diff.as_ref().unwrap();
    assert!(d.lazy, "24k lines should load lazily");
    // Large files start collapsed; expanding the first loads only it.
    h.keys("enter");
    let d = h.app.diff.as_ref().unwrap();
    assert!(d.patches[0].is_some());
    assert!(d.patches[1].is_none() && d.patches[2].is_none());
    assert!(
        d.rows
            .iter()
            .any(|r| r.file == 0 && matches!(r.kind, RowKind::Line(..)))
    );
}

#[test]
fn horizontal_scroll_shifts_text_not_gutter() {
    let r = TestRepo::new();
    r.commit_file("seed.txt", "s\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    let long = format!("{}TAIL\n", "x".repeat(100));
    r.commit_file("long.txt", &long, "long line");
    let mut h = Harness::in_memory(&r);
    h.keys("j enter enter");
    let before = h.render(60, 10);
    assert!(!before.contains("TAIL"));
    for _ in 0..8 {
        h.keys("l");
    }
    let after = h.render(60, 10);
    assert!(after.contains("TAIL"), "{after}");
    // The line number column stays put.
    assert!(
        after.lines().any(|l| l.starts_with("       1 + ")),
        "{after}"
    );
}

#[test]
fn m_toggles_merge_diff_mode() {
    let r = TestRepo::new();
    r.commit_file("shared.txt", "base\n", "init");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.commit_file("shared.txt", "feature\n", "feature edit");
    r.git(&["checkout", "-q", "main"]);
    r.commit_file("shared.txt", "main\n", "main edit");
    r.commit_file("other.txt", "from main\n", "main other");
    r.git(&["checkout", "-q", "feature"]);
    assert!(!r.git_may_fail(&["merge", "-q", "main"]));
    r.write("shared.txt", "resolved\n");
    r.commit("Merge main");
    let mut h = Harness::in_memory(&r);
    h.keys("j");
    let merge = h.app.commits()[0].sha.clone();
    assert_eq!(h.app.files_of(&TargetId::Commit(merge.clone())).len(), 1);
    h.keys("enter enter m");
    assert_eq!(h.app.files_of(&TargetId::Commit(merge.clone())).len(), 2);
    let d = h.app.diff.as_ref().unwrap();
    assert_eq!(
        d.files.len(),
        2,
        "the open diff reloads in first-parent mode"
    );
    h.keys("m");
    assert_eq!(h.app.diff.as_ref().unwrap().files.len(), 1);
}

#[test]
fn i_toggles_uncommitted_in_branch_total() {
    let r = TestRepo::new();
    r.commit_file("seed.txt", "s\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.commit_file("committed.txt", "c\n", "work");
    r.write("dirty.txt", "d\n");
    let mut h = Harness::in_memory(&r);
    assert_eq!(h.app.files_of(&TargetId::Total).len(), 2);
    h.keys("i");
    let total = h.app.files_of(&TargetId::Total);
    assert_eq!(total.len(), 1);
    assert_eq!(total[0].path, "committed.txt");
    h.keys("i");
    assert_eq!(h.app.files_of(&TargetId::Total).len(), 2);
}

#[test]
fn n_and_p_walk_commits_in_the_diff_view() {
    let r = TestRepo::new();
    r.commit_file("seed.txt", "s\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.commit_file("1.txt", "1\n", "one");
    r.commit_file("2.txt", "2\n", "two");
    let mut h = Harness::in_memory(&r);
    h.keys("j enter enter");
    let newest = h.app.commits()[0].sha.clone();
    let older = h.app.commits()[1].sha.clone();
    assert_eq!(
        h.app.diff.as_ref().unwrap().target,
        TargetId::Commit(newest.clone())
    );
    h.keys("p");
    assert_eq!(h.app.diff.as_ref().unwrap().target, TargetId::Commit(older));
    h.keys("n");
    assert_eq!(
        h.app.diff.as_ref().unwrap().target,
        TargetId::Commit(newest)
    );
    h.keys("n");
    assert_eq!(h.app.toast.as_ref().unwrap().0, "no newer commit");
}
