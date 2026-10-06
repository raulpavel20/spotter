//! Several repositories in tabs: opening a folder of repositories,
//! switching, and what reaches you from the tabs you aren't looking at.

mod common;

use std::path::PathBuf;

use common::{Harness, TestRepo, WsHarness};
use spotter::askpass::PromptKind;
use spotter::config::{Config, SETTINGS};
use spotter::msg::{Effect, Msg, RefreshKind, TabId};

/// `portal/` with three services, and folders that must not count.
fn portal() -> (TestRepo, TestRepo, TestRepo, PathBuf) {
    let api = TestRepo::at("portal/api");
    api.commit_file("src/main.rs", "fn main() {}\n", "Initial api");
    api.git(&["checkout", "-q", "-b", "feature/upload"]);
    api.commit_file("src/upload.rs", "pub fn upload() {}\n", "Add upload");
    api.commit_file("src/queue.rs", "pub fn queue() {}\n", "Add queue");
    api.write("src/main.rs", "fn main() { upload(); }\n");
    let web = api.sibling("portal/web");
    web.commit_file("index.html", "<h1>hi</h1>\n", "Initial web");
    web.add_remote("origin");
    web.git(&["branch", "--set-upstream-to=origin/main"]);
    web.commit_file("index.html", "<h1>hello</h1>\n", "Say hello");
    let svc = api.sibling("portal/group/svc");
    svc.commit_file("svc.py", "print('svc')\n", "Initial svc");
    api.sibling("portal/node_modules/pkg");
    api.sibling("portal/.hidden/repo");
    let root = api.tmp.path().join("portal");
    (api, web, svc, root)
}

fn names(h: &WsHarness) -> Vec<&str> {
    h.ws.tabs.iter().map(|t| t.name.as_str()).collect()
}

fn setting_index(key: &str) -> usize {
    SETTINGS.iter().position(|s| s.key == key).unwrap()
}

#[test]
fn opens_every_repository_in_a_folder() {
    let (api, _web, _svc, root) = portal();
    let mut h = WsHarness::open(&api, &[&root]);
    assert_eq!(names(&h), ["api", "svc", "web"]);
    assert_eq!(h.ws.title.as_deref(), Some("portal"));
    // Nothing is "new" on the first look.
    assert!(h.ws.tabs.iter().all(|t| !t.activity));
    let s = |h: &WsHarness, i| h.ws.summary(i);
    assert!(s(&h, 0).dirty && s(&h, 0).to_review == 2);
    // Trunk history isn't counted as work to review; ahead of upstream is.
    assert!(s(&h, 1).quiet());
    assert_eq!(s(&h, 2).to_review, 1);
    insta::assert_snapshot!("tabs_80x24", h.render(80, 24));
    // The bar takes a row from the repository's screen.
    assert_eq!(h.app(0).size, (80, 23));
    assert_eq!(h.app(2).size, (80, 23));
}

#[test]
fn a_single_repository_inside_opens_directly() {
    let api = TestRepo::at("portal/api");
    api.commit_file("a.txt", "a\n", "First");
    api.write("a.txt", "b\n");
    let root = api.tmp.path().join("portal");
    let mut h = WsHarness::open(&api, &[&root]);
    assert!(!h.ws.show_bar());
    assert_eq!(
        h.app(0).toast.as_ref().map(|t| t.0.as_str()),
        Some("opened api, the only repository in portal")
    );
    assert_eq!(h.app(0).label, None);
    h.ws.active_app_mut().toast = None;
    // Exactly what Spotter shows for the repository itself.
    let mut single = Harness::in_memory(&api);
    assert_eq!(h.render(80, 24), single.render(80, 24));
}

#[test]
fn several_paths_open_as_tabs() {
    let (api, web, _svc, _) = portal();
    // The same repository twice is one tab; order is kept.
    let mut h = WsHarness::open(&api, &[&web.path, &api.path, &api.path.join("src")]);
    assert_eq!(names(&h), ["web", "api"]);
    assert_eq!(h.ws.title, None);
    assert!(h.render(80, 24).starts_with(" 1 web 1 │ 2 api"));
}

#[test]
fn keys_switch_tabs() {
    let (api, _web, _svc, root) = portal();
    let mut h = WsHarness::open(&api, &[&root]);
    let active = |h: &WsHarness| h.ws.active;
    h.keys(">");
    assert_eq!(active(&h), 1);
    h.keys("> >");
    assert_eq!(active(&h), 0, "wraps around");
    h.keys("<");
    assert_eq!(active(&h), 2);
    h.keys("1");
    assert_eq!(active(&h), 0);
    h.keys("ctrl-pgdn");
    assert_eq!(active(&h), 1);
    h.keys("9");
    assert_eq!(active(&h), 1);
    assert_eq!(
        h.app(1).toast.as_ref().map(|t| t.0.as_str()),
        Some("there are only 3 repositories")
    );
    // Help takes the key first: it only closes.
    h.keys("? >");
    assert_eq!(active(&h), 1);
    // Each tab keeps its own place.
    h.keys("1 j j");
    assert_eq!(h.app(0).sel, 2);
    h.keys("3 1");
    assert_eq!(h.app(0).sel, 2);
}

