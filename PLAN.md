# Spotter — MVP Plan

A terminal dashboard that sits next to a coding agent and keeps one question answered at all times:

> **What has changed on this branch, and have I actually looked at it?**

It is not an AI interface, not an editor, and not a Git client. It is a read-only, always-live review view over what Git already records.

---

## 1. Principles

1. **Git is the history.** Commits are the timeline, and commit messages are the changelog. There is no parallel history, no session tracking, and no attempt to attribute changes to AI vs human.
2. **Read-only observer.** Spotter never modifies the repository: no staging, no committing, no `git add -N`, and no index refreshes that take `index.lock` (§7). It must be safe to run while an agent is in the middle of `git commit`.
3. **Live by default.** Spotter is meant to stay open, and a stale view is a broken view. File watching is part of the MVP, not a later feature.
4. **Uncommitted work is first-class.** Agents often leave large uncommitted changes for a long time. The working tree is a row in the timeline, not a hidden mode.
5. **One small, honest piece of state.** Spotter stores *viewed marks*. They are content-addressed, local to the git dir, and never committed. Deleting them only loses checkmarks, and they are never used to reconstruct history. This is the only deliberate exception to "no state", and it is what makes Spotter a *review* tool rather than another log viewer.
6. **Narrow pane first.** Spotter usually lives in a split beside an agent at 60–90 columns. The narrow layout is the primary design.
7. **Small, predictable keyboard model.**

## 2. What sets it apart

`tig main..HEAD` and lazygit can already show commits, files and diffs. Spotter is only worth building if these stay sharp:

- **Branch-scoped automatically.** It finds the base branch and merge-base without being told.
- **One timeline.** Uncommitted work, each commit, and the cumulative branch diff are rows in the same list.
- **Always live and lock-safe.** It updates as the agent works and never fights the agent for `index.lock`.
- **Remembers what you've reviewed.** Viewed marks survive commits, amends and rebases when content is unchanged.

---

## 3. The timeline model

Everything is one list of **review targets**:

```
◌ Uncommitted            HEAD → working tree (staged + unstaged + untracked)
● 83ac12 Fix item drag   parent → commit
● d19e42 Add tooltips    parent → commit
  …
Σ Branch total           merge-base → working tree (toggle: merge-base → HEAD)
```

Selecting a target shows its files. Opening a file shows the target's diff, scrolled to that file.

| Target | Diff | Notes |
|---|---|---|
| ◌ Uncommitted | `git diff HEAD` + untracked files | Unborn repo: diff against the empty tree |
| Commit | `git diff <parent> <sha>` | Root commit: parent is the empty tree |
| Merge commit | `git show --remerge-diff <sha>` | `m` toggles to `git diff <first-parent> <sha>` |
| Σ Branch total | `git diff <merge-base>` + untracked | `i` toggles to `git diff <merge-base> HEAD` (committed only) |

- File lists for every target come from `--raw --numstat -z --no-abbrev -M`. This gives status, blob ids (needed for viewed marks, §6.3), paths and line counts in one call.
- The empty tree is `git hash-object -t tree /dev/null`. Computing it this way works for both SHA-1 and SHA-256 repos.
- Untracked files come from `git status --porcelain=v2 -z --untracked-files=all`. Their content is read directly from the file: every line is an addition, which is exactly what `git diff --no-index -- /dev/null <path>` would print, without one process per file.

---

## 4. Interface

### Wide layout (≥ 100 columns)

```
 feature/inventory · base origin/main @4cb831 · 6 commits              ● live · 2 to review
┌ Timeline ────────────────────────────────┐┌ Files · d19e42 Add tooltips ─────────────────┐
│ ◌ Uncommitted          3 files   +18 -2  ││ ✓ M  src/inventory_ui.gd             +31 -4  │
│ ● 83ac12 Fix item drag           +12 -3  ││   M  src/tooltip.gd                  +28 -8  │
│▸◐ d19e42 Add tooltips            +64 -12 ││   A  assets/theme.tres                +5     │
│ ✓ a71bc9 Add stacking            +88 -20 ││                                              │
│ ✓ f21810 Inventory UI           +312     ││                                              │
│ ✓ 129aae Data model              +97 -2  ││                                              │
├──────────────────────────────────────────┤│                                              │
│ Σ Branch total       17 files  +842 -293 ││                                              │
└──────────────────────────────────────────┘└──────────────────────────────────────────────┘
 enter open · space viewed · u next to review · w/b jump · ? help
```

