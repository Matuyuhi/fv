# fv

Read-only TUI code viewer with syntax highlighting and git status.

fv is for *looking at* code, not editing it — browse a directory tree, open files with syntax highlighting, search, and see git changes at a glance. Files reload automatically when they change on disk.

## Features

- File tree with `.gitignore`-aware scanning and git status markers
- Syntax highlighting (syntect)
- Fuzzy file finder (`Ctrl+p`)
- In-file search (`/`, `n`/`N`) and line jump (`:N`)
- Changed-line markers in the gutter (`▎`) based on `git diff`
- Auto-reload on file system changes
- Mouse support (click to select/open, wheel to scroll)
- Wrap toggle, horizontal scroll, navigation history (`Ctrl+o`/`Ctrl+i`)

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
| `?` | Help |

## License

Apache-2.0
