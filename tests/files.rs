//! File lists and patches for every target (PLAN §3, §6.4).

mod common;

use std::collections::HashSet;

use common::{TestRepo, both, paths};
use spotter::config::Config;
use spotter::model::{Collapse, LineKind, Status, TargetId};
use spotter::refresh::{self, Cache, RefreshOpts};

fn status_of(files: &[spotter::model::FileChange], path: &str) -> Status {
    files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{path} missing"))
        .status
}

#[test]
fn uncommitted_lists_staged_unstaged_untracked_binary() {
    both(|r| {
        r.commit_file("keep.txt", "1\n2\n3\n", "init");
        r.commit_file("gone.txt", "bye\n", "add gone");
        r.write("keep.txt", "1\n2\n3\n4\n");
        r.write("staged.txt", "s\n");
        r.git(&["add", "staged.txt"]);
        r.remove("gone.txt");
        r.write("dir with space/ü n\u{12f}code.txt", "x\ny\n");
        r.write("blob.bin", [0u8, 1, 2, 3, 0, 9]);
        r.write("no-newline.txt", "a\nb");
        Box::new(|r| {
            let s = r.snapshot();
            let u = &s.uncommitted;
            assert_eq!(status_of(u, "keep.txt"), Status::Modified);
            assert_eq!(status_of(u, "staged.txt"), Status::Added);
            assert_eq!(status_of(u, "gone.txt"), Status::Deleted);
            assert_eq!(
                status_of(u, "dir with space/ü n\u{12f}code.txt"),
                Status::Untracked
            );
            let bin = u.iter().find(|f| f.path == "blob.bin").unwrap();
            assert!(bin.is_binary());
            let nn = u.iter().find(|f| f.path == "no-newline.txt").unwrap();
            assert_eq!(nn.added, Some(2));
            let keep = u.iter().find(|f| f.path == "keep.txt").unwrap();
            assert_eq!((keep.added, keep.deleted), (Some(1), Some(0)));
            // Working-tree blob ids are filled in.
            assert!(
                u.iter()
                    .filter(|f| f.status != Status::Deleted)
                    .all(|f| f.new_oid != r.repo().null_oid())
            );
        })
    });
}

#[test]
fn commit_files_with_renames_and_hostile_names() {
    both(|r| {
        r.commit_file("old name.txt", &"line\n".repeat(20), "init");
        r.git(&["mv", "old name.txt", "new name.txt"]);
        r.write("new name.txt", format!("{}extra\n", "line\n".repeat(20)));
        r.write("nl\nname.txt", "x\n");
        r.write("quote\"back\\slash.txt", "q\n");
        r.write("tab\there.txt", "t\n");
        r.commit("rename and weird names");
        Box::new(|r| {
            let s = r.snapshot();
            let files = &s.commits[0].files;
            let renamed = files.iter().find(|f| f.path == "new name.txt").unwrap();
            assert!(matches!(renamed.status, Status::Renamed(_)));
            assert_eq!(renamed.old_path.as_ref().unwrap(), "old name.txt");
            assert!(files.iter().any(|f| f.path == "nl\nname.txt"));
            assert!(files.iter().any(|f| f.path == "quote\"back\\slash.txt"));
            assert!(files.iter().any(|f| f.path == "tab\there.txt"));

            // Every file gets its patch, matched by header.
            let repo = r.repo();
            let id = TargetId::Commit(s.commits[0].sha.clone());
            let spec = s.spec(&id, &repo.empty_tree).unwrap();
            let loaded = refresh::load_patch(&repo, &spec, files, Default::default()).unwrap();
            for (f, p) in files.iter().zip(&loaded.patches) {
                let p = p.as_ref().unwrap();
                if f.path != "new name.txt" {
                    assert_eq!(p.hunks.len(), 1, "{:?}", f.path);
                }
            }
            let rp = loaded.patches[files.iter().position(|f| f.path == "new name.txt").unwrap()]
                .as_ref()
                .unwrap();
            assert!(
                rp.hunks[0]
                    .lines
                    .iter()
                    .any(|l| l.kind == LineKind::Add && l.text == "extra")
            );
        })
    });
}

