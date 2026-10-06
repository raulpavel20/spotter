//! Commit and push from Spotter (`git.actions`), and password prompts.

mod common;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;

use common::{Harness, TestRepo};
use spotter::app::Glyph;
use spotter::app::actions::CommitFocus;
use spotter::askpass::{self, PromptKind};
use spotter::git::write;
use spotter::msg::Msg;

/// A feature branch with uncommitted work: two modified files, a new
/// untracked file, and a new file the agent has staged.
fn work_in_progress() -> TestRepo {
    let r = TestRepo::new();
    r.write("a.txt", "a\n");
    r.write("b.txt", "b\n");
    r.commit("base");
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.write("a.txt", "a changed\n");
    r.write("b.txt", "b changed\n");
    r.write("c.txt", "new\n");
    r.write("d.txt", "staged by the agent\n");
    r.git(&["add", "d.txt"]);
    r
}

fn actions(r: &TestRepo) -> Harness {
    Harness::configured(r, |c| c.git_actions = true)
}

/// Marks an uncommitted file viewed, as `space` would.
fn view(h: &mut Harness, path: &str) {
    let f = h
        .app
        .snap
        .as_ref()
        .unwrap()
        .uncommitted
        .iter()
        .find(|f| f.path == path)
        .unwrap()
        .clone();
    h.app.marks.set(f.mark_key(), true, 0);
    h.app.sync_viewed();
}

fn toast(h: &Harness) -> String {
    h.app
        .toast
        .as_ref()
        .map(|t| t.0.clone())
        .unwrap_or_default()
}

#[test]
fn off_by_default() {
    let r = work_in_progress();
    let mut h = Harness::in_memory(&r);
    h.keys("c");
    assert!(h.app.commit.is_none());
    assert!(toast(&h).contains("turn them on in settings"));
    h.keys("P");
    assert!(h.app.push.is_none());
    assert!(!h.render(100, 24).contains("c commit"));
}

#[test]
fn commits_exactly_the_viewed_files() {
    let r = work_in_progress();
    let mut h = actions(&r);
    view(&mut h, "a.txt");
    view(&mut h, "c.txt");
    assert!(h.render(120, 24).contains("c commit"));
    h.keys("c");
    let p = h.app.commit.as_ref().unwrap();
    let picked: Vec<String> = p
        .items
        .iter()
        .filter(|i| i.picked)
        .map(|i| i.file.path.to_string())
        .collect();
    assert_eq!(picked, ["a.txt", "c.txt"]);
    assert_eq!(p.focus, CommitFocus::Summary, "ready to type");
    insta::assert_snapshot!("commit_80x24", h.render(80, 24));
    h.type_text("Add c and fix a");
    h.keys("enter");
    assert!(h.app.commit.is_none());
    assert!(toast(&h).starts_with("committed "), "{}", toast(&h));
    assert_eq!(
        r.git(&["show", "--name-only", "--format=%s", "HEAD"]),
        "Add c and fix a\n\na.txt\nc.txt"
    );
    // b stays modified, and what the agent staged stays staged.
    assert_eq!(r.git(&["status", "--porcelain"]), " M b.txt\nA  d.txt");
    // Viewed marks follow the content into the new commit.
    let newest = &h.app.commits()[0];
    assert_eq!(newest.subject, "Add c and fix a");
    assert_eq!(h.app.glyph(&newest.files), Glyph::All);
}

