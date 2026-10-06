# Changelog

All notable changes are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- `z` in the diff view wraps long lines, for the session; `diff.wrap_lines` (`spotter.wrapLines`) sets whether diffs start wrapped.
- A folder of repositories opens them as tabs: run `spotter` in a folder that holds several repositories (up to three folders deep), or name them (`spotter api web`). `<` / `>` and `1`–`9` switch; a tab shows uncommitted changes and commits left to review, and turns bold when something changes there.
- Commit and push from Spotter, off by default (`git.actions`, `spotter.gitActions`). `c` commits the files you pick, with the viewed ones picked to start, using `git commit --only`; `P` pushes the current branch, or publishes it. Password prompts from git and ssh appear in a panel.

### Changed
- Pasted text arrives as one paste (bracketed paste) instead of as key presses.
- Started outside a repository, Spotter looks for repositories inside the folder before giving up.
- The file watcher starts in the background, so the first screen appears sooner in big repositories.

### Fixed
- Changing `diff.ignore_whitespace` in the settings now takes effect after `W` was pressed.

## [0.1.0] - 2026-10-03

The first release.

### Timeline and review
- One timeline: uncommitted work (◌), each commit on the branch, and the branch total (Σ).
- The base branch is picked automatically (closest of `main`/`master`/`origin/*`); on `main` itself, the timeline shows unpushed commits; `--base` overrides.
- Viewed marks tied to file content, so they survive commits, amends and rebases that don't change a file. Stored in `.git/spotter/viewed.json`.
- `u` jumps to the oldest unreviewed commit; `Space` marks a file viewed and moves on, into the next commit.

### Diff view
- The whole target in one scroll, with syntax highlighting (bat's grammars and themes) and tinted `+`/`-` rows that emphasize the words that changed.
- File explorer: a side panel on wide terminals, a drawer on narrow ones.
- Pager-style scrolling with a pinned file header, collapsible files (viewed files collapse), hunk and file jumps, remerge-diff for merges.
- `e` opens your editor at the change on screen.

### Live and safe
- Refreshes on file-system events (gitignore-aware), with a polling fallback.
- Read-only: only allowlisted git commands, and it never touches `.git/index` or `index.lock`.

### Settings
- A settings panel (`,`) with live preview, saved to `~/.config/spotter/config.toml`; git config `spotter.*` overrides per repository.

### Troubleshooting
- `SPOTTER_LOG=<file>` records every git command and error.
