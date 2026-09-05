# fv

<!-- UI スクリーンショットテストの成果物そのもの (docs/preview/)。CI が焼き直すので手で貼り替えない -->
<img width="700" alt="fv: file tree, syntax-highlighted viewer and git status in one terminal" src="docs/preview/view.svg" />


TUI code viewer with syntax highlighting, git status, and inline editing.

Browse a directory tree, open files with syntax highlighting, search, see git changes at a glance, review diffs, and edit files in-place without leaving the terminal. Files reload automatically when they change on disk.

## Features

- **Modes** (`Shift+Tab` to cycle) — VIEW / EDIT / GIT, so each mode keeps its own key map
- File tree with `.gitignore`-aware scanning and git status markers (`i` shows ignored files too — `.gitignore` / `.ignore` / `.git/info/exclude` — dimmed)
- Syntax highlighting (syntect)
- **Inline editing** (`e`) — insert, delete, undo/redo, paste, save
- **Git mode** — tree filtered to changed files only (hierarchy preserved), unified diff with hunk jumping
- **Commit log panel** (`L`) — the commit history sits under the tree in the same pane, so you can read a file and walk its history side by side; `Enter` shows the commit's diff on the right
- Live changed-line markers (`▎`) in the gutter while editing (LCS diff against git HEAD, no per-keystroke git calls)
- Fuzzy file finder (`Ctrl+p`)
- Workspace-wide text search (`Ctrl+f`, streams hits while it scans)
- In-file search (`/`, `n`/`N`) and line jump (`:N`)
- **Copy out of the viewer** — drag with the mouse for a character range, `v` for a line range, `y` to copy, `Y` for the whole file (`pbcopy`/`wl-copy`/`xclip`/`xsel`/`clip.exe`, falling back to OSC 52 so it works over SSH)
- Auto-reload on file system changes
- Mouse support (click to select/open/move cursor, press-and-drag to select text, wheel to scroll)
- Wrap toggle, horizontal scroll, navigation history (`Ctrl+o`/`Ctrl+i`)
- Settings popup (`s`) for hidden files / gitignored files / icons / wrap default / syntax theme / UI language (English or Japanese, auto-detected from the locale), persisted to `~/.config/fv/config`

## Install

### Homebrew (macOS / Linux)

```sh
brew install Matuyuhi/tools/fv
```

### From source

```sh
cargo install --git https://github.com/Matuyuhi/fv
```

## Usage

```sh
fv [dir]   # defaults to the current directory
```

## Key bindings

Press `?` inside fv for the full list.

| Key | Action |
| --- | --- |
| `q` / `Ctrl+c` | Quit |
| `Shift+Tab` | Switch mode (VIEW → EDIT → GIT) |
| `Tab` | Switch focus (tree / viewer; the commit log joins the cycle while its panel is open) |
| `L` | Toggle the commit log panel (VIEW only) |
| `Ctrl+p` | Fuzzy finder |
| `Ctrl+f` | Search the whole workspace (Enter jumps to the hit) |
| `j`/`k`, `↑`/`↓` | Move / scroll |
| `h`/`l`, `←`/`→` | Collapse/expand (tree), horizontal scroll (viewer) |
| `gg` / `G` | Top / bottom |
| `Ctrl+d`/`Ctrl+u` | Half-page scroll |
| `/`, `n`/`N` | Search, next/previous match |
| `:N` `Enter` | Jump to line N |
| `w` | Toggle wrap |
| drag / `v` | Select a range in the viewer (character-wise / line-wise) |
| `y` / `Y` | Copy the selection / the whole file to the clipboard |
| `Ctrl+o`/`Ctrl+i` | History back / forward |
| `r` | Rescan tree |
| `n` / `N` (tree) | New file / new directory under the selected directory (`a/b.rs` creates intermediate directories) |
| `R` (tree) | Rename the selected file or directory |
| `D` (tree) | Delete the selected file or directory (asks for confirmation; not undoable) |
| `y` (tree) | Copy the relative path to the clipboard |
| `a` | Toggle hidden files (`-a`, `--hidden` at startup) |
| `i` | Toggle ignored files — `.gitignore` / `.ignore` / `.git/info/exclude` (`-i`, `--ignored` at startup) |
| `s` | Settings |
| `?` | Help |
| `e` | Enter edit mode |

### Edit mode (`e`)

| Key | Action |
| --- | --- |
| character keys | Insert text (click to move cursor) |
| `↑`/`↓`/`←`/`→` | Move cursor |
| `Ctrl+←`/`→` | Move word by word |
| `Home`/`End` | Beginning / end of line |
| `Ctrl+s` / `Cmd+s` | Save |
| `Ctrl+z` / `Ctrl+y` | Undo / redo |
| `Ctrl+k` | Delete line |
| `Esc` | Exit edit mode (prompts if unsaved; press `s` at prompt to save) |

### Git mode (`Shift+Tab`)

The tree is filtered down to changed files with the directory hierarchy preserved, and the right pane shows the unified diff of the selected file. Leaving the mode restores the tree exactly as it was.