#[test]
fn picking_files_and_writing_a_body() {
    let r = work_in_progress();
    let mut h = actions(&r);
    h.keys("c");
    // Nothing viewed: the list has focus.
    assert_eq!(h.app.commit.as_ref().unwrap().focus, CommitFocus::Files);
    h.keys("enter");
    assert!(h.app.commit.as_ref().unwrap().error.is_none());
    h.type_text("Change b");
    h.keys("enter");
    let err = h.app.commit.as_ref().unwrap().error.clone().unwrap();
    assert!(err.contains("pick at least one file"), "{err}");
    // Back to the list: pick b only.
    h.keys("backtab j space tab tab");
    h.type_text("First line.\nSecond line.");
    h.keys("backtab enter");
    assert!(h.app.commit.is_none(), "{:?}", h.app.commit);
    assert_eq!(
        r.git(&["log", "-1", "--format=%B"]),
        "Change b\n\nFirst line.\nSecond line."
    );
    assert_eq!(
        r.git(&["show", "--name-only", "--format=", "HEAD"]),
        "b.txt"
    );
}

#[test]
fn editor_and_drafts() {
    let r = work_in_progress();
    let mut h = actions(&r);
    h.keys("c enter");
    h.type_text("Draft");
    h.keys("ctrl-e");
    assert_eq!(h.rec.messages, ["Draft\n"]);
    h.send(Msg::MessageEdited(
        "\nFrom the editor\n\nWith a body.\n".into(),
    ));
    let p = h.app.commit.as_ref().unwrap();
    assert_eq!(p.summary.text, "From the editor");
    assert_eq!(p.body.text, "With a body.");
    // Closing keeps the message for next time.
    h.keys("esc c");
    assert_eq!(
        h.app.commit.as_ref().unwrap().summary.text,
        "From the editor"
    );
    // A paste can't press enter.
    h.keys("esc c tab");
    h.send(Msg::Paste("one\ntwo".into()));
    let p = h.app.commit.as_ref().unwrap();
    assert_eq!(p.focus, CommitFocus::Summary);
    assert_eq!(p.summary.text, "From the editorone two");
}

