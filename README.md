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

Screens can be rendered to stdout without launching the TUI (`cargo preview <scene>`), and every
scene is committed as an SVG under `docs/preview/` that CI re-renders on each PR, so a UI change
shows up as a before/after picture. See [`docs/preview/README.md`](docs/preview/README.md) for
the scene gallery and how the preview and screenshot tests work.

## License

Apache-2.0