#[test]
fn uncommitted_patch_includes_untracked_and_no_newline() {
    both(|r| {
        r.commit_file("a.txt", "one\ntwo\n", "init");
        r.write("a.txt", "one\nTWO");
        r.write("new.txt", "fresh\n");
        Box::new(|r| {
            let s = r.snapshot();
            let repo = r.repo();
            let spec = s.spec(&TargetId::Uncommitted, &repo.empty_tree).unwrap();
            let loaded =
                refresh::load_patch(&repo, &spec, &s.uncommitted, Default::default()).unwrap();
            let a = loaded.patches[0].as_ref().unwrap();
            let kinds: Vec<_> = a.hunks[0].lines.iter().map(|l| l.kind).collect();
            assert_eq!(
                kinds,
                [
                    LineKind::Context,
                    LineKind::Del,
                    LineKind::Add,
                    LineKind::NoNewline
                ]
            );
            let n = loaded.patches[1].as_ref().unwrap();
            assert_eq!(n.hunks[0].lines[0].text, "fresh");
        })
    });
}

#[test]
fn binary_summary_sizes() {
    let r = TestRepo::new();
    r.commit_file("img.bin", "\0\0\0\0", "init");
    r.write("img.bin", [0u8; 10]);
    r.commit("grow");
    let s = r.snapshot();
    let repo = r.repo();
    let id = TargetId::Commit(s.commits[0].sha.clone());
    let spec = s.spec(&id, &repo.empty_tree).unwrap();
    let loaded =
        refresh::load_patch(&repo, &spec, &s.commits[0].files, Default::default()).unwrap();
    let p = loaded.patches[0].as_ref().unwrap();
    assert!(p.binary);
    assert_eq!((p.old_size, p.new_size), (Some(4), Some(10)));
}

#[test]
fn collapse_rules() {
    let r = TestRepo::new();
    r.write(".gitattributes", "gen/** linguist-generated\n*.dat -diff\n");
    r.write("Cargo.lock", "lock\n");
    r.write("web/app.min.js", "x\n");
    r.write("gen/out.rs", "fn x() {}\n");
    r.write("data.dat", "d\n");
    r.write("big.txt", "x\n".repeat(1600));
    r.write("snap/a.snap", "s\n");
    r.write("src/main.rs", "fn main() {}\n");
    r.git(&["config", "spotter.collapse", "*.snap"]);
    r.commit("all");
    let s = r.snapshot();
    let c = |p: &str| {
        s.commits[0]
            .files
            .iter()
            .find(|f| f.path == p)
            .unwrap()
            .collapse
    };
    assert_eq!(c("Cargo.lock"), Some(Collapse::Lockfile));
    assert_eq!(c("web/app.min.js"), Some(Collapse::Lockfile));
    assert_eq!(c("snap/a.snap"), Some(Collapse::Lockfile));
    assert_eq!(c("gen/out.rs"), Some(Collapse::Generated));
    assert_eq!(c("data.dat"), Some(Collapse::NoDiff));
    assert_eq!(c("big.txt"), Some(Collapse::Large));
    assert_eq!(c("src/main.rs"), None);
}

#[test]
fn sha256_repository() {
    let r = TestRepo::sha256();
    r.commit_file("a.txt", "1\n", "init");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.commit_file("a.txt", "1\n2\n", "change");
    r.write("a.txt", "1\n2\n3\n");
    let repo = r.repo();
    assert_eq!(repo.empty_tree.len(), 64);
    let s = r.snapshot();
    assert_eq!(s.commits.len(), 1);
    assert_eq!(s.commits[0].sha.len(), 64);
    assert_eq!(s.uncommitted[0].new_oid.len(), 64);
    assert_ne!(s.uncommitted[0].new_oid, repo.null_oid());
}

