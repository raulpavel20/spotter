//! Live refresh: lock safety, convergence and the watcher.

mod common;

use std::fs;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::{Harness, TestRepo};
use spotter::app::Focus;
use spotter::model::TargetId;
use spotter::msg::{Msg, RefreshKind};

fn index_state(r: &TestRepo) -> (Vec<u8>, std::time::SystemTime) {
    let p = r.path.join(".git/index");
    (
        fs::read(&p).unwrap(),
        fs::metadata(&p).unwrap().modified().unwrap(),
    )
}

#[test]
fn refreshes_never_touch_the_index() {
    let r = TestRepo::new();
    for i in 0..20 {
        r.write(&format!("f{i}.txt"), format!("{i}\n"));
    }
    r.commit("init");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.commit_file("f0.txt", "changed\n", "work");
    // Make every file stat-dirty: plain `git status` would rewrite the index.
    std::thread::sleep(Duration::from_millis(1100));
    for i in 0..20 {
        r.write(
            &format!("f{i}.txt"),
            if i == 0 {
                "changed\n".to_string()
            } else {
                format!("{i}\n")
            },
        );
    }
    r.write("f3.txt", "edited\n");
    let before = index_state(&r);
    let mut h = Harness::in_memory(&r);
    for _ in 0..10 {
        h.refresh(RefreshKind::Worktree);
        h.refresh(RefreshKind::Full);
    }
    // Only the real edit shows; stat-only changes are filtered by content.
    let unc: Vec<_> = h
        .app
        .snap
        .as_ref()
        .unwrap()
        .uncommitted
        .iter()
        .map(|f| f.display_path())
        .collect();
    assert_eq!(unc, ["f3.txt"]);
    h.keys("enter enter");
    assert!(h.app.diff.is_some());
    assert_eq!(index_state(&r), before, "spotter wrote .git/index");
    assert!(!r.path.join(".git/index.lock").exists());
    // Sanity check that the scenario was meaningful: git itself rewrites it.
    r.git(&["status", "--porcelain"]);
    assert_ne!(index_state(&r).1, before.1, "index was not stat-dirty");
}

fn subjects(h: &Harness) -> Vec<String> {
    h.app.commits().iter().map(|c| c.subject.clone()).collect()
}

