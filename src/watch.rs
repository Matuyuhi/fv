use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::event::{EventKind, ModifyKind};
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

    /// 溜まったイベントを非ブロッキングで全部取り出す。
    /// .git 配下や .gitignore にマッチするパスはここで除外する。
    pub fn drain(&mut self) -> Vec<Change> {
        self.adopt();
        let State::Active(active) = &self.state else {
            return Vec::new();
        };
        let mut changes = Vec::new();
        while let Ok(res) = active.rx.try_recv() {
            let Ok(event) = res else { continue };
            let Some(structural) = classify(&event.kind) else {
                continue;
            };
            for path in event.paths {
                if !self.is_ignored(&path) {
                    changes.push(Change { path, structural });
                }
            }
        }
        changes
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

/// 中身が変わったと見なすイベントだけ通し、**ツリーの構造 (作成・削除・リネーム) を変えるか**
/// を Some の中身 (structural) で表す。None は完全に無視するイベント (Access・chmod 等)。
/// **Access と Modify(Metadata) を落とすのが要点**で、通してしまうと「開いているファイルを
/// reload する → 読んだことで atime が更新されてまた通知が来る → reload」の自走ループになり、
/// 何もしていないのに再ハイライトと git 呼び出しを 100ms ごとに繰り返して CPU を焼き続ける。
/// chmod だけの変更 (Metadata) がツリーの status に反映されなくなるが、
/// 内容を伴う操作なら別のイベントが必ず来るので実害は無い。
/// Modify(Data) だけ structural=false にする — ファイルの中身が変わってもツリーの行構成
/// (どのパスが存在するか) は変わらないため、呼び出し側はここだけ WalkBuilder の全走査を
/// 省略できる。種別が判別できない Modify (Rename 以外の Any 等) は「構造が変わったかもしれない」
/// 側に倒し、全走査をスキップして表示が古いまま固定される事故を避ける
fn classify(kind: &EventKind) -> Option<bool> {
    match kind {
        EventKind::Create(_) | EventKind::Remove(_) => Some(true),
        EventKind::Modify(ModifyKind::Metadata(_)) => None,
        EventKind::Modify(ModifyKind::Data(_)) => Some(false),
        EventKind::Modify(_) => Some(true),
        _ => None,
    }
}

/// FS 監視 1 件分の変更。`structural` は呼び出し側 (App::on_tick) が全走査を要するか
/// (作成・削除・リネーム) か、git status の再取得だけで足りる内容変更かを判定するのに使う
pub struct Change {
    pub path: PathBuf,
    pub structural: bool,
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