#[test]
fn a_failing_hook_keeps_the_panel_and_untracked_files() {
    let r = work_in_progress();
    let hook = r.path.join(".git/hooks/pre-commit");
    std::fs::write(
        &hook,
        "#!/bin/sh\necho 'lint: c.txt has no license header' >&2\nexit 1\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut h = actions(&r);
    view(&mut h, "c.txt");
    h.keys("c");
    h.type_text("Add c");
    h.keys("enter");
    let p = h.app.commit.as_ref().unwrap();
    assert!(!p.busy);
    let err = p.error.clone().unwrap();
    assert!(err.contains("no license header"), "{err}");
    assert!(h.render(80, 24).contains("no license header"));
    // c.txt was added for the commit; it is untracked again.
    assert!(r.git(&["status", "--porcelain"]).contains("?? c.txt"));
    let p = h.app.commit.as_ref().unwrap();
    assert_eq!(p.summary.text, "Add c", "the message is kept");
}

#[test]
fn refuses_while_git_is_busy_or_mid_merge() {
    let r = work_in_progress();
    let mut h = actions(&r);
    view(&mut h, "a.txt");
    std::fs::write(r.path.join(".git/index.lock"), "").unwrap();
    h.keys("c");
    h.type_text("Fix a");
    h.keys("enter");
    let err = h.app.commit.as_ref().unwrap().error.clone().unwrap();
    assert!(err.contains("git is busy"), "{err}");
    std::fs::remove_file(r.path.join(".git/index.lock")).unwrap();
    h.keys("esc");
    // A merge in progress: finish it in git.
    let head = r.head();
    std::fs::write(r.path.join(".git/MERGE_HEAD"), format!("{head}\n")).unwrap();
    h.refresh(spotter::msg::RefreshKind::Full);
    h.keys("c");
    assert!(h.app.commit.is_none());
    assert!(toast(&h).contains("merge in progress"), "{}", toast(&h));
}

#[test]
fn nothing_to_commit() {
    let r = TestRepo::new();
    r.commit_file("a.txt", "a\n", "base");
    let mut h = actions(&r);
    h.keys("c");
    assert!(h.app.commit.is_none());
    assert_eq!(toast(&h), "nothing to commit");
}

/// `main` pushed to a bare `origin` and tracking it, then two new commits.
fn ahead_of_origin() -> (TestRepo, std::path::PathBuf) {
    let r = TestRepo::new();
    r.commit_file("a.txt", "a\n", "base");
    let bare = r.add_remote("origin");
    r.git(&["branch", "-q", "--set-upstream-to=origin/main"]);
    r.commit_file("a.txt", "a2\n", "second");
    r.commit_file("a.txt", "a3\n", "third");
    (r, bare)
}

fn remote_head(r: &TestRepo, bare: &Path, branch: &str) -> String {
    r.git_in(bare, &["rev-parse", branch])
}

#[test]
fn push_sends_the_branch_to_its_upstream() {
    let (r, bare) = ahead_of_origin();
    let mut h = actions(&r);
    h.keys("P");
    let p = h.app.push.as_ref().unwrap();
    assert_eq!((p.ahead, p.upstream.as_deref()), (2, Some("origin/main")));
    let screen = h.render(80, 24);
    insta::assert_snapshot!("push_80x24", screen);
    h.keys("enter");
    assert!(h.app.push.is_none());
    assert_eq!(toast(&h), "pushed to origin/main");
    assert_eq!(remote_head(&r, &bare, "main"), r.head());
    // Again: nothing left to push.
    h.keys("P");
    assert!(h.app.push.is_none());
    assert!(toast(&h).starts_with("nothing to push"), "{}", toast(&h));
}

#[test]
fn push_publishes_a_new_branch() {
    let (r, bare) = ahead_of_origin();
    r.git(&["checkout", "-q", "-b", "feature/x"]);
    r.commit_file("b.txt", "b\n", "feature work");
    let mut h = actions(&r);
    h.keys("P");
    let p = h.app.push.as_ref().unwrap();
    assert_eq!(p.upstream, None);
    assert!(h.render(80, 24).contains("Publish feature/x to origin?"));
    h.keys("enter");
    assert_eq!(toast(&h), "published feature/x to origin");
    assert_eq!(remote_head(&r, &bare, "feature/x"), r.head());
    assert_eq!(r.git(&["config", "branch.feature/x.remote"]), "origin");
}

#[test]
fn rejected_pushes_are_explained() {
    let (r, bare) = ahead_of_origin();
    // Someone else pushes first.
    let other = r.tmp.path().join("other");
    r.git_in(
        r.tmp.path(),
        &[
            "clone",
            "-q",
            bare.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    r.git_in(&other, &["commit", "-q", "--allow-empty", "-m", "theirs"]);
    r.git_in(&other, &["push", "-q", "origin", "main"]);
    let mut h = actions(&r);
    h.keys("P enter");
    let err = h.app.push.as_ref().unwrap().error.clone().unwrap();
    assert!(
        err.contains("the remote has commits you don't have"),
        "{err}"
    );
    h.keys("enter");
    assert!(h.app.push.is_none());
}

#[test]
fn push_needs_a_branch_and_a_remote() {
    let r = TestRepo::new();
    r.commit_file("a.txt", "a\n", "base");
    let mut h = actions(&r);
    h.keys("P");
    assert!(toast(&h).starts_with("no remote"), "{}", toast(&h));
    r.git(&["checkout", "-q", "--detach"]);
    h.keys("P");
    assert!(toast(&h).contains("detached HEAD"), "{}", toast(&h));
}

#[test]
fn gpg_signing_runs_in_the_foreground() {
    let r = TestRepo::new();
    let repo = r.repo();
    assert!(!write::signs_with_gpg(&repo));
    r.git(&["config", "commit.gpgsign", "true"]);
    assert!(write::signs_with_gpg(&repo));
    r.git(&["config", "gpg.format", "ssh"]);
    assert!(!write::signs_with_gpg(&repo), "ssh signing asks the helper");
}

#[test]
fn prompts_are_answered_from_the_panel() {
    let r = work_in_progress();
    let mut h = actions(&r);
    h.send(Msg::AskPass {
        id: 7,
        prompt: "Password for 'https://alice@example.com':".into(),
        kind: PromptKind::Secret,
    });
    h.type_text("hunter2");
    let screen = h.render(80, 24);
    assert!(
        screen.contains("Password for 'https://alice@example.com':"),
        "{screen}"
    );
    assert!(
        screen.contains("•••••••") && !screen.contains("hunter2"),
        "{screen}"
    );
    // Keys go to the prompt, not the screen underneath.
    h.keys("enter");
    assert_eq!(h.rec.answers, [(7, Some("hunter2".to_owned()))]);
    h.send(Msg::AskPass {
        id: 8,
        prompt: "Username for 'https://example.com':".into(),
        kind: PromptKind::Visible,
    });
    h.type_text("alice");
    assert!(h.render(80, 24).contains("alice"));
    h.keys("esc");
    h.send(Msg::AskPass {
        id: 9,
        prompt: "Allow use of key?".into(),
        kind: PromptKind::Confirm,
    });
    h.keys("enter");
    assert_eq!(h.rec.answers[1..], [(8, None), (9, Some("yes".to_owned()))]);
    assert!(h.app.prompts.is_empty());
}

/// git asks for credentials through the spotter binary, which hands the
/// prompts to the listening TUI side and passes the answers back.
#[test]
fn askpass_round_trip_through_git() {
    let r = TestRepo::new();
    let (tx, rx) = mpsc::channel();
    let server = askpass::Server::start(tx).unwrap();
    let mut env = askpass::env(Path::new(env!("CARGO_BIN_EXE_spotter")), server.socket());
    env.extend(r.env());
    let mut git = Command::new("git")
        .args(["credential", "fill"])
        .current_dir(&r.path)
        .envs(env)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    git.stdin
        .take()
        .unwrap()
        .write_all(b"protocol=https\nhost=example.com\n\n")
        .unwrap();
    let mut prompts = Vec::new();
    for _ in 0..2 {
        let Ok(Msg::AskPass { id, prompt, kind }) = rx.recv() else {
            panic!("no prompt");
        };
        let answer = match kind {
            PromptKind::Visible => "alice",
            _ => "s3cret",
        };
        server.answer(id, Some(answer.into()));
        prompts.push(prompt);
    }
    let out = git.wait_with_output().unwrap();
    let out = String::from_utf8_lossy(&out.stdout);
    assert!(
        prompts[0].starts_with("Username for 'https://example.com'"),
        "{prompts:?}"
    );
    assert!(
        prompts[1].starts_with("Password for 'https://alice@example.com'"),
        "{prompts:?}"
    );
    assert!(
        out.contains("username=alice\n") && out.contains("password=s3cret\n"),
        "{out}"
    );
}

#[test]
fn renames_commit_both_paths() {
    let r = TestRepo::new();
    r.write("old name.txt", "same content\n");
    r.write("other.txt", "x\n");
    r.commit("base");
    r.git(&["mv", "old name.txt", "new name.txt"]);
    r.write("other.txt", "y\n");
    let mut h = actions(&r);
    h.keys("c space");
    let p = h.app.commit.as_ref().unwrap();
    let picked: Vec<String> = p
        .items
        .iter()
        .filter(|i| i.picked)
        .map(|i| i.file.path.to_string())
        .collect();
    assert_eq!(picked, ["new name.txt"], "{:?}", p.items);
    h.keys("tab");
    h.type_text("Rename");
    h.keys("enter");
    assert!(h.app.commit.is_none(), "{:?}", h.app.commit);
    assert_eq!(
        r.git(&["show", "-M", "--name-status", "--format=", "HEAD"]),
        "R100\told name.txt\tnew name.txt"
    );
    assert_eq!(r.git(&["status", "--porcelain"]), " M other.txt");
}
