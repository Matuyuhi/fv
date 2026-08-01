use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

/// root を再帰監視し、変更パスをためておくキューを持つ。
/// 再帰監視の登録 (inotify では配下のディレクトリ 1 つずつに watch を張る) は
/// ツリーが大きいほど時間がかかるため別スレッドで行い、起動を待たせない。
/// 登録が終わるまでのイベントは取りこぼすが、それは監視開始前と同じ状態でしかない
pub struct FsWatcher {
    state: State,
    root: PathBuf,
    ignore: Option<Gitignore>,
    show_hidden: bool,
}

enum State {
    Starting(Receiver<Option<Active>>),
    Active(Active),
    // 監視なし (登録失敗・スレッドを起こせない等)。この場合も自動リロードが
    // 効かないだけでアプリは動き続ける
    Off,
}

// watcher 本体は _watcher で保持しているだけで直接は使わない
// (Drop すると監視が止まるため生かしておく必要がある)。
struct Active {
    _watcher: RecommendedWatcher,
    rx: Receiver<notify::Result<Event>>,
}

impl FsWatcher {
    pub fn new(root: &Path, show_hidden: bool) -> Self {
        let (tx, rx) = channel();
        let target = root.to_path_buf();
        let state = match thread::Builder::new().spawn(move || {
            let _ = tx.send(Active::start(&target));
        }) {
            Ok(_) => State::Starting(rx),
            Err(_) => State::Off,
        };
        Self {
            state,
            root: root.to_path_buf(),
            ignore: build_gitignore(root),
            show_hidden,
        }
    }

    /// 溜まったイベントのパスを非ブロッキングで全部取り出す。
    /// .git 配下や .gitignore にマッチするパスはここで除外する。
    pub fn drain(&mut self) -> Vec<PathBuf> {
        self.adopt();
        let State::Active(active) = &self.state else {
            return Vec::new();
        };
        let mut paths = Vec::new();
        while let Ok(res) = active.rx.try_recv() {
            let Ok(event) = res else { continue };
            for path in event.paths {
                if !self.is_ignored(&path) {
                    paths.push(path);
                }
            }
        }
        paths
    }

    // 別スレッドでの監視開始を待たずに毎 tick 覗きに行く (届いていなければ何もしない)
    fn adopt(&mut self) {
        let State::Starting(rx) = &self.state else {
            return;
        };
        match rx.try_recv() {
            Ok(Some(active)) => self.state = State::Active(active),
            Ok(None) | Err(TryRecvError::Disconnected) => self.state = State::Off,
            Err(TryRecvError::Empty) => {}
        }
    }

    fn is_ignored(&self, path: &Path) -> bool {
        let Ok(rel) = path.strip_prefix(&self.root) else {
            return false;
        };
        if !self.show_hidden
            && rel
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

impl Active {
    /// 監視の開始に失敗しても None を返すだけで、呼び出し側は
    /// 監視なしでアプリを動かし続けられるようにする。
    fn start(root: &Path) -> Option<Self> {
        let (tx, rx) = channel();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })
        .ok()?;
        watcher.watch(root, RecursiveMode::Recursive).ok()?;
        Some(Self {
            _watcher: watcher,
            rx,
        })
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