### Narrow layout (< 100 columns): the primary one

```
 feature/inventory · origin/main     ● live · 2 to review
┌ Timeline ──────────────────────────────────────────────┐
│ ◌ Uncommitted                        3 files   +18 -2  │
│ ● 83ac12 Fix item drag                         +12 -3  │
│▸◐ d19e42 Add tooltips                          +64 -12 │
│ ✓ a71bc9 Add stacking                          +88 -20 │
├────────────────────────────────────────────────────────┤
│ Σ Branch total                     17 files  +842 -293 │
└────────────────────────────────────────────────────────┘
┌ Files · d19e42 ────────────────────────────────────────┐
│ ✓ M  src/inventory_ui.gd                       +31 -4  │
│   M  src/tooltip.gd                            +28 -8  │
│   A  assets/theme.tres                          +5     │
└────────────────────────────────────────────────────────┘
 enter open · space viewed · u next · ? help
```

The Σ row is pinned to the bottom of the timeline panel and stays visible regardless of scroll.

### Diff view (full screen)

```
 d19e42 Add tooltips · src/tooltip.gd (2/3)                    ◐ 1/3 viewed
────────────────────────────────────────────────────────────────────────────
 M src/tooltip.gd                                              +28 -8
 @@ -10,7 +10,12 @@ func _ready():
  10   10      var label = Label.new()
  11      -    label.text = item.name
       11 +    label.text = item.display_name
       12 +    label.tooltip_text = item.description
  12   13      add_child(label)
────────────────────────────────────────────────────────────────────────────
 ]/[ hunk · }/{ file · space viewed+next · n/p commit · e edit · esc back
```

The diff view shows the **whole target** as one continuous scroll with a header per file, like GitHub's "Files changed" tab. Opening a file jumps to its header, which covers both "diff of a commit" and "diff of one file".

### Header and glyphs

- **Header:** branch (or `detached @sha`), base ref, merge-base, commit count, watch status (`● live` / `◌ polling` / `✕ watch error`), and the number of commits not fully viewed.
- **Banner line, when relevant:** trunk mode, rebase/merge/cherry-pick in progress, base looks wrong, git too old for remerge-diff.
- **Timeline glyphs:** `●` nothing viewed, `◐` partly viewed, `✓` all files viewed, `▸` cursor. Merge commits get a dim `(merge)` tag. ◌ and Σ are fixed type glyphs, and their files carry their own ✓ marks.
- **File status letters:** `M A D R C T U` as in git, plus `?` for untracked. Collapsed files show a dim `⋯ collapsed` suffix.

---

## 5. Keyboard

**Everywhere**

| Key | Action |
|---|---|
| `q` | Quit (in the diff view: back) |
| `Esc` | Back |
| `Tab` / `Shift-Tab` | Switch panel |
| `w` / `b` | Jump to ◌ Uncommitted / Σ Branch total |
| `u` | Jump to the oldest commit that still has unviewed files |
| `n` / `p` | Next (newer) / previous (older) commit |
| `i` | Σ: toggle including uncommitted changes |
| `Ctrl-L` | Force refresh and redraw |
| `?` | Help overlay |

**Timeline and file panels**

| Key | Action |
|---|---|
| `j` / `k`, `↓` / `↑` | Move |
| `g` / `G` | Top / bottom |
| `Enter` | Timeline: focus files. Files: open diff at that file |
| `Space` | Files: toggle viewed |
| `r` | Timeline: toggle all files of the target as viewed |
| `e` | Files: open in editor |

**Diff view**

| Key | Action |
|---|---|
| `j` / `k`, `Ctrl-D` / `Ctrl-U`, `g` / `G` | Scroll |
| `]` / `[` | Next / previous hunk |
| `}` / `{` | Next / previous file |
| `h` / `l` | Horizontal scroll |
| `Space` | Mark the current file viewed and jump to the next unviewed file, continuing into the next commit |
| `Enter` | Expand a collapsed file |
| `m` | Merge commit: toggle remerge-diff / first-parent |
| `e` | Open the editor at the line under the cursor |

