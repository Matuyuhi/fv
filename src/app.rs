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

// Search と Goto (:N 行ジャンプ) の入力を kind で分ける
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Search,
    Goto,
}

pub enum Mode {
    Normal,
    Input { kind: InputKind, buffer: String },
}

pub struct App {
    pub root: PathBuf,
    pub focus: Focus,
    pub mode: Mode,
    pub tree: Tree,
    pub viewer: Viewer,
    pub should_quit: bool,
    // g 待ち状態。Mode を増やすほどのものではないので App の小さなフラグで持つ
    pub pending_g: bool,
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
            mode: Mode::Normal,
            tree,
            viewer: Viewer::new(),
            should_quit: false,
            pending_g: false,
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
        // Ctrl+c は Input モード中でも終了させる
        if ctrl && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        if let Mode::Input { kind, .. } = &self.mode {
            let kind = *kind;
            self.on_input_key(kind, key);
            return;
        }
        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return;
            }
            KeyCode::Tab => {
                // フォーカスを跨ぐと g 待ちの文脈は失われるので破棄する
                self.pending_g = false;
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

    // Input モード中は q も含め全ての印字キーを buffer に積む。Esc でキャンセル、Enter で確定
    fn on_input_key(&mut self, kind: InputKind, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.cancel_input(kind);
            }
            KeyCode::Enter => {
                // Goto は confirm 時に buffer を読むので、Mode を Normal に戻す前に確定処理を行う
                self.confirm_input(kind);
                self.mode = Mode::Normal;
            }
            KeyCode::Backspace => {
                if let Mode::Input { buffer, .. } = &mut self.mode {
                    buffer.pop();
                }
                self.live_update_input(kind);
            }
            KeyCode::Char(c) => {
                // Goto は行番号入力なので数字以外は無視する
                if kind == InputKind::Goto && !c.is_ascii_digit() {
                    return;
                }
                if let Mode::Input { buffer, .. } = &mut self.mode {
                    buffer.push(c);
                }
                self.live_update_input(kind);
            }
            _ => {}
        }
    }

    fn cancel_input(&mut self, kind: InputKind) {
        match kind {
            InputKind::Search => self.viewer.cancel_search(),
            // Goto は確定時にしか状態を変えないので、キャンセル時に戻すものがない
            InputKind::Goto => {}
        }
    }

    fn confirm_input(&mut self, kind: InputKind) {
        match kind {
            InputKind::Search => self.viewer.confirm_search(),
            InputKind::Goto => {
                // buffer は数字のみ。空文字列や "0" は parse/goto_line 側で no-op になる
                if let Mode::Input { buffer, .. } = &self.mode {
                    if let Ok(line_no) = buffer.parse::<usize>() {
                        self.viewer.goto_line(line_no);
                    }
                }
            }
        }
    }

    fn live_update_input(&mut self, kind: InputKind) {
        match kind {
            InputKind::Search => {
                if let Mode::Input { buffer, .. } = &self.mode {
                    let query = buffer.clone();
                    self.viewer.update_search(&query);
                }
            }
            // Goto はステータスバーが buffer をそのまま表示するのでライブ更新は不要
            InputKind::Goto => {}
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
        // g 待ち状態: 続く g で先頭へ。それ以外のキーは待ちを解除した上で下の通常処理に流す
        if self.pending_g {
            self.pending_g = false;
            if key.code == KeyCode::Char('g') && self.viewer.is_text() {
                self.viewer.jump_to_top();
                return;
            }
        }
        let half_page = (self.viewer.viewport_height / 2).max(1) as isize;
        match key.code {
            KeyCode::Char('d') if ctrl => self.viewer.scroll_by(half_page),
            KeyCode::Char('u') if ctrl => self.viewer.scroll_by(-half_page),
            KeyCode::Char('j') | KeyCode::Down => self.viewer.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.viewer.scroll_by(-1),
            KeyCode::Char('g') if self.viewer.is_text() => self.pending_g = true,
            KeyCode::Char('G') if self.viewer.is_text() => self.viewer.jump_to_bottom(),
            KeyCode::Char(':') if self.viewer.is_text() => {
                self.mode = Mode::Input {
                    kind: InputKind::Goto,
                    buffer: String::new(),
                };
            }
            KeyCode::Char('/') if self.viewer.is_text() => {
                self.mode = Mode::Input {
                    kind: InputKind::Search,
                    buffer: String::new(),
                };
            }
            // 未確定 (Enter していない) 状態では no-op。Viewer::next_match/prev_match が保証する
            KeyCode::Char('n') => self.viewer.next_match(),
            KeyCode::Char('N') => self.viewer.prev_match(),
            _ => {}
        }
    }
}