#[test]
fn converges_through_commit_amend_rebase_and_edits() {
    let r = TestRepo::new();
    r.commit_file("a.txt", "1\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.commit_file("b.txt", "1\n", "first");
    let mut h = Harness::in_memory(&r);
    assert_eq!(subjects(&h), ["first"]);

    // A new commit.
    r.commit_file("c.txt", "1\n", "second");
    h.refresh(RefreshKind::Full);
    assert_eq!(subjects(&h), ["second", "first"]);
    assert_eq!(h.app.toast.as_ref().unwrap().0, "+1 commit");

    // Select "second", then amend it: selection follows by subject.
    h.keys("j");
    assert_eq!(
        h.app.selected(),
        Some(TargetId::Commit(h.app.commits()[0].sha.clone()))
    );
    r.write("c.txt", "2\n");
    r.git(&["commit", "-q", "-a", "--amend", "--no-edit"]);
    h.refresh(RefreshKind::Full);
    assert_eq!(
        h.app.toast.as_ref().unwrap().0,
        "history rewritten (amend/rebase)"
    );
    let amended = r.head();
    assert_eq!(h.app.selected(), Some(TargetId::Commit(amended)));

    // Rebase onto a moved base.
    r.git(&["checkout", "-q", "main"]);
    r.commit_file("a.txt", "2\n", "base moved");
    r.git(&["checkout", "-q", "feature"]);
    h.refresh(RefreshKind::Full);
    r.git(&["rebase", "-q", "main"]);
    h.refresh(RefreshKind::Full);
    assert_eq!(subjects(&h), ["second", "first"]);
    assert_eq!(
        h.app.snap.as_ref().unwrap().base.merge_base.as_deref(),
        Some(r.git(&["rev-parse", "main"]).as_str())
    );

    // Working tree edits only need a worktree refresh.
    r.write("b.txt", "dirty\n");
    r.write("new.txt", "n\n");
    h.refresh(RefreshKind::Worktree);
    let unc: Vec<_> = h
        .app
        .snap
        .as_ref()
        .unwrap()
        .uncommitted
        .iter()
        .map(|f| f.display_path())
        .collect();
    assert_eq!(unc, ["b.txt", "new.txt"]);

    // Branch switch.
    r.git(&["stash", "-q", "-u"]);
    r.git(&["checkout", "-q", "main"]);
    h.refresh(RefreshKind::Full);
    assert_eq!(h.app.toast.as_ref().unwrap().0, "switched to main");
}

#[test]
fn open_volatile_diff_follows_the_working_tree() {
    let r = TestRepo::new();
    r.commit_file("a.txt", "1\n2\n3\n", "base");
    r.write("a.txt", "1\nTWO\n3\n");
    let mut h = Harness::in_memory(&r);
    h.keys("enter enter");
    let d = h.app.diff.as_ref().unwrap();
    assert_eq!(d.files.len(), 1);
    r.write("b.txt", "new\n");
    h.refresh(RefreshKind::Worktree);
    let d = h.app.diff.as_ref().unwrap();
    assert_eq!(d.files.len(), 2);
    assert!(d.patches.iter().all(Option::is_some));
    // The cursor stays on the same file.
    assert_eq!(d.current_file(), Some(0));
}

#[test]
fn diff_of_rewritten_commit_follows_by_subject() {
    let r = TestRepo::new();
    r.commit_file("a.txt", "1\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.commit_file("b.txt", "1\n", "work");
    let mut h = Harness::in_memory(&r);
    h.keys("j enter enter");
    r.write("b.txt", "2\n");
    r.git(&["commit", "-q", "-a", "--amend", "--no-edit"]);
    h.refresh(RefreshKind::Full);
    let d = h.app.diff.as_ref().expect("diff stays open");
    assert_eq!(d.target, TargetId::Commit(r.head()));
    assert_eq!(d.patches[0].as_ref().unwrap().hunks[0].lines[0].text, "2");
}

#[test]
fn in_progress_rebase_banner() {
    let r = TestRepo::new();
    r.commit_file("a.txt", "base\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.commit_file("a.txt", "feature\n", "f1");
    r.commit_file("b.txt", "b\n", "f2");
    r.git(&["checkout", "-q", "main"]);
    r.commit_file("a.txt", "main\n", "m1");
    r.git(&["checkout", "-q", "feature"]);
    assert!(!r.git_may_fail(&["rebase", "-q", "main"]));
    let mut h = Harness::in_memory(&r);
    let ops: Vec<String> = h
        .app
        .snap
        .as_ref()
        .unwrap()
        .ops
        .iter()
        .map(|o| o.to_string())
        .collect();
    assert_eq!(ops, ["rebase in progress (1/2)"]);
    let screen = h.render(100, 20);
    assert!(screen.contains("rebase in progress (1/2)"), "{screen}");
    assert!(screen.contains("U  a.txt"), "{screen}");
}

#[test]
fn polling_mode_refreshes_on_ticks() {
    let r = TestRepo::new();
    r.commit_file("a.txt", "1\n", "base");
    let mut h = Harness::in_memory(&r);
    h.send(Msg::Watch(spotter::msg::WatchStatus::Polling));
    r.write("a.txt", "2\n");
    for _ in 0..spotter::app::POLL_TICKS {
        h.send(Msg::Tick);
    }
    assert_eq!(h.app.snap.as_ref().unwrap().uncommitted.len(), 1);
}

#[test]
fn u_then_enter_opens_the_oldest_unviewed_file() {
    let r = TestRepo::new();
    r.commit_file("a.txt", "1\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.commit_file("b.txt", "1\n", "older");
    r.commit_file("c.txt", "1\n", "newer");
    let mut h = Harness::in_memory(&r);
    h.keys("u");
    assert_eq!(h.app.focus, Focus::Files);
    h.keys("enter");
    let d = h.app.diff.as_ref().unwrap();
    assert_eq!(d.files[0].path, "b.txt");
    // Space marks it and continues into the newer commit.
    h.keys("space");
    let d = h.app.diff.as_ref().unwrap();
    assert_eq!(d.files[0].path, "c.txt");
    h.keys("space");
    assert_eq!(h.app.to_review(), 0);
    assert_eq!(h.app.toast.as_ref().unwrap().0, "all commits reviewed ✓");
}

// --- the real watcher ------------------------------------------------------

fn expect(rx: &mpsc::Receiver<Msg>, within: Duration) -> Option<RefreshKind> {
    let deadline = Instant::now() + within;
    let mut kind = None;
    while let Ok(m) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        if let Msg::Fs(k) = m {
            kind = kind.max(Some(k));
            // Drain anything else in the same burst.
            while let Ok(Msg::Fs(k)) = rx.recv_timeout(Duration::from_millis(400)) {
                kind = kind.max(Some(k));
            }
            break;
        }
    }
    kind
}

#[test]
fn watcher_classifies_and_respects_gitignore() {
    let r = TestRepo::new();
    r.write(".gitignore", "target/\n");
    r.write("src/lib.rs", "x\n");
    r.commit("base");
    fs::create_dir_all(r.path.join("target/debug")).unwrap();
    let repo = r.repo();
    let (tx, rx) = mpsc::channel();
    let _w = spotter::watch::spawn(&repo, tx).expect("watcher starts");
    let quiet = Duration::from_millis(700);
    let soon = Duration::from_secs(3);

    // Ignored directories are not watched.
    fs::write(r.path.join("target/debug/out.o"), "bin").unwrap();
    assert_eq!(expect(&rx, quiet), None);

    // A worktree edit.
    r.write("src/lib.rs", "y\n");
    assert_eq!(expect(&rx, soon), Some(RefreshKind::Worktree));

    // A new directory gets watched, and files inside it are seen.
    fs::create_dir(r.path.join("newdir")).unwrap();
    let _ = expect(&rx, Duration::from_millis(600));
    fs::write(r.path.join("newdir/file.txt"), "z").unwrap();
    assert_eq!(expect(&rx, soon), Some(RefreshKind::Worktree));

    // A new ignored directory is not.
    fs::create_dir(r.path.join("target/release")).unwrap();
    let _ = expect(&rx, Duration::from_millis(600));
    fs::write(r.path.join("target/release/x"), "z").unwrap();
    assert_eq!(expect(&rx, quiet), None);

    // A commit moves refs: full refresh.
    r.commit("second");
    assert_eq!(expect(&rx, soon), Some(RefreshKind::Full));

    // Spotter's own reads and its marks file don't trigger anything.
    let mut h = Harness::in_memory(&r);
    h.refresh(RefreshKind::Full);
    fs::create_dir_all(r.path.join(".git/spotter")).unwrap();
    fs::write(r.path.join(".git/spotter/viewed.json"), "{}").unwrap();
    assert_eq!(expect(&rx, quiet), None);
}