**The review loop:** press `u`, then `Enter`, then read and press `Space`, `Space`, `Space`… When the agent commits again, the new commit appears as `●` and the header count goes up. Files you already viewed in ◌ Uncommitted stay viewed after the agent commits the same content (§6.3).

---

## 6. Behaviour details

### 6.1 Base resolution

1. Use `--base <ref>` if given, otherwise `git config spotter.base`.
2. Find the **default branch**: the target of `refs/remotes/origin/HEAD`, otherwise `main`, otherwise `master`. If there is no `origin`, use the only remote if exactly one exists.
3. **If HEAD is on the default branch, use trunk mode** (below).
4. **Otherwise (feature branch or detached HEAD):**
   - Candidates are whichever of these exist: the default branch, its remote-tracking ref, `main`, `master`, `origin/main` and `origin/master`, excluding the current branch.
   - For each candidate, compute `git rev-list --count <candidate>..HEAD` and pick the **smallest** count, i.e. the closest base. This handles a stale local `main` that is behind `origin/main`.
5. The merge-base is `git merge-base <base> HEAD`. The timeline is `git log --topo-order <base>..HEAD`.

**Trunk mode** (on `main` itself, common when agents commit straight to main):

- **With an upstream:** base is `@{upstream}`, so the timeline shows unpushed commits. The banner reads `trunk mode · unpushed commits (origin/main..HEAD)`. If nothing is unpushed, the timeline shows only ◌, with the hint `nothing unpushed · --base HEAD~10 to look further back`.
- **Without an upstream:** show the last 20 commits (`spotter.trunkDepth`).

**Edge cases**

- **Unborn repo (no commits):** the timeline has only ◌, and Σ is hidden.
- **Sanity guard:** if `base..HEAD` has more than 300 commits, show the first 300 and the banner `base origin/main is 1,204 commits behind · wrong base? use --base`.
- **Re-resolution:** the base is resolved again on every change to HEAD or refs, such as a branch switch or a fetch.

### 6.2 Live refresh

**What to watch**

- The worktree, plus the git dir and the common dir. These differ for linked worktrees; get them with `git rev-parse --git-dir --git-common-dir`.
- Do **not** register recursive watches blindly. `node_modules/` and `target/` would exhaust inotify watches. Walk the tree with the `ignore` crate, add non-recursive watches on non-ignored directories, and add watches for new directories as they appear.

**What to ignore**

