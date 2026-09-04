use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::event::{EventKind, ModifyKind};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::component::tree::ScanOptions;

/// root を再帰監視し、変更パスをためておくキューを持つ。
/// 再帰監視の登録 (inotify では配下のディレクトリ 1 つずつに watch を張る) は
/// ツリーが大きいほど時間がかかるため別スレッドで行い、起動を待たせない。
/// 登録が終わるまでのイベントは取りこぼすが、それは監視開始前と同じ状態でしかない
pub struct FsWatcher {
    state: State,
    root: PathBuf,
    ignore: Option<Gitignore>,
    opts: ScanOptions,
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
    pub fn new(root: &Path, opts: ScanOptions) -> Self {
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
            opts,
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
        let mut ignore_changed = false;
        while let Ok(res) = active.rx.try_recv() {
            let Ok(event) = res else { continue };
            // キューが溢れて取りこぼした (inotify の overflow 等)。何が変わったか分からないので
            // root 全体の構造変化として通す — 横断検索はこれを見て前回の一覧を信用しなくなる
            if event.need_rescan() {
                changes.push(Change {
                    path: self.root.clone(),
                    structural: true,
                    overflow: true,
                });
                continue;
            }
            let Some(structural) = classify(&event.kind) else {
                continue;
            };
            for path in event.paths {
                // 無視設定そのものの変更は、どのファイルが対象かを丸ごと変えるので常に構造変化
                // として通す (隠しファイルとして落とさない)。横断検索の一覧はこれで信用を失う
                if is_ignore_config(&self.root, &path) {
                    ignore_changed = true;
                    changes.push(Change {
                        path,
                        structural: true,
                        overflow: false,
                    });
                } else if !self.is_ignored(&path) {
                    changes.push(Change {
                        path,
                        structural,
                        overflow: false,
                    });
                }
            }
        }
        // 無視設定が変わったら間引きの matcher も作り直す。起動時のままだと、除外規則を外して
        // 新しく表示対象になったファイルの変更通知を古い規則で落とし続ける
        if ignore_changed {
            self.ignore = build_gitignore(&self.root);
        }
        changes
    }

    /// 監視が張られていて、以後の変更が必ず届く状態か。横断検索が「前回の一覧を歩き直さずに
    /// 使ってよいか」の根拠にする (登録前・失敗時は false)
    pub fn is_active(&mut self) -> bool {
        self.adopt();
        matches!(self.state, State::Active(_))
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
        if !self.opts.show_hidden
            && rel
                .iter()
                .any(|component| component.to_string_lossy().starts_with('.'))
        {
            return true;
        }
        // 無視ファイルも表示している間は、その変更もツリー・ビューアへ反映する必要がある
        // (表示しているのに自動リロードだけ効かない、を避ける)
        if self.opts.show_ignored {
            return false;
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

/// 走査側 (ignore クレート) が読む無視設定ファイルか: 各階層の .gitignore / .ignore と
/// root の .git/info/exclude。root 外の global gitignore は監視できないので、横断検索側が
/// 指紋 (mtime, size) で別途照合する
fn is_ignore_config(root: &Path, path: &Path) -> bool {
    if path == root.join(".git").join("info").join("exclude") {
        return true;
    }
    matches!(
        path.file_name().and_then(|n| n.to_str()),
        Some(".gitignore" | ".ignore")
    )
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
    /// 監視のキューが溢れて何が変わったか分からない (path は root)。呼び出し側は「全部が
    /// 変わったかもしれない」として扱う — path 単位の後始末 (開いているファイルの reload・
    /// cache からの削除) では、取りこぼした変更が表示中のファイルだった場合に古いままになる
    pub overflow: bool,
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

// 走査側 (ScanOptions::walker) が見る無視ファイルと同じ 3 種を読む。root の .gitignore だけだと
// .ignore / .git/info/exclude で無視したパスの変更イベントが素通りし、ツリーに出ないファイルの
// ために status 再取得・再走査が走ってしまう。
// 下の階層の .gitignore までは追わない — ここはあくまでイベントの間引きで、取りこぼした側の
// コストは 500ms デバウンス済みの再取得 1 回でしかないため (逆に消しすぎると表示が古いまま
// 固定されるので、判断に迷う側は通す方へ倒す)
fn build_gitignore(root: &Path) -> Option<Gitignore> {
    let mut builder = GitignoreBuilder::new(root);
    let mut found = false;
    for path in [
        root.join(".gitignore"),
        root.join(".ignore"),
        root.join(".git").join("info").join("exclude"),
    ] {
        if path.is_file() && builder.add(&path).is_none() {
            found = true;
        }
    }
    if !found {
        return None;
    }
    builder.build().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::tree::ScanOptions;

    // 走査側と同じ 3 種の無視ファイルでイベントを間引けているか。root の .gitignore しか
    // 見ていないと「ツリーに出ないファイルの変更で再走査が走る」に戻る
    #[test]
    fn filters_events_from_every_ignore_source() {
        let root = std::env::temp_dir().join("fv-watch-ignore-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".git/info")).unwrap();
        std::fs::write(root.join(".gitignore"), "/target\n").unwrap();
        std::fs::write(root.join(".ignore"), "notes.md\n").unwrap();
        std::fs::write(root.join(".git/info/exclude"), "*.bak\n").unwrap();

        let watcher = FsWatcher::new(
            &root,
            ScanOptions {
                show_hidden: false,
                show_ignored: false,
            },
        );
        assert!(watcher.is_ignored(&root.join("target/debug/fv")));
        assert!(watcher.is_ignored(&root.join("notes.md")));
        assert!(watcher.is_ignored(&root.join("src/main.rs.bak")));
        assert!(!watcher.is_ignored(&root.join("src/main.rs")));
        // 無視設定そのものは隠しファイルでも構造変化として通す
        assert!(is_ignore_config(&root, &root.join(".gitignore")));
        assert!(is_ignore_config(&root, &root.join("src/.gitignore")));
        assert!(is_ignore_config(&root, &root.join(".ignore")));
        assert!(is_ignore_config(&root, &root.join(".git/info/exclude")));
        assert!(!is_ignore_config(&root, &root.join(".git/index")));

        // 無視ファイルを表示している間は、その変更も追従させたいので通す
        let showing = FsWatcher::new(
            &root,
            ScanOptions {
                show_hidden: false,
                show_ignored: true,
            },
        );
        assert!(!showing.is_ignored(&root.join("notes.md")));

        let _ = std::fs::remove_dir_all(&root);
    }
}