#[test]
fn linked_worktree() {
    let r = TestRepo::new();
    r.commit_file("a.txt", "1\n", "init");
    let wt = r.tmp.path().join("wt");
    r.git(&["worktree", "add", "-q", "-b", "agent", wt.to_str().unwrap()]);
    std::fs::write(wt.join("a.txt"), "1\n2\n").unwrap();
    r.git_in(&wt, &["commit", "-qam", "agent work"]);
    std::fs::write(wt.join("b.txt"), "new\n").unwrap();
    let repo = r.repo_at(&wt);
    assert_ne!(repo.git_dir, repo.common_dir);
    assert!(repo.git_dir.starts_with(&repo.common_dir));
    assert_eq!(repo.marks_path(), r.repo().marks_path());
    let cfg = Config::load(None, &repo.git).config;
    let s = refresh::full(
        &repo,
        &cfg,
        None,
        &RefreshOpts {
            include_wt: true,
            ..Default::default()
        },
        None,
        &mut Cache::default(),
    )
    .unwrap();
    assert_eq!(s.base.branch.as_deref(), Some("agent"));
    assert_eq!(s.commits.len(), 1);
    assert_eq!(paths(&s.uncommitted), ["b.txt"]);
    assert_eq!(paths(s.total.as_ref().unwrap()), ["a.txt", "b.txt"]);
}

#[test]
fn merge_commit_remerge_and_first_parent() {
    both(|r| {
        r.commit_file("shared.txt", "base\n", "init");
        r.git(&["checkout", "-q", "-b", "feature"]);
        r.commit_file("shared.txt", "feature\n", "feature edit");
        r.git(&["checkout", "-q", "main"]);
        r.commit_file("shared.txt", "main\n", "main edit");
        r.commit_file("other.txt", "from main\n", "main other");
        r.git(&["checkout", "-q", "feature"]);
        assert!(!r.git_may_fail(&["merge", "-q", "main"]));
        r.write("shared.txt", "resolved\n");
        r.commit("Merge main into feature");
        Box::new(|r| {
            let s = r.snapshot();
            let merge = &s.commits[0];
            assert!(merge.is_merge());
            // remerge-diff: only the conflict resolution.
            assert_eq!(paths(&merge.files), ["shared.txt"]);
            let repo = r.repo();
            let cfg = Config::load(None, &repo.git).config;
            let opts = RefreshOpts {
                include_wt: true,
                first_parent: HashSet::from([merge.sha.clone()]),
            };
            let s2 = refresh::full(&repo, &cfg, None, &opts, None, &mut Cache::default()).unwrap();
            assert_eq!(paths(&s2.commits[0].files), ["other.txt", "shared.txt"]);
            // Patch of the remerge diff.
            let id = TargetId::Commit(merge.sha.clone());
            let spec = s.spec(&id, &repo.empty_tree).unwrap();
            let loaded =
                refresh::load_patch(&repo, &spec, &merge.files, Default::default()).unwrap();
            assert!(
                loaded.patches[0].as_ref().unwrap().hunks[0]
                    .lines
                    .iter()
                    .any(|l| l.kind == LineKind::Add && l.text == "resolved")
            );
        })
    });
}

#[test]
fn conflicted_files_show_as_unmerged() {
    let r = TestRepo::new();
    r.commit_file("c.txt", "base\n", "init");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.commit_file("c.txt", "feature\n", "feature");
    r.git(&["checkout", "-q", "main"]);
    r.commit_file("c.txt", "main\n", "main");
    assert!(!r.git_may_fail(&["merge", "-q", "feature"]));
    let s = r.snapshot();
    assert_eq!(paths(&s.uncommitted), ["c.txt"]);
    assert_eq!(s.uncommitted[0].status, Status::Unmerged);
    assert!(s.ops.iter().any(|o| o.to_string().contains("merge")));
}

#[test]
fn worktree_hash_matches_committed_blob() {
    let r = TestRepo::new();
    r.commit_file("a.txt", "1\n", "init");
    r.write("a.txt", "1\n2\n");
    let before = r.snapshot();
    let key = before.uncommitted[0].mark_key();
    r.commit("commit it");
    let after = r.snapshot();
    assert_eq!(after.commits[0].files[0].mark_key(), key);
}

#[test]
fn symlinks_hash_their_target_text() {
    let r = TestRepo::new();
    r.commit_file("target.txt", "content\n", "init");
    #[cfg(unix)]
    std::os::unix::fs::symlink("target.txt", r.path.join("link")).unwrap();
    let before = r.snapshot();
    let key = before
        .uncommitted
        .iter()
        .find(|f| f.path == "link")
        .unwrap()
        .clone();
    r.commit("add link");
    let after = r.snapshot();
    let committed = after.commits[0]
        .files
        .iter()
        .find(|f| f.path == "link")
        .unwrap();
    assert_eq!(committed.new_oid, key.new_oid);
}