#[test]
fn keys_are_text_while_typing() {
    let (api, _web, _svc, root) = portal();
    api.git(&["config", "spotter.gitActions", "true"]);
    let mut h = WsHarness::open(&api, &[&root]);
    h.keys("c");
    assert!(h.app(0).commit.is_some());
    // Nothing viewed: the file list has focus; tab to the summary.
    h.keys("tab");
    h.type_text("a > b 2");
    assert_eq!(h.ws.active, 0);
    let p = h.app(0).commit.as_ref().unwrap();
    assert_eq!(p.summary.text, "a > b 2");
}

#[test]
fn changes_in_other_tabs_only_mark_them() {
    let (api, web, _svc, root) = portal();
    let mut h = WsHarness::open(&api, &[&root]);
    web.write("index.html", "<h1>changed</h1>\n");
    h.refresh(2, RefreshKind::Worktree);
    assert!(h.ws.tabs[2].activity);
    assert!(!h.ws.tabs[0].activity && !h.ws.tabs[1].activity);
    // No toast about another repository's files.
    assert_eq!(h.app(0).toast, None);
    let bar = h.render_buffer(80, 24);
    let top = common::buffer_text(&bar);
    let web_x = top[..top.find("web").unwrap()].chars().count() as u16;
    assert!(
        bar[(web_x, 0)]
            .modifier
            .contains(ratatui::style::Modifier::BOLD)
    );
    // Changes in the tab you look at don't mark it.
    api.write("src/queue.rs", "pub fn queue() { todo!() }\n");
    h.refresh(0, RefreshKind::Worktree);
    assert!(!h.ws.tabs[0].activity);
    // Looking clears it.
    h.keys("3");
    assert!(!h.ws.tabs[2].activity);
    h.keys("1");
    assert!(!h.ws.tabs[2].activity);
}

#[test]
fn commit_results_reach_you_in_another_tab() {
    let (api, web, _svc, root) = portal();
    web.git(&["config", "spotter.gitActions", "true"]);
    web.write("index.html", "<h1>changed</h1>\n");
    let mut h = WsHarness::open(&api, &[&root]);
    h.keys("3 c tab");
    h.type_text("Change the title");
    // The commit runs (busy); you can leave meanwhile.
    h.ws.tabs[2].app.commit.as_mut().unwrap().busy = true;
    h.keys("1");
    assert_eq!(h.ws.active, 0);
    h.send_to(2, Msg::Committed(Ok("abc1234".into())));
    assert_eq!(
        h.app(0).toast.as_ref().map(|t| t.0.as_str()),
        Some("web: committed abc1234 · Change the title")
    );

    // A failed commit waits in its tab.
    h.keys("3 c tab");
    h.type_text("Again");
    h.ws.tabs[2].app.commit.as_mut().unwrap().busy = true;
    h.keys("1");
    h.send_to(2, Msg::Committed(Err("pre-commit hook failed".into())));
    assert_eq!(
        h.app(0).toast.as_ref().map(|t| t.0.as_str()),
        Some("web needs you · press 3")
    );
    assert!(h.ws.summary(2).attention);
    let bar = h.render(80, 24);
    assert!(bar.lines().next().unwrap().contains("web ◌ 1 !"), "{bar}");
}

#[test]
fn a_password_prompt_in_another_tab_waits_for_you() {
    let (api, _web, _svc, root) = portal();
    let mut h = WsHarness::open(&api, &[&root]);
    h.send_to(
        2,
        Msg::AskPass {
            id: 4,
            prompt: "Password for 'https://git.example.com':".into(),
            kind: PromptKind::Secret,
        },
    );
    assert_eq!(h.ws.active, 0);
    assert_eq!(
        h.app(0).toast.as_ref().map(|t| t.0.as_str()),
        Some("web needs you · press 3")
    );
    insta::assert_snapshot!("attention_80x24", h.render(80, 24));
    h.keys("3");
    let screen = h.render(80, 24);
    assert!(screen.contains(" Password · web "), "{screen}");
    // Typing goes to the prompt, digits included.
    h.type_text("s3cret\n");
    assert_eq!(h.ws.active, 2);
    assert_eq!(h.rec.answers, [(4, Some("s3cret".to_owned()))]);
}

