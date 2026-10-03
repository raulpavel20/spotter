//! Viewed marks end to end (PLAN §6.3, M5).

mod common;

use common::{Harness, TestRepo};
use spotter::app::Glyph;
use spotter::msg::RefreshKind;

fn commit_glyph(h: &Harness, i: usize) -> Glyph {
    h.app.glyph(&h.app.commits()[i].files)
}

#[test]
fn viewed_in_uncommitted_stays_viewed_after_commit() {
    let r = TestRepo::new();
    r.commit_file("a.txt", "1\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.write("a.txt", "1\n2\n");
    r.write("b.txt", "new\n");
    let mut h = Harness::new(&r);
    // ◌ → files → mark both viewed.
    h.keys("enter space j space");
    assert!(
        h.app
            .snap
            .as_ref()
            .unwrap()
            .uncommitted
            .iter()
            .all(|f| h.app.is_viewed(f))
    );
    // The agent commits with plain git.
    r.commit("agent commit");
    h.refresh(RefreshKind::Full);
    assert_eq!(h.app.commits().len(), 1);
    assert_eq!(commit_glyph(&h, 0), Glyph::All);
    assert_eq!(h.app.to_review(), 0);
    // The marks live in the shared store, so a new instance sees them.
    let h2 = Harness::new(&r);
    assert_eq!(commit_glyph(&h2, 0), Glyph::All);
}

#[test]
fn editing_a_viewed_file_unviews_it() {
    let r = TestRepo::new();
    r.commit_file("a.txt", "1\n", "base");
    r.write("a.txt", "2\n");
    let mut h = Harness::new(&r);
    h.keys("enter space");
    assert!(
        h.app
            .is_viewed(&h.app.snap.as_ref().unwrap().uncommitted[0])
    );
    r.write("a.txt", "3\n");
    h.refresh(RefreshKind::Worktree);
    assert!(
        !h.app
            .is_viewed(&h.app.snap.as_ref().unwrap().uncommitted[0])
    );
}

#[test]
fn content_preserving_rebase_keeps_marks_and_r_toggles_all() {
    let r = TestRepo::new();
    r.commit_file("base.txt", "1\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.commit_file("a.txt", "a\n", "add a");
    r.write("b.txt", "b\n");
    r.write("c.txt", "c\n");
    r.commit("add b and c");
    let mut h = Harness::new(&r);
    // `r` on the newest commit marks both files.
    h.keys("j r");
    assert_eq!(commit_glyph(&h, 0), Glyph::All);
    assert_eq!(commit_glyph(&h, 1), Glyph::None);
    assert_eq!(h.app.to_review(), 1);
    // Rebase onto a moved main that doesn't touch these files.
    r.git(&["checkout", "-q", "main"]);
    r.commit_file("other.txt", "o\n", "unrelated");
    r.git(&["checkout", "-q", "feature"]);
    r.git(&["rebase", "-q", "main"]);
    h.refresh(RefreshKind::Full);
    assert_eq!(h.app.commits()[0].subject, "add b and c");
    assert_eq!(commit_glyph(&h, 0), Glyph::All);
    // `r` again unmarks.
    h.keys("r");
    assert_eq!(commit_glyph(&h, 0), Glyph::None);
}

#[test]
fn partial_review_shows_half_glyph_and_space_advances() {
    let r = TestRepo::new();
    r.commit_file("base.txt", "1\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.write("a.txt", "a\n");
    r.write("b.txt", "b\n");
    r.write("c.txt", "c\n");
    r.commit("three files");
    let mut h = Harness::new(&r);
    h.keys("u enter space");
    assert_eq!(commit_glyph(&h, 0), Glyph::Some);
    let d = h.app.diff.as_ref().unwrap();
    assert_eq!(d.current_file(), Some(1));
    h.keys("space space");
    assert_eq!(commit_glyph(&h, 0), Glyph::All);
}

#[test]
fn linked_worktrees_share_marks() {
    let r = TestRepo::new();
    r.commit_file("a.txt", "1\n", "base");
    r.write("a.txt", "2\n");
    let mut h = Harness::new(&r);
    h.keys("enter space");
    let wt = r.tmp.path().join("wt");
    r.git(&["worktree", "add", "-q", "--detach", wt.to_str().unwrap()]);
    std::fs::write(wt.join("a.txt"), "2\n").unwrap();
    let repo = r.repo_at(&wt);
    assert_eq!(repo.marks_path(), r.repo().marks_path());
    let marks = spotter::review::Marks::load(repo.marks_path());
    let key = h.app.snap.as_ref().unwrap().uncommitted[0].mark_key();
    assert!(marks.is_viewed(&key));
}
