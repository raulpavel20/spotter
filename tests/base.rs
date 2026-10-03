//! Base resolution scenarios (PLAN §6.1), each under clean and hostile config.

mod common;

use common::{TestRepo, both};
use spotter::git::base::{BaseMode, SANITY_LIMIT};
use spotter::git::log::LogRange;

fn subjects(r: &TestRepo) -> Vec<String> {
    r.snapshot()
        .commits
        .iter()
        .map(|c| c.subject.clone())
        .collect()
}

#[test]
fn feature_branch() {
    both(|r| {
        r.commit_file("a.txt", "a\n", "main 1");
        let mb = r.commit_file("a.txt", "a\nb\n", "main 2");
        r.git(&["checkout", "-q", "-b", "feature/inventory"]);
        r.commit_file("b.txt", "b\n", "feat 1");
        r.commit_file("c.txt", "c\n", "feat 2");
        r.git(&["checkout", "-q", "main"]);
        r.commit_file("a.txt", "a\nb\nc\n", "main 3 after branching");
        r.git(&["checkout", "-q", "feature/inventory"]);
        Box::new(move |r| {
            let s = r.snapshot();
            assert_eq!(s.base.mode, BaseMode::Feature);
            assert_eq!(s.base.branch.as_deref(), Some("feature/inventory"));
            assert_eq!(s.base.base_ref.as_deref(), Some("main"));
            assert_eq!(s.base.merge_base.as_deref(), Some(mb.as_str()));
            assert_eq!(s.base.count, 2);
            assert_eq!(subjects(r), ["feat 2", "feat 1"]);
            let total = s.total.as_ref().unwrap();
            assert_eq!(common::paths(total), ["b.txt", "c.txt"]);
        })
    });
}

#[test]
fn stale_local_main_prefers_closer_origin_main() {
    both(|r| {
        r.commit_file("a.txt", "1\n", "c1");
        r.add_remote("origin");
        // origin/main moves ahead of local main.
        r.git(&["checkout", "-q", "-b", "tmp"]);
        r.commit_file("a.txt", "1\n2\n", "c2");
        r.commit_file("a.txt", "1\n2\n3\n", "c3");
        r.git(&["push", "-q", "origin", "HEAD:main"]);
        r.git(&["fetch", "-q", "origin"]);
        r.git(&["checkout", "-q", "-b", "feature", "origin/main"]);
        r.git(&["branch", "-q", "-D", "tmp"]);
        r.commit_file("f.txt", "f\n", "feature work");
        Box::new(|r| {
            let s = r.snapshot();
            assert_eq!(s.base.mode, BaseMode::Feature);
            assert_eq!(s.base.base_ref.as_deref(), Some("origin/main"));
            assert_eq!(s.base.count, 1);
            assert_eq!(subjects(r), ["feature work"]);
        })
    });
}

#[test]
fn trunk_mode_with_upstream_shows_unpushed() {
    both(|r| {
        r.commit_file("a.txt", "1\n", "pushed");
        r.add_remote("origin");
        r.git(&["branch", "-q", "--set-upstream-to=origin/main"]);
        r.commit_file("a.txt", "1\n2\n", "unpushed 1");
        r.commit_file("a.txt", "1\n2\n3\n", "unpushed 2");
        Box::new(|r| {
            let s = r.snapshot();
            assert_eq!(s.base.mode, BaseMode::TrunkUpstream);
            assert_eq!(s.base.base_ref.as_deref(), Some("origin/main"));
            assert_eq!(s.base.count, 2);
            assert!(s.base.notes.iter().any(|n| n.contains("trunk mode")));
            assert!(s.base.hint.is_none());
            assert_eq!(subjects(r), ["unpushed 2", "unpushed 1"]);
        })
    });
}

#[test]
fn trunk_mode_nothing_unpushed_has_hint() {
    both(|r| {
        r.commit_file("a.txt", "1\n", "pushed");
        r.add_remote("origin");
        r.git(&["branch", "-q", "--set-upstream-to=origin/main"]);
        r.write("a.txt", "1\nlocal edit\n");
        Box::new(|r| {
            let s = r.snapshot();
            assert_eq!(s.base.mode, BaseMode::TrunkUpstream);
            assert!(s.commits.is_empty());
            assert!(s.base.hint.as_deref().unwrap().contains("nothing unpushed"));
            assert_eq!(common::paths(&s.uncommitted), ["a.txt"]);
        })
    });
}

