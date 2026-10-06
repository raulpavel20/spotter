# Spotter

**A live view of what changed on your branch, and whether you've reviewed it yet.**

Run Spotter in its own terminal tab, next to the tab where your coding agent (Claude Code, Codex, Aider…) works. While the agent edits and commits, Spotter keeps an up-to-date timeline: every commit on the branch, the uncommitted work, and the branch's total diff. Switch to the Spotter tab whenever you want to catch up, and it remembers which files you have already reviewed. A split pane next to the agent works too: the layout adapts down to 80 columns.

Spotter is read-only by default. It never stages, commits or takes git's index lock, so it can't get in the agent's way. If you turn it on, it can also [commit the files you've reviewed and push](#commit-and-push).

![Spotter: reviewing an agent's commits as they land](assets/demo.gif)

## Install

**Prebuilt binary** (Linux and macOS, x86-64 and ARM):

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/raulpavel20/spotter/releases/latest/download/spotter-tui-installer.sh | sh
```

**From source** with Cargo (needs Rust 1.88+ and a C compiler):

```sh
cargo install spotter-tui --locked
```

Either way, the command is `spotter`.

## Use

Open a second tab in the agent's repository and start Spotter there:

```sh
spotter            # in any git repository
spotter ~/code/app # or point it at one
spotter ~/portal   # a folder of repositories opens them as tabs
```

**The review loop:** press `u` to jump to the oldest commit you haven't reviewed, then `Enter` to open its diff. Read, and press `Space` to mark each file viewed: the file collapses and Spotter moves to the next unviewed file, then on into the next commit. When the agent commits again, the new commit shows up as `●` and the "to review" count goes up.

**Viewed marks are tied to file content, not to commit hashes.** A file you viewed as uncommitted work stays viewed after the agent commits it unchanged, and a rebase that doesn't touch a file keeps its mark. Marks live in `.git/spotter/viewed.json`, are shared across worktrees, and are never committed.

### What the timeline shows

| Row | What it is |
|---|---|
| ◌ Uncommitted | Staged, unstaged and untracked changes |
| ● ◐ ✓ commits | Each commit on the branch: not viewed, partly viewed, fully viewed |
| Σ Branch total | Everything since the branch left its base. `i` switches between including and leaving out uncommitted work |

Spotter picks the base for you. On a feature branch it is the closest of `main`, `master`, `origin/main` and `origin/master`. On `main` itself it is the upstream, so you see your unpushed commits. `--base <ref>` (or `git config spotter.base <ref>`) overrides this.

### Keys

| Everywhere | |
|---|---|
| `u` | Oldest commit that still has unviewed files |
| `n` / `p` | Newer / older commit |
| `w` / `b` | Jump to ◌ Uncommitted / Σ Branch total |
| `i` | Σ: include or exclude uncommitted changes |
| `c` · `P` | Commit · push, [when turned on](#commit-and-push) |
| `,` | Settings |
| `?` | Help |
| `q` / `Esc` | Back / quit |
| `<` / `>` · `1`–`9` | Previous / next repository · repository by number, with [several open](#several-repositories) |

| Main screen | |
|---|---|
| `j` `k` / arrows | Move · `←` `→` or `Tab` switch between the timeline and files |
| `Enter` | Timeline: go to its files · Files: open the diff |
| `Space` | Toggle a file viewed |
| `r` | Toggle every file of the selected commit viewed |
| `e` | Open the file in your editor |

| Diff view | |
|---|---|
| `j` `k`, `Ctrl-D` `Ctrl-U`, `g` `G` | Scroll |
| `]` / `[` | Next / previous hunk |
| `}` / `{` | Next / previous file |
| `Enter` | Collapse / expand the file at the top |
| `Space` | Mark viewed (collapses it), go to the next unviewed file; on a viewed file, unmark it |
| `f` | Show / hide the file explorer · `←` `→` or `Tab` move between it and the diff |
| `W` | Hide whitespace-only changes for this session |
| `z` | Wrap long lines, or cut them off again, for this session |
| `m` | Merge commit: remerge-diff / first-parent diff |
| `e` | Open your editor at the first change on screen |

The bar at the bottom always shows the keys that apply to what's on screen.

## Settings

Press `,` to open the settings panel. Changes apply immediately and are saved to `~/.config/spotter/config.toml` (or `$XDG_CONFIG_HOME/spotter/config.toml`, or `--config <file>`). You can edit that file by hand too: its comments are kept, and Spotter picks up the changes while it runs.

Any setting can also be set for a single repository with git config, which wins over the file. Collapse patterns from git config are added to the file's.

```sh
git config spotter.contextLines 5
git config spotter.base origin/develop   # the base exists only here, or as --base
```

<details>
<summary>All settings</summary>

| Setting in `config.toml` | Git config key | Default |
|---|---|---|
| `review.explorer_on_open`: open diffs with the file explorer (wide terminals) | `spotter.explorerOnOpen` | `true` |
| `review.collapse_viewed`: viewed files collapse | `spotter.collapseViewed` | `true` |
| `review.space_continues`: `Space` moves on into the next commit | `spotter.spaceContinues` | `true` |
| `review.total_includes_uncommitted`: Σ starts with uncommitted work | `spotter.totalIncludesUncommitted` | `true` |
| `diff.context_lines` (0–20) | `spotter.contextLines` | `3` |
| `diff.ignore_whitespace` | `spotter.ignoreWhitespace` | `false` |
| `diff.word_highlights`: tint the words that changed | `spotter.wordHighlights` | `true` |
| `diff.line_numbers` | `spotter.lineNumbers` | `true` |
| `diff.wrap_lines`: diffs start with long lines wrapped (`z` toggles) | `spotter.wrapLines` | `false` |
| `diff.syntax`: syntax highlighting | `spotter.syntax` | `true` |
| `diff.tab_width` (1–16) | `spotter.tabWidth` | `4` |
| `diff.collapse_lines`: files with more changed lines start collapsed | `spotter.collapseLines` | `1500` |
| `diff.collapse`: extra patterns of files that start collapsed | `spotter.collapse` (multi-valued) | `[]` |
| `theme.background`: `auto`, `dark` or `light` | `spotter.theme` | `auto` |
| `theme.syntax_theme`: `default` or one of the bundled themes (`ansi` follows your terminal's palette) | `spotter.syntaxTheme` | `default` |
| `theme.ascii`: ASCII glyphs and borders | `spotter.ascii` | `false` |
| `layout.wide_breakpoint`: columns from which panels sit side by side | `spotter.wideBreakpoint` | `100` |
| `layout.timeline_width`, `layout.explorer_width` (% of the width) | `spotter.timelineWidth`, `spotter.explorerWidth` | `45`, `30` |
| `editor.command`, e.g. `code -g {file}:{line}` (empty: `$VISUAL`, `$EDITOR`, `vi`) | `spotter.editor` | — |
| `editor.gui`: `auto`, `true` or `false` | `spotter.editorGui` | `auto` |
| `git.actions`: [commit and push](#commit-and-push) from Spotter | `spotter.gitActions` | `false` |
| `repo.trunk_depth`: commits shown when there is no base | `spotter.trunkDepth` | `20` |

</details>

## Several repositories

Started in a folder that isn't a repository itself, say a `portal/` folder holding one repository per service, Spotter opens every repository inside it as a tab. You can also name them: `spotter api web`.

```
┌───────────┐
│ 1 api ◌ 2 │ 2 worker  3 web 1                                      portal
│           └──────────────────────────────────────────────────────────────┐
│ feature/upload · main                               ● live · 2 to review │
└──────────────────────────────────────────────────────────────────────────┘
```

- Each tab shows `◌` when the repository has uncommitted changes, and how many commits on its branch are left to review (a trunk branch with no upstream has no review count). Quiet repositories are dimmed.
- When the agent changes a repository you aren't looking at, its tab turns bold until you look. Spotter never switches tabs by itself.
- `<` and `>` switch tabs, `1`–`9` jump to one. Each tab keeps its own place; `z`, `W` and the file explorer stay as you set them.
- When a commit or push finishes in another tab, the tab you're on says so. A password prompt waiting in another tab marks it with `!`.
- Spotter looks up to three folders deep. It skips hidden folders, `node_modules`, `target`, `vendor` and `venv`, and doesn't look inside the repositories it finds (submodules and nested repositories stay theirs). It opens at most 32; past that, name the ones you want.
- A repository cloned into the folder while Spotter runs shows up the next time you start it.

## Commit and push

Off by default: turn on **Commit and push from Spotter** in the settings (`,`), or run `git config spotter.gitActions true` for one repository.

- `c` opens the commit panel with the uncommitted files, the ones you've marked viewed already picked. `space` picks a file, `a` all of them, `n` none, `v` the viewed ones. Write a summary, and a body after `tab` (`Ctrl-E` opens git's editor instead), then press `Enter`.
- Spotter commits exactly the picked files, as they are on disk (`git commit --only`). Everything else stays as it was, including whatever the agent has staged. The new commit shows up already viewed.
- `P` pushes the current branch to its upstream once you confirm. A branch without one is published to `origin` and starts tracking it. Spotter never force-pushes: if the remote has moved on, it tells you to pull in git first.
- When git or ssh need a username, password or passphrase, Spotter asks in a panel. For these commands Spotter is git's `GIT_ASKPASS` and ssh's `SSH_ASKPASS` helper: answers go straight to git over a private socket and are never logged. A commit signed with gpg briefly hands the terminal to gpg's own prompt.

## Safe to run next to an agent

- Refreshes only run read-only git commands, checked against an allowlist in the code. They never write `.git/index` or take `index.lock`, so they can't make an agent's `git commit` fail.
- Unless you commit or push, Spotter's only write is the viewed-marks file, `.git/spotter/viewed.json`.
- Commit and push are off by default and run only when you confirm them, through their own short allowlist (`add`, `commit`, `push`, `reset`). A commit stops with "git is busy" rather than wait for the agent's index lock, and a push is never forced.
- It refreshes from file-system events: gitignored directories such as `node_modules` and `target` aren't watched. If watching fails, it falls back to polling every 2 seconds (`--no-watch` forces polling); a repository in a tab you aren't looking at is polled every 8 seconds.

## Troubleshooting

Set `SPOTTER_LOG` to log every git command Spotter runs (with exit code and timing) and every error:

```sh
SPOTTER_LOG=/tmp/spotter.log spotter
```

Please attach that log when you open an issue.

**Platforms:** Linux and macOS. Windows isn't supported yet.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT), at your option.
