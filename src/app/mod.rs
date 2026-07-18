mod keys;
mod mode;
mod mouse;

pub use mode::{Focus, InputKind, Mode, SETTINGS_ROWS, SettingsState};

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::layout::Rect;

use crate::config::Config;
use crate::git::{self, GitStatus};
use crate::tree::Tree;
use crate::viewer::{self, Viewer};
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
    // Nerd Font アイコン表示。起動時に確定し実行中は変わらない (判定は main 側)
    pub icons: bool,
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
    pub fn new(root: PathBuf, config: Config) -> Self {
        let tree = Tree::new(&root, config.show_hidden);
        // 監視の初期化に失敗しても (権限等) 監視なしで起動を続ける
        let watcher = FsWatcher::new(&root, config.show_hidden);
        let git = git::file_statuses(&root);
        let mut viewer = Viewer::new();
        viewer.wrap = config.wrap_default;
        // 設定ファイルのテーマ名が壊れていても set_theme が false を返すだけで、
        // Viewer::new() が入れた既定テーマのまま起動を続ける (パニックしない)
        viewer.set_theme(&config.theme);
        Self {
            root,
            focus: Focus::Tree,
            mode: Mode::Normal,
            tree,
            viewer,
            git,
            icons: config.icons,
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

    pub fn toggle_hidden(&mut self) {
        let show_hidden = self.tree.toggle_hidden(&self.root);
        // 既存 watcher のキューには切替前のフィルタ結果が残るため、監視も作り直して揃える。
        self.watcher = FsWatcher::new(&self.root, show_hidden);
        self.last_rescan = Instant::now();
        self.rescan_pending = false;
        self.persist_config();
    }

    pub fn toggle_icons(&mut self) {
        self.icons = !self.icons;
        self.persist_config();
    }

    pub fn toggle_wrap(&mut self) {
        self.viewer.toggle_wrap();
        self.persist_config();
    }

    /// delta の符号方向に THEME_NAMES を巡回する (設定画面の h/l 用)
    pub fn cycle_theme(&mut self, delta: isize) {
        let names = viewer::THEME_NAMES;
        let current = self.viewer.theme_name();
        let idx = names.iter().position(|n| *n == current).unwrap_or(0) as isize;
        let len = names.len() as isize;
        let next = (idx + delta).rem_euclid(len) as usize;
        self.viewer.set_theme(names[next]);
        self.persist_config();
    }

    fn current_config(&self) -> Config {
        Config {
            show_hidden: self.tree.show_hidden(),
            icons: self.icons,
            wrap_default: self.viewer.wrap,
            theme: self.viewer.theme_name().to_string(),
        }
    }

    // 保存失敗 (権限なし等) はここで握り潰す。読み取り専用ビューアの付随機能が
    // ファイル書き込み失敗でクラッシュ・エラー表示をする理由はない
    fn persist_config(&self) {
        let _ = self.current_config().save();
    }
}