#[test]
fn trunk_mode_without_upstream_shows_recent() {
    both(|r| {
        r.commit_file("a.txt", "0\n", "c0");
        r.empty_commits(24, "c");
        Box::new(|r| {
            let s = r.snapshot();
            assert_eq!(s.base.mode, BaseMode::TrunkRecent);
            assert_eq!(s.base.range, LogRange::Last(20));
            assert_eq!(s.commits.len(), 20);
            let expected = r.git(&["rev-parse", "HEAD~20"]);
            assert_eq!(s.base.merge_base.as_deref(), Some(expected.as_str()));
        })
    });
}

#[test]
fn trunk_without_upstream_short_history_starts_at_empty_tree() {
    both(|r| {
        r.commit_file("a.txt", "0\n", "only");
        Box::new(|r| {
            let s = r.snapshot();
            assert_eq!(s.base.mode, BaseMode::TrunkRecent);
            assert_eq!(s.commits.len(), 1);
            assert_eq!(
                s.base.merge_base.as_deref(),
                Some(r.repo().empty_tree.as_str())
            );
            assert_eq!(common::paths(s.total.as_ref().unwrap()), ["a.txt"]);
            assert_eq!(common::paths(&s.commits[0].files), ["a.txt"]);
        })
    });
}

#[test]
fn detached_head_resolves_like_a_feature_branch() {
    both(|r| {
        r.commit_file("a.txt", "1\n", "main");
        r.git(&["checkout", "-q", "-b", "feature"]);
        r.commit_file("b.txt", "1\n", "f1");
        r.commit_file("b.txt", "2\n", "f2");
        r.git(&["checkout", "-q", "--detach", "HEAD~1"]);
        Box::new(|r| {
            let s = r.snapshot();
            assert_eq!(s.base.mode, BaseMode::Feature);
            assert_eq!(s.base.branch, None);
            assert_eq!(s.base.base_ref.as_deref(), Some("main"));
            assert_eq!(subjects(r), ["f1"]);
        })
    });
}

#[test]
fn unborn_repo_has_only_uncommitted() {
    both(|r| {
        r.write("new.txt", "hello\nworld\n");
        r.write("staged.txt", "s\n");
        r.git(&["add", "staged.txt"]);
        Box::new(|r| {
            let s = r.snapshot();
            assert_eq!(s.base.mode, BaseMode::Unborn);
            assert!(s.commits.is_empty());
            assert!(s.total.is_none());
            assert_eq!(common::paths(&s.uncommitted), ["staged.txt", "new.txt"]);
            assert_eq!(s.uncommitted[1].added, Some(2));
        })
    });
}

#[test]
fn sanity_guard_truncates_long_ranges() {
    let r = TestRepo::new();
    r.commit_file("a.txt", "1\n", "base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.empty_commits(SANITY_LIMIT + 5, "work");
    let s = r.snapshot();
    assert_eq!(s.base.count, SANITY_LIMIT + 5);
    assert!(s.base.truncated());
    assert_eq!(s.commits.len(), SANITY_LIMIT);
    assert!(
        s.base
            .notes
            .iter()
            .any(|n| n.contains("305 commits behind"))
    );
}

#[test]
fn single_non_origin_remote_provides_default_branch() {
    both(|r| {
        r.git(&["checkout", "-q", "-b", "trunk"]);
        r.commit_file("a.txt", "1\n", "c1");
        r.add_remote("upstream");
        r.git(&["checkout", "-q", "-b", "feature"]);
        r.commit_file("b.txt", "1\n", "f1");
        Box::new(|r| {
            let s = r.snapshot();
            assert_eq!(s.base.mode, BaseMode::Feature);
            assert_eq!(s.base.base_ref.as_deref(), Some("upstream/trunk"));
            assert_eq!(subjects(r), ["f1"]);
        })
    });
}

#[test]
fn explicit_base_overrides() {
    both(|r| {
        r.commit_file("a.txt", "1\n", "c1");
        r.commit_file("a.txt", "2\n", "c2");
        r.commit_file("a.txt", "3\n", "c3");
        Box::new(|r| {
            let s = r.snapshot_with(Some("HEAD~2"), true);
            assert_eq!(s.base.mode, BaseMode::Explicit);
            assert_eq!(s.commits.len(), 2);
            let s = r.snapshot_with(Some("does-not-exist"), true);
            assert_eq!(s.base.mode, BaseMode::TrunkRecent);
            assert!(s.base.notes.iter().any(|n| n.contains("not found")));
        })
    });
}

#[test]
fn no_base_branch_falls_back_to_recent() {
    both(|r| {
        r.git(&["checkout", "-q", "-b", "lonely"]);
        r.commit_file("a.txt", "1\n", "c1");
        Box::new(|r| {
            let s = r.snapshot();
            assert_eq!(s.base.mode, BaseMode::NoBase);
            assert_eq!(s.commits.len(), 1);
        })
    });
}
