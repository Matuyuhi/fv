use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::editor::{EditOutcome, EditState};
use crate::finder::Finder;

use super::{App, Focus, InputKind, Mode, SETTINGS_ROWS, SettingsState};

impl App {
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
        if let Mode::Settings(_) = &self.mode {
            self.on_settings_key(key);
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
        // 編集中は q/s/Tab 等のグローバルキーも全て文字入力として扱うため、
        // ここより先のディスパッチには流さない (Ctrl+c だけが上で強制終了として残る)
        if let Mode::Edit(_) = &self.mode {
            self.on_edit_key(key);
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
            KeyCode::Char('a') => {
                self.pending_g = false;
                self.toggle_hidden();
                return;
            }
            KeyCode::Char('s') => {
                self.pending_g = false;
                self.mode = Mode::Settings(SettingsState::default());
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
                if let Mode::Input { buffer, .. } = &self.mode
                    && let Ok(line_no) = buffer.parse::<usize>()
                {
                    self.viewer.goto_line(line_no);
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

    fn on_edit_key(&mut self, key: KeyEvent) {
        // self.mode (EditState) と self.viewer は別フィールドなので同時に借りられる
        let Mode::Edit(state) = &mut self.mode else {
            return;
        };
        match state.handle_key(key, &mut self.viewer) {
            EditOutcome::Exit => {
                // 編集中は wrap を無視して hscroll を動かしているため、
                // wrap 閲覧に戻る時は「wrap 中は hscroll 0」の前提を復元する
                if self.viewer.wrap {
                    self.viewer.hscroll = 0;
                }
                self.mode = Mode::Normal;
            }
            EditOutcome::Continue => {}
        }
    }

    // Help 中は ?/Esc/q のいずれでも閉じる。それ以外は無視する (Ctrl+c は on_key 冒頭で処理済み)
    fn on_help_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            _ => {}
        }
    }

    // Settings 中は s/Esc/q のいずれでも閉じる。h/l/Enter は「選択行の値を変える」で統一し、
    // 方向が意味を持つ (テーマの巡回方向) のは h/l だけ。Enter は l と同じ「進む」扱いにする
    fn on_settings_key(&mut self, key: KeyEvent) {
        let Mode::Settings(state) = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Char('s') | KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down => {
                state.selected = (state.selected + 1) % SETTINGS_ROWS.len();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                state.selected = (state.selected + SETTINGS_ROWS.len() - 1) % SETTINGS_ROWS.len();
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => self.apply_settings_action(1),
            KeyCode::Char('h') | KeyCode::Left => self.apply_settings_action(-1),
            _ => {}
        }
    }

    fn apply_settings_action(&mut self, delta: isize) {
        let Mode::Settings(state) = &self.mode else {
            return;
        };
        let selected = state.selected;
        match selected {
            0 => self.toggle_hidden(),
            1 => self.toggle_icons(),
            2 => self.toggle_wrap(),
            3 => self.cycle_theme(delta),
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
            KeyCode::Char('w') if self.viewer.is_text() => self.toggle_wrap(),
            // 6 桁単位の水平スクロール。wrap 中は Viewer::hscroll_by 側で no-op になる
            KeyCode::Char('h') | KeyCode::Left if self.viewer.is_text() => {
                self.viewer.hscroll_by(-6)
            }
            KeyCode::Char('l') | KeyCode::Right if self.viewer.is_text() => {
                self.viewer.hscroll_by(6)
            }
            KeyCode::Char('0') if self.viewer.is_text() => self.viewer.hscroll_reset(),
            KeyCode::Char('e') if self.viewer.is_text() => {
                // 巨大ファイル・非 UTF-8・読込失敗は open が None を返し no-op になる
                if let Some(open) = &self.viewer.current
                    && let Some(state) =
                        EditState::open(&open.path, &self.viewer, self.viewer.scroll, &self.root)
                {
                    self.mode = Mode::Edit(state);
                }
            }
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
