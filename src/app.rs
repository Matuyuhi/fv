use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tree::Tree;
use crate::viewer::Viewer;
use crate::watch::FsWatcher;

// イベント嵐 (git checkout やビルド等) でツリーを毎回フル再走査しないための間引き間隔
const RESCAN_DEBOUNCE: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Viewer,
}

pub struct App {
    pub root: PathBuf,
    pub focus: Focus,
    pub tree: Tree,
    pub viewer: Viewer,
    pub should_quit: bool,
    watcher: Option<FsWatcher>,
    last_rescan: Instant,
    rescan_pending: bool,
}

impl App {
    pub fn new(root: PathBuf) -> Self {
        let tree = Tree::new(&root);
        // 監視の初期化に失敗しても (権限等) 監視なしで起動を続ける
        let watcher = FsWatcher::new(&root);
        Self {
            root,
            focus: Focus::Tree,
            tree,
            viewer: Viewer::new(),
            should_quit: false,
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
            self.tree.rescan(&self.root);
            self.last_rescan = Instant::now();
            self.rescan_pending = false;
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return;
            }
            KeyCode::Char('c') if ctrl => {
                self.should_quit = true;
                return;
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Tree => Focus::Viewer,
                    Focus::Viewer => Focus::Tree,
                };
                return;
            }
            _ => {}
        }
        match self.focus {
            Focus::Tree => self.on_tree_key(key),
            Focus::Viewer => self.on_viewer_key(key, ctrl),
        }
    }

    fn on_tree_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.tree.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.tree.move_selection(-1),
            KeyCode::Enter => {
                if let Some(path) = self.tree.toggle_or_open() {
                    self.viewer.open(&path, &self.root);
                }
            }
            _ => {}
        }
    }

    fn on_viewer_key(&mut self, key: KeyEvent, ctrl: bool) {
        let half_page = (self.viewer.viewport_height / 2).max(1) as isize;
        match key.code {
            KeyCode::Char('d') if ctrl => self.viewer.scroll_by(half_page),
            KeyCode::Char('u') if ctrl => self.viewer.scroll_by(-half_page),
            KeyCode::Char('j') | KeyCode::Down => self.viewer.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.viewer.scroll_by(-1),
            _ => {}
        }
    }
}