#[test]
fn settings_reach_every_tab() {
    let (api, web, _svc, root) = portal();
    // web's own git config wins there.
    web.git(&["config", "spotter.wrapLines", "false"]);
    let mut h = WsHarness::open(&api, &[&root]);
    let i = setting_index("diff.wrap_lines");
    h.keys(&format!(", {} space esc", "j ".repeat(i)));
    assert!(h.app(0).config.wrap_lines);
    assert!(h.app(1).config.wrap_lines, "reloaded from the file");
    assert!(!h.app(2).config.wrap_lines, "git config overrides it");
}

#[test]
fn session_toggles_follow_you() {
    let (api, web, _svc, root) = portal();
    let long = format!("<p>{}</p>\n", "a line that goes on and on, ".repeat(5));
    web.write("index.html", long);
    let mut h = WsHarness::open(&api, &[&root]);
    h.keys("enter enter z");
    assert_eq!(h.app(0).wrap_override, Some(true));
    h.keys("3 enter enter");
    assert!(h.app(2).diff.is_some());
    assert_eq!(h.app(2).wrap_override, Some(true));
    insta::assert_snapshot!("diff_100x30", h.render(100, 30));
}

#[test]
fn the_highlighter_follows_the_tab_you_look_at() {
    let (api, web, _svc, root) = portal();
    let mut h = WsHarness::open(&api, &[&root]);
    web.git(&["config", "spotter.syntaxTheme", "Nord"]);
    let loaded = Config::load(Some(&h.config), &h.runtimes[2].1.git);
    let fx = h.ws.update(Msg::Tab(
        TabId(2),
        Box::new(Msg::ConfigReloaded(Box::new(loaded))),
    ));
    let themes = |fx: &[(TabId, Effect)]| {
        fx.iter()
            .filter_map(|(_, e)| match e {
                Effect::SetSyntaxTheme(t) => Some(t.as_name()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert!(themes(&fx).is_empty(), "a hidden tab doesn't switch it");
    let fx = h.ws.update(Msg::Key(common::key_events("3")[0]));
    assert_eq!(themes(&fx), ["Nord"]);
    let fx = h.ws.update(Msg::Key(common::key_events("1")[0]));
    assert_eq!(themes(&fx), ["Monokai Extended"]);
}

#[test]
fn many_tabs_scroll() {
    let first = TestRepo::at("portal/accounts-service");
    first.commit_file("a", "a\n", "a");
    let mut repos = vec![];
    for name in [
        "billing-service",
        "collector",
        "dashboard-frontend",
        "exporter",
        "forms",
    ] {
        let r = first.sibling(&format!("portal/{name}"));
        r.commit_file("a", "a\n", "a");
        repos.push(r);
    }
    let root = first.tmp.path().join("portal");
    let mut h = WsHarness::open(&first, &[&root]);
    assert_eq!(h.ws.tabs.len(), 6);
    h.keys("5");
    insta::assert_snapshot!("tabs_overflow_60x16", h.render(60, 16));
    for t in &mut h.ws.tabs {
        t.app.config.ascii = true;
    }
    let bar = h.render(60, 16);
    assert!(bar.lines().next().unwrap().is_ascii(), "{bar}");
    assert!(bar.lines().next().unwrap().starts_with("< 3 "), "{bar}");
}

#[test]
fn help_lists_the_repository_keys() {
    let (api, _web, _svc, root) = portal();
    let mut h = WsHarness::open(&api, &[&root]);
    h.keys("?");
    insta::assert_snapshot!("help_80x24", h.render(80, 24));
}

#[test]
fn background_tabs_poll_slowly() {
    let (api, _web, _svc, root) = portal();
    let mut h = WsHarness::open(&api, &[&root]);
    for t in &mut h.ws.tabs {
        t.app.watch = spotter::msg::WatchStatus::Polling;
    }
    let refreshes = |fx: &[(TabId, Effect)]| {
        fx.iter()
            .filter(|(_, e)| matches!(e, Effect::Refresh { .. }))
            .map(|(id, _)| id.0)
            .collect::<Vec<_>>()
    };
    let mut seen = Vec::new();
    for _ in 0..32 {
        let fx = h.ws.update(Msg::Tick);
        seen.extend(refreshes(&fx));
    }
    seen.sort();
    // Every 2 s for the one you see, every 8 s for the others.
    assert_eq!(seen, [0, 0, 0, 0, 1, 2]);
    // Looking at a polling tab refreshes it now.
    let fx = h.ws.update(Msg::Key(common::key_events("2")[0]));
    assert_eq!(refreshes(&fx), [1]);
}