- Inside git dirs, only `HEAD`, `index`, `packed-refs`, `refs/**`, `MERGE_HEAD`, `CHERRY_PICK_HEAD` and `rebase-*/**` matter.
- Drop `*.lock` files and `<common-dir>/spotter/**` (Spotter's own writes).

**Two refresh kinds**, debounced by about 250 ms:

| Trigger | Refreshes |
|---|---|
| HEAD / refs / packed-refs changed | Base, merge-base, timeline, status: everything |
| Worktree or index changed | Status, ◌ and Σ stats, and the open diff if it is ◌ or Σ |

**How refreshes run**

- **Off the UI thread:** refreshes run on a worker thread and are tagged with a generation number; stale results are dropped.
- **Coalescing:** events that arrive during a refresh trigger exactly one more refresh afterwards.
- **Polling fallback:** if the watcher can't start, poll every 2 s and show `◌ polling`. `--no-watch` forces this mode.
- **Toasts:** `+2 commits`, or `history rewritten (amend/rebase)` when the old HEAD is not an ancestor of the new HEAD.

**Lock safety:** every call uses `--no-optional-locks` and `GIT_OPTIONAL_LOCKS=0`. Spotter therefore never writes `.git/index` and can't make an agent's commit fail with *"Another git process seems to be running"*.

### 6.3 Viewed marks

**The key.** A mark is the key `(path, old_blob, new_blob)`: one exact file change, identified by content.

- Blob ids come from `git diff --raw --no-abbrev`.
- For working-tree files, raw output shows all zeros, so the id is computed with `git hash-object -- <path>`. This applies the same clean filters git uses on commit, and without `-w` it writes nothing.

**What this gives for free**

- A file viewed in ◌ Uncommitted is still ✓ after the agent commits that exact content.
- If the agent edits a viewed file again, its key changes and it becomes unviewed.
- A rebase that doesn't change a file's content keeps its marks.

**Commit state is derived.** ✓ means all files viewed, ◐ means some, ● means none. `r` marks every file of a target. There is no separate per-commit store.

**Storage.** `<git-common-dir>/spotter/viewed.json` holds `{ "version": 1, "viewed": { "<old>:<new>:<path>": <unix-ts> } }`.

- The file is shared across linked worktrees.
- Writes are atomic (temp file plus rename) and do read-merge-write, so two Spotter instances don't clobber each other.
- Entries older than 90 days are pruned on write.
- Merge commits use keys from whichever diff mode is currently shown.

### 6.4 Diff content

- **Parsing and rendering:** one unified-diff parser (files → hunks → lines with old/new numbers) and one renderer for all targets.
- **Loading:** a target's whole patch loads at once up to about 20k lines. Above that, files load on demand as you navigate to them.
- **Collapsed by default** (`Enter` expands):
  - Lockfiles and generated patterns: `Cargo.lock`, `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `poetry.lock`, `uv.lock`, `Gemfile.lock`, `composer.lock`, `go.sum`, `*.min.js`, `*.min.css`, `*.map`. Configurable via multi-valued `spotter.collapse`.
  - Files with the gitattributes `linguist-generated` or `-diff` (checked with `git check-attr -z --stdin`).
  - Files whose patch is longer than 1,500 lines.
- **Untracked files:** read directly (binary means a NUL in the first 8000 bytes, git's heuristic). Files over 1 MB, and binary files, get a summary only.
- **Binary files:** shown as `Binary file · 12.4 KB → 13.1 KB`, with sizes from `git cat-file -s` or the filesystem.
- **Renames:** detected with `-M` and shown as `R  old/path → new/path (92%)`.
- **Mode changes, symlinks, submodules** (`--submodule=short`): one-line entries.
- **Merge commits:** remerge-diff by default, which shows only what was decided during the merge (conflict resolutions and "evil" changes) and is usually tiny or empty. It needs git ≥ 2.36; older git falls back to first-parent.
- **Display:** tabs expand to 4 columns (`spotter.tabWidth`), widths use `unicode-width`, control characters are escaped, lines don't wrap (horizontal scroll instead), and the "no newline at end of file" marker is shown.

### 6.5 History rewrites and repository states

- **Stable selection.** The selection follows the target: same SHA if it still exists, otherwise same subject and author, otherwise same position, otherwise clamped. The diff view keeps its scroll position if the same file still exists.
- **In-progress operations.** Banners for rebase (`rebase-merge/` or `rebase-apply/`, with `n/m` progress), merge (`MERGE_HEAD`), cherry-pick, revert and bisect. Spotter keeps working and just says what is happening. Conflicted files appear with status `U`.
- **Branch switch:** a full refresh plus base re-resolution.

### 6.6 Open in editor

**Which line**

- In the diff view: the new-side line under the cursor.
- In the file list: the first changed line.

**Which command**

- Order of precedence: `spotter.editor` (a template such as `code -g {file}:{line}`), then `$VISUAL`, then `$EDITOR`, then `vi`.
- Built-in templates:
  - `vi`, `vim`, `nvim`, `nano`, `emacs`, `micro`, `kak`: `+{line} {file}`
  - `hx`, `zed`, `subl`: `{file}:{line}`
  - `code`, `codium`, `cursor`: `-g {file}:{line}`

**How it launches**

- **Terminal editors:** suspend the TUI (leave the alternate screen and raw mode), run the editor, then restore the TUI and refresh.
- **GUI editors** (`code`, `codium`, `cursor`, `zed`, `subl`, or `spotter.editorGui=true`): spawn detached; the TUI keeps running.

**Historical commits:** the current file opens. If it changed since that commit, show `file changed since d19e42 · line is approximate`. Deleted files show a message and nothing opens.

---

## 7. Git interface rules

Every git invocation goes through one function, `git::run`, which enforces the following.

**Fixed arguments and environment**

- Global arguments: `--no-optional-locks --no-pager --literal-pathspecs -c core.quotepath=off -c color.ui=never -c diff.noprefix=false -c diff.mnemonicPrefix=false -c diff.relative=false -c log.showSignature=false -c diff.autoRefreshIndex=false`
- `diff.autoRefreshIndex=false` matters: porcelain `git diff` against the working tree otherwise rewrites `.git/index` when files are stat-dirty, **even with `--no-optional-locks`**. Stat-dirty files with unchanged content are then listed with a zero blob id; Spotter hashes them anyway (§6.3) and drops the ones whose content matches.
- Diff and log arguments: `--no-ext-diff --no-textconv --src-prefix=a/ --dst-prefix=b/ -M`, plus `-z` whenever the output contains paths.
- Environment: `GIT_OPTIONAL_LOCKS=0`, `GIT_TERMINAL_PROMPT=0`.

**Parsing**

- Custom formats use `%x1f` between fields and `-z` between records. Human-oriented output is never parsed.
- Output and paths are handled as bytes (`bstr`), because paths can be non-UTF-8. They are converted lossily only for display.

**Errors**

- A non-zero exit is shown on the status line with git's stderr; it is never a panic.
- Known non-error exits, like `rev-parse --verify -q` or `merge-base` returning 1, are whitelisted per call.

**Read-only allowlist**

- Allowed subcommands: `version`, `rev-parse`, `symbolic-ref`, `for-each-ref`, `rev-list`, `merge-base`, `log`, `show`, `diff`, `status`, `cat-file`, `check-attr`, `hash-object` (never with `-w`), and `config --get*`.
- A unit test asserts that nothing else can be invoked.

---

## 8. Architecture

**Stack**

- Rust stable with `ratatui` and `crossterm`.
- Libraries: `notify`, `ignore`, `clap`, `serde`/`serde_json`, `unicode-width`, `bstr`, `anyhow`.
- Dev dependencies: `insta`, `tempfile`. Optional: `ansi-to-tui` for the delta renderer.
- No async runtime: std threads and channels.

```
src/
  main.rs        CLI, terminal setup/restore (including on panic)
  app.rs         App state + update(msg) → effects; no I/O, unit-testable
  msg.rs         Key, Resize, FsEvent, RefreshDone, PatchLoaded, Tick
  ui/            layout.rs (wide/narrow), header.rs, timeline.rs, files.rs, diff.rs, help.rs
  git/
    cmd.rs       the single choke point (§7)
    repo.rs      discovery, git dirs, version, in-progress operation detection
    base.rs      base and trunk-mode resolution (§6.1)
    log.rs       timeline commits + shortstat
    status.rs    porcelain v2 parser
    diff.rs      raw+numstat parser, unified diff parser
  model.rs       Target, FileChange, Patch, Hunk, Line
  worker.rs      background refresh, generations, coalescing, patch cache
  watch.rs       gitignore-aware watcher + polling fallback
  review.rs      viewed-marks store
  editor.rs      editor templates, suspend/resume
  config.rs      CLI flags + `git config --get-regexp '^spotter\.'`
```

- **Event loop.** The main thread owns the terminal and draws from `App` state. It receives `Msg`s from one channel fed by the input, watcher and worker threads.
- **Caching.** Commit file lists and patches never change, so they are cached by SHA (LRU by size). Only ◌ and Σ are volatile.
- **Configuration.** Settings live in git config (`spotter.base`, `spotter.editor`, `spotter.editorGui`, `spotter.collapse`, `spotter.tabWidth`, `spotter.trunkDepth`), per repo or global. There is no new config file.

```
spotter [PATH]                # defaults to the current directory
  --base <ref>                # override base resolution
  --no-watch                  # poll instead of watching
  --renderer builtin|delta    # M7
```

- **Packaging.** The crate is published as `spotter-tui`, because plain `spotter` is taken on crates.io. The binary is `spotter`. An optional `git-spotter` alias makes `git spotter` work too.

---

## 9. Milestones

Each milestone ends in something usable and tested.

| | Scope | Done when |
|---|---|---|
| **M0 Skeleton** | Cargo project, CI (fmt, clippy, test), terminal init/restore with panic hook, `git::run` with allowlist, repo discovery, git version check | Starts and quits cleanly; clear error outside a repo; terminal restored after a forced panic |
| **M1 Timeline** | Base resolution (feature, trunk, detached, unborn, sanity guard), commits + shortstat, ◌ and Σ rows, header, wide and narrow layouts | Integration tests pass for every §6.1 scenario |
| **M2 Files** | raw+numstat for all targets, untracked, renames, binary, collapse rules, status letters | Correct lists for fixtures, including paths with spaces, unicode and newlines |
| **M3 Diff view** | Parser and renderer, continuous multi-file scroll, hunk/file navigation, horizontal scroll, expand collapsed, merge toggle, `n`/`p` | Snapshot tests at 80×24 and 160×48; a 10k-line diff scrolls without lag |
| **M4 Live** | Gitignore-aware watcher, debounce, two refresh kinds, generations, polling fallback, selection stability, toasts, in-progress banners | A script that commits, amends, rebases and edits in a loop: the view converges within 1 s, and `.git/index` is never touched by Spotter |
| **M5 Viewed marks** | Store, `Space`/`r`/`u`, glyphs, header count, `Space`-advance in the diff view | View a file in ◌, commit it with plain git, and the commit shows that file ✓ |
| **M6 Editor + help** | `e` with templates, suspend/resume, GUI detection, `?` overlay | Manual check with nvim and VS Code |
| **M7 Delta (optional)** | Spike: render each file via `delta --paging=never --width <cols>` and convert with `ansi-to-tui`; per-file runs keep file navigation, hunk navigation is best-effort | Decide to keep or drop after the spike |

**The MVP is done** once it has been used daily beside an agent for a week, and every rough edge found has been fixed or explicitly deferred.

---

## 10. Testing

- **Parser unit tests** run on captured git output: status v2, raw+numstat `-z` including renames, and unified diffs including binary files, no-newline markers, mode changes and submodules.
- **Integration tests** build throwaway repos with a `TestRepo` helper (`tempfile` plus real `git`). They set `GIT_CONFIG_GLOBAL` to a temp file and `GIT_CONFIG_NOSYSTEM=1`, so the developer's own config can't leak in.
- **Scenarios:**
  - Feature branch
  - Stale local `main` vs `origin/main` (bare repo as the remote)
  - Trunk mode with and without an upstream
  - Detached HEAD; unborn repo
  - Merge with a conflict resolution
  - Amend and rebase
  - Untracked, binary, renamed and huge files
  - Linked worktree
  - SHA-256 repo (`git init --object-format=sha256`)
- **Hostile config:** the same scenarios rerun with `diff.noprefix=true`, `color.ui=always`, `diff.external=<script>`, `log.showSignature=true`, `diff.relative=true` and `core.quotepath=true`.
- **UI snapshots** use ratatui's `TestBackend` with `insta`.

---

## 11. Non-goals

- An AI chat interface, a code editor, a file browser.
- An AI checkpoint database, session tracking, or AI-vs-human attribution.
- **Git operations** (staging, committing, branching, pushing). The original "commit from the TUI" key is dropped: the agent commits, and lazygit or plain git handle everything else.
- Replacing lazygit or tig as a general Git client.

## 12. Later

- **Multi-worktree dashboard.** One section per worktree/branch, for watching several parallel agents. This is the most promising direction after the MVP.
- **"Since last review" diff:** one combined diff of everything not yet viewed.
- Search and filter for commits and files.
- Built-in syntax highlighting (syntect or tree-sitter) and word-level intra-line highlights.
- Side-by-side mode and a line-wrap toggle.
- A staged vs unstaged split view for ◌.
- Test/build status per commit.
- Per-directory stats and a commit graph.
- Stacked-branch support and a `--first-parent` toggle.
- tmux/zellij integration.
- `--ascii` glyphs and mouse support.

## 13. Open questions (resolved)

All four went with the proposal.


1. **Minimum git version.** 2.30+, with remerge-diff only on ≥ 2.36 (Ubuntu 22.04 ships 2.34).
2. **Σ default.** Includes uncommitted changes; `i` toggles to committed only.
3. **Trunk mode with nothing unpushed.** Only ◌, with the `--base HEAD~10` hint.
4. **Where viewed marks live.** `<git-common-dir>/spotter/`: invisible to the repo and shared across worktrees.
