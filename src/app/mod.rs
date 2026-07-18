mod keys;
mod mode;
mod mouse;

pub use mode::{Focus, InputKind, Mode};

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::layout::Rect;

use crate::git::{self, GitStatus};
use crate::tree::Tree;
use crate::viewer::Viewer;
use crate::watch::FsWatcher;

// イベント嵐 (git checkout やビルド等) でツリーを毎回フル再走査しないための間引き間隔
const RESCAN_DEBOUNCE: Duration = Duration::from_millis(500);

pub struct App {
    pub root: PathBuf,
    pub focus: Focus,
    pub mode: Mode,
    pub tree: Tree,
    pub viewer: Viewer,
    // git repo でない / git 未インストールなら None のままで通常表示にフォールバックする
    pub git: Option<GitStatus>,
    pub should_quit: bool,
    // g 待ち状態。Mode を増やすほどのものではないので App の小さなフラグで持つ
    pub pending_g: bool,
    // マウスのヒットテスト用。ui::draw が毎フレーム書き戻す (viewport_height と同じパターン)
    pub tree_area: Rect,
    pub viewer_area: Rect,
    watcher: Option<FsWatcher>,
    last_rescan: Instant,
    rescan_pending: bool,
}

impl App {
    pub fn new(root: PathBuf) -> Self {
        let tree = Tree::new(&root);
        // 監視の初期化に失敗しても (権限等) 監視なしで起動を続ける
        let watcher = FsWatcher::new(&root);
        let git = git::file_statuses(&root);
        Self {
            root,
            focus: Focus::Tree,
            mode: Mode::Normal,
            tree,
            viewer: Viewer::new(),
            git,
            should_quit: false,
            pending_g: false,
            tree_area: Rect::default(),
            viewer_area: Rect::default(),
            watcher,
            last_rescan: Instant::now(),
            rescan_pending: false,
        }
    }

    /// watcher に溜まったファイル変更を取り込む。キー入力の有無に関わらず、
    /// イベントループの毎 tick (poll タイムアウト時も含む) で呼ばれる。
    pub fn on_tick(&mut self) {
        let Some(watcher) = &self.watcher else {
            return;
        };
        let changed = watcher.drain();
        let open_path = self.viewer.current.as_ref().map(|open| open.path.clone());

        for path in &changed {
            if open_path.as_deref() == Some(path.as_path()) {
                self.viewer.reload(path);
            } else {
                self.rescan_pending = true;
            }
        }

        if self.rescan_pending && self.last_rescan.elapsed() >= RESCAN_DEBOUNCE {
            self.rescan();
            self.last_rescan = Instant::now();
            self.rescan_pending = false;
        }
    }

    /// ツリーと git status をまとめて再取得する。FS 監視の間引き後と、
    /// 手動再走査 (r キー) の両方から呼ばれる共通処理。
    fn rescan(&mut self) {
        self.tree.rescan(&self.root);
        self.git = git::file_statuses(&self.root);
    }
}
