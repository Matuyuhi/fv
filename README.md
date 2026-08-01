# fv

<img width="700" alt="スクリーンショット 2026-07-18 21 19 04" src="https://github.com/user-attachments/assets/5736ca52-ebf1-42d5-92fa-61c41ebc7e97" />


TUI code viewer with syntax highlighting, git status, and inline editing.

Browse a directory tree, open files with syntax highlighting, search, see git changes at a glance, review diffs, and edit files in-place without leaving the terminal. Files reload automatically when they change on disk.

## Features

- **Modes** (`Shift+Tab` to cycle) — VIEW / EDIT / GIT, so each mode keeps its own key map
- File tree with `.gitignore`-aware scanning and git status markers
- Syntax highlighting (syntect)
- **Inline editing** (`e`) — insert, delete, undo/redo, paste, save
- **Git mode** — tree filtered to changed files only (hierarchy preserved), unified diff with hunk jumping
- Live changed-line markers (`▎`) in the gutter while editing (LCS diff against git HEAD, no per-keystroke git calls)
- Fuzzy file finder (`Ctrl+p`)
- In-file search (`/`, `n`/`N`) and line jump (`:N`)
- Auto-reload on file system changes
- Mouse support (click to select/open/move cursor, wheel to scroll)
- Wrap toggle, horizontal scroll, navigation history (`Ctrl+o`/`Ctrl+i`)
- Settings popup (`s`) for hidden files / icons / wrap default / syntax theme, persisted to `~/.config/fv/config`

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
| `Tab` | Switch focus (tree / viewer) |
| `Ctrl+p` | Fuzzy finder |
| `j`/`k`, `↑`/`↓` | Move / scroll |
| `h`/`l`, `←`/`→` | Collapse/expand (tree), horizontal scroll (viewer) |
| `gg` / `G` | Top / bottom |
| `Ctrl+d`/`Ctrl+u` | Half-page scroll |
| `/`, `n`/`N` | Search, next/previous match |
| `:N` `Enter` | Jump to line N |
| `w` | Toggle wrap |
| `Ctrl+o`/`Ctrl+i` | History back / forward |
| `r` | Rescan tree |
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
| `n`/`N` (`]`/`[`) | Next / previous hunk |
| `w` | Toggle wrap (diff only, not persisted) |
| `r` | Rescan (also refreshes git status) |

Files that are deleted but not yet committed are listed as well, so they can still be selected and reviewed. Changes under hidden directories (for example `.github/`) need `a` or `--hidden`.

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

### UI snapshots

Every scene is also committed as plain text under `tests/snapshots/`, and CI re-renders them on
every PR. If the rendering changes, the job fails and prints the diff — so an unintended UI
regression shows up as concrete lines rather than "something changed":

```diff
-│▾ src                          │
+│▾ docs                         │
+│     D old.md                  │
```

Volatile values (commit SHAs, absolute dates) are masked while keeping their column width, so
the snapshots stay byte-identical between runs and still read as screenshots. When a change is
intentional, refresh them and commit:

```sh
cargo preview --update-snapshots
```

## License

Apache-2.0