| Key | Action |
| --- | --- |
| `j`/`k`, `↑`/`↓` | Move between changed files / scroll the diff |
| `h`/`l`, `←`/`→` | Collapse/expand (tree), horizontal scroll (diff) |
| `Enter` | Show the diff of the selected file |
| `]`/`[` | Next / previous hunk |
| `Space` (tree) | Stage / unstage the selected file or directory |
| `Space` (diff) | Stage just the hunk you are looking at (`unstage` when the diff base is `staged`) |
| `t` | Cycle the diff base (HEAD → staged → unstaged) |
| `w` | Toggle wrap (diff only, not persisted) |
| `r` | Rescan (also refreshes git status) |

Files that are deleted but not yet committed are listed as well, so they can still be selected and reviewed. Changes under hidden directories (for example `.github/`) need `a` or `--hidden`.

### Commit log panel (`L`)

<img width="700" alt="fv: commit log panel under the file tree, with the selected commit's diff on the right" src="docs/preview/log.svg" />

`L` splits the left pane in two: the file tree on top, the commit history underneath. It is a panel rather than a mode, so the tree stays where it is and `Tab` cycles through one more pane while it is open (with the panel closed, `Tab` still goes straight from the tree to the viewer). `Enter` on a commit puts its diff in the right pane (moving the focus with it); opening a file again from the tree brings the file back.

| Key | Action |
| --- | --- |
| `L` | Show / hide the panel |
| `j`/`k`, `↑`/`↓` | Move between commits (the diff does not follow) |
| `Enter` / `l` / `→` | Show the diff of the selected commit |
| `gg` / `G` | Top / end of what is loaded (loads one more page at the end) |
| `]`/`[`, `n`/`N` | Next / previous hunk (diff) |
| `w` | Toggle wrap (diff only, not persisted) |
| `Esc` | Close the diff (from the diff) / close the panel (from the list) |

Merge commits are shown as the diff against their first parent, with a note saying so — `git show` shows nothing at all for them by default.

Ignored files (`.gitignore`, `.ignore`, `.git/info/exclude`) are left out of the tree and the finder by default. `i` (or `-i` / `--ignored`) brings them in — they are drawn dimmed, since git does not track them — and the setting is persisted like the other settings.

## Development

### UI previews

Like Jetpack Compose `@Preview` or SwiftUI previews, you can render a single frame of any
screen straight to stdout — no need to launch the TUI and click your way to the state you
are working on:

```sh
cargo preview                      # list the available scenes
cargo preview git                  # render one scene
cargo preview git log commit       # render several, stacked
cargo preview all --size 140x40    # everything, at a specific terminal size
```

`cargo preview` is an alias for `cargo run --features preview -- --preview`. The preview is a
dev-only feature: it is off by default, so released binaries contain no preview code and do not
accept `--preview`.

Every scene runs against a throwaway sample repository (created under `$TMPDIR`) that always
has staged, unstaged, untracked and deleted files plus a few commits, so the output does not
depend on the state of your working tree. Scenes are built by feeding real key presses to the
app, and rendered by the same `ui::draw` the real terminal uses — there is no preview-only
drawing path.

For the edit-and-look loop, re-render on every save:

```sh
scripts/preview-watch.sh git      # rebuilds and redraws whenever src/ changes
```

Add a scene in `src/preview/scene.rs`; a scene is a name, a description and a key script.

See [`docs/preview/README.md`](docs/preview/README.md) for a screenshot of every scene.

### UI screenshot tests

Every scene is also committed as an image under `docs/preview/`, and CI re-renders all of them on
every PR. The images *are* the snapshots — there is no separate text form. Since they are SVG,
GitHub renders them: the diff shows up as a before/after picture in Files changed, and a bot
comment lays each changed scene out as **before | diff | after**, so an unintended UI regression
is something you can actually see.

The middle panel comes from [shotdiff](https://github.com/Matuyuhi/shotdiff) (`--diff-only`),
which paints every changed pixel pink — you spot the change without comparing two full screens by
eye. Its `diff` and `after` are rendered by that CI run rather than read from the commit, so the
comment shows the current drawing even when the committed images have not been refreshed yet.
Neither file exists in the repository, so they are published to a history-less orphan branch
(`ci-ui-diff`) and referenced by commit SHA — a branch name would let GitHub's image proxy serve
a stale picture forever.

```sh
cargo preview --update-snapshots        # re-render every scene into docs/preview/
cargo preview view --update-snapshots   # just one
```

Volatile values (commit SHAs, absolute dates) are masked while keeping their column width, so the
images stay byte-identical between runs and still read as screenshots. Masking happens on the
rendered buffer rather than on text, because the image carries per-cell colour too: the style
boundary left by a shorter date would otherwise move even after the text was padded back to a
fixed width.

SVG rather than PNG because it needs no rasteriser (and therefore no new dependency), it carries
no fonts of its own — so the Japanese text in the sample repository renders with the reader's
fonts rather than as tofu — and it is text, so it lives in git history like any other file.
Columns are held by giving every glyph its own `x`, the way a terminal puts characters on a grid;
relying on the viewer's font advance would drift across a 110-column line.

A stale image never fails a PR: the committed files are refreshed automatically by a bot commit
once the change lands on `main`. Refresh them yourself if you want the UI change to show up in
your own PR — and the screenshot at the top of this README, which is `docs/preview/view.svg`, to
match your branch.

## License

Apache-2.0
