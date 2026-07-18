use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::finder::Finder;
use crate::git::{self, GitStatus};
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
    // Ctrl+p ファジーファインダー。Input に押し込むと Search/Goto と挙動が絡み合うため独立させる
    Finder(Finder),
    // キーバインド一覧のオーバーレイ。状態を持たないので unit variant で十分
    Help,
}

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

    pub fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Ctrl+c は Input モード中でも終了させる
        if ctrl && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        if let Mode::Help = &self.mode {
            self.on_help_key(key);
            return;
        }
        if let Mode::Finder(_) = &self.mode {
            self.on_finder_key(key, ctrl);
            return;
        }
        if let Mode::Input { kind, .. } = &self.mode {
            let kind = *kind;
            self.on_input_key(kind, key);
            return;
        }
        // Input モード中は除き、どのフォーカスからでも起動する
        if ctrl && key.code == KeyCode::Char('p') {
            self.open_finder();
            return;
        }
        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return;
            }
            KeyCode::Char('?') => {
                self.mode = Mode::Help;
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

    /// マウス操作。Input/Finder 中はクリック位置の意味が入力欄と衝突するため無視する
    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        if !matches!(self.mode, Mode::Normal) {
            return;
        }
        // クリック/スクロールはどちらも文脈を切り替えうるので、キー入力の g 待ちと同様に破棄する
        self.pending_g = false;
        let pos = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.tree_area.contains(pos) {
                    self.focus = Focus::Tree;
                    self.click_tree_row(mouse.row);
                } else if self.viewer_area.contains(pos) {
                    self.focus = Focus::Viewer;
                }
            }
            MouseEventKind::ScrollUp => {
                if self.tree_area.contains(pos) {
                    self.tree.move_selection(-3);
                } else if self.viewer_area.contains(pos) {
                    self.viewer.scroll_by(-3);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.tree_area.contains(pos) {
                    self.tree.move_selection(3);
                } else if self.viewer_area.contains(pos) {
                    self.viewer.scroll_by(3);
                }
            }
            _ => {}
        }
    }

    // クリックされた画面行をツリーの selected に変換する。上枠1行分を引き、
    // ListState::offset() (直前フレームでのスクロールオフセット) を足して実際の行 index を求める。
    // 範囲外 (枠線や空行をクリックした場合) は選択を変えずフォーカス移動のみで終える
    fn click_tree_row(&mut self, row: u16) {
        let row = row as isize - self.tree_area.y as isize - 1
            + self.tree.list_state.offset() as isize;
        if row < 0 || row as usize >= self.tree.visible.len() {
            return;
        }
        self.tree.selected = row as usize;
        if let Some(path) = self.tree.toggle_or_open() {
            self.viewer.open(&path, &self.root);
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

    // Help 中は ?/Esc/q のいずれでも閉じる。それ以外は無視する (Ctrl+c は on_key 冒頭で処理済み)
    fn on_help_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            _ => {}
        }
    }

    // 候補は既存 tree の nodes から集めるだけで、新たな走査はしない
    fn open_finder(&mut self) {
        let candidates = self
            .tree
            .collect_file_paths(&self.root)
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        self.mode = Mode::Finder(Finder::new(candidates));
    }

    fn on_finder_key(&mut self, key: KeyEvent, ctrl: bool) {
        let Mode::Finder(finder) = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => {
                // finder (self.mode の借用) を使い切ってから self.mode へ書き戻す
                let path = finder.selected_path().map(|rel| self.root.join(rel));
                if let Some(path) = path {
                    self.viewer.open(&path, &self.root);
                    self.focus = Focus::Viewer;
                }
                self.mode = Mode::Normal;
            }
            KeyCode::Backspace => finder.backspace(),
            KeyCode::Down => finder.move_selection(1),
            KeyCode::Up => finder.move_selection(-1),
            KeyCode::Char('n') if ctrl => finder.move_selection(1),
            KeyCode::Char('p') if ctrl => finder.move_selection(-1),
            // ctrl 付きの印字キー (Ctrl+n/p 以外) はクエリに積まない
            KeyCode::Char(c) if !ctrl => finder.push_char(c),
            _ => {}
        }
    }

    fn on_tree_key(&mut self, key: KeyEvent) {
        // g 待ち状態は viewer と同じフラグを共用する (Tab を跨ぐと on_key 側で破棄される)
        if self.pending_g {
            self.pending_g = false;
            if key.code == KeyCode::Char('g') {
                self.tree.select_top();
                return;
            }
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.tree.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.tree.move_selection(-1),
            KeyCode::Enter => {
                if let Some(path) = self.tree.toggle_or_open() {
                    self.viewer.open(&path, &self.root);
                }
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if let Some(path) = self.tree.expand_or_enter() {
                    self.viewer.open(&path, &self.root);
                }
            }
            KeyCode::Char('h') | KeyCode::Left => self.tree.collapse_or_parent(),
            KeyCode::Char('H') => self.tree.select_parent_and_collapse(),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => self.tree.select_bottom(),
            // 手動再走査。FS 監視のデバウンスは効かないので直後の自動再走査は起こさないよう
            // タイマーもここで揃えておく
            KeyCode::Char('r') => {
                self.rescan();
                self.last_rescan = Instant::now();
                self.rescan_pending = false;
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
            // Ctrl+o: 履歴を戻る。Backspace は同じ操作の代替キー
            KeyCode::Char('o') if ctrl => self.viewer.back(),
            KeyCode::Backspace => self.viewer.back(),
            // Ctrl+i: 履歴を進む。多くの端末では Ctrl+i が Tab (0x09) と同一バイトで届き
            // KeyCode::Tab として解釈されるため、この分岐が発火しない環境がある。
            // Tab はフォーカス切り替えに使っているため奪えず、この制約は許容する
            KeyCode::Char('i') if ctrl => self.viewer.forward(),
            KeyCode::Char('j') | KeyCode::Down => self.viewer.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.viewer.scroll_by(-1),
            KeyCode::Char('w') if self.viewer.is_text() => self.viewer.toggle_wrap(),
            // 6 桁単位の水平スクロール。wrap 中は Viewer::hscroll_by 側で no-op になる
            KeyCode::Char('h') | KeyCode::Left if self.viewer.is_text() => {
                self.viewer.hscroll_by(-6)
            }
            KeyCode::Char('l') | KeyCode::Right if self.viewer.is_text() => {
                self.viewer.hscroll_by(6)
            }
            KeyCode::Char('0') if self.viewer.is_text() => self.viewer.hscroll_reset(),
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
