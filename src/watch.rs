use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

/// root を再帰監視し、変更パスをためておくキューを持つ。
/// watcher 本体は _watcher で保持しているだけで直接は使わない
/// (Drop すると監視が止まるため生かしておく必要がある)。
pub struct FsWatcher {
    _watcher: RecommendedWatcher,
    rx: Receiver<notify::Result<Event>>,
    root: PathBuf,
    ignore: Option<Gitignore>,
}

impl FsWatcher {
    /// 監視の開始に失敗しても None を返すだけで、呼び出し側は
    /// 監視なしでアプリを起動し続けられるようにする。
    pub fn new(root: &Path) -> Option<Self> {
        let (tx, rx) = channel();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })
        .ok()?;
        watcher.watch(root, RecursiveMode::Recursive).ok()?;

        Some(Self {
            _watcher: watcher,
            rx,
            root: root.to_path_buf(),
            ignore: build_gitignore(root),
        })
    }

    /// 溜まったイベントのパスを非ブロッキングで全部取り出す。
    /// .git 配下や .gitignore にマッチするパスはここで除外する。
    pub fn drain(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        while let Ok(res) = self.rx.try_recv() {
            let Ok(event) = res else { continue };
            for path in event.paths {
                if !self.is_ignored(&path) {
                    paths.push(path);
                }
            }
        }
        paths
    }

    fn is_ignored(&self, path: &Path) -> bool {
        let Ok(rel) = path.strip_prefix(&self.root) else {
            return false;
        };
        if rel
            .iter()
            .any(|component| component.to_string_lossy().starts_with('.'))
        {
            return true;
        }
        match &self.ignore {
            // 削除イベントは path がもう存在しないため is_dir を確定できない。
            // false 扱いでも大半の gitignore パターン (拡張子・ディレクトリ名) には支障ない。
            // matched ではなく matched_path_or_any_parents を使うのは、`target/` のような
            // ディレクトリパターンを target/debug/foo など配下のイベントにも効かせるため
            Some(ignore) => ignore
                .matched_path_or_any_parents(rel, path.is_dir())
                .is_ignore(),
            None => false,
        }
    }
}

fn build_gitignore(root: &Path) -> Option<Gitignore> {
    let path = root.join(".gitignore");
    if !path.is_file() {
        return None;
    }
    let mut builder = GitignoreBuilder::new(root);
    builder.add(&path);
    builder.build().ok()
}
