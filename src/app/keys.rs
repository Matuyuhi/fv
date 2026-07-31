use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::editor::EditOutcome;
use crate::finder::Finder;
use crate::git;

use super::{
    App, ConfirmAction, Focus, InputKind, Lane, Mode, SETTINGS_ROWS, SettingsState, Workspace,
};

impl App {
    pub fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Ctrl+c は Input モード中でも終了させる
        if ctrl && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        // オーバーレイ (Mode) はレーンより先に処理する。ここで Shift+Tab を通さないことで、
        // 入力中にレーンが切り替わって文脈が壊れるのを防ぐ
        if let Mode::Confirm { .. } = &self.mode {
            self.on_confirm_key(key);
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
        // Ctrl+t / Alt+1..3: Workspace タブ切替。Shift+Tab と同じ位置 (オーバーレイ判定の後・
        // Lane::Edit の前) でルーティングする。印字キーではないので編集中の文字入力ポリシーと
        // 衝突しない。使えない間 (workspace_available が false) は無効時の挙動を一切変えないため
        // ここで素通りさせる
        if self.workspace_available() {
            if ctrl && key.code == KeyCode::Char('t') {
                self.cycle_workspace();
                return;
            }
            if key.modifiers.contains(KeyModifiers::ALT) {
                match key.code {
                    KeyCode::Char('1') => {
                        self.set_workspace(Workspace::Viewer);
                        return;
                    }
                    KeyCode::Char('2') => {
                        self.set_workspace(Workspace::Issues);
                        return;
                    }
                    KeyCode::Char('3') => {
                        self.set_workspace(Workspace::PullRequests);
                        return;
                    }
                    _ => {}
                }
            }
        }
        // Shift+Tab は Edit レーンより前に処理する。印字キーではないので
        // 「編集中は印字キーを全て文字入力にする」ポリシーとは衝突しない。
        // 端末によっては Tab + SHIFT で届くため両方を受ける
        if key.code == KeyCode::BackTab
            || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT))
        {
            self.cycle_lane();
            return;
        }
        // Issues/PR タブは #33/#34 までプレースホルダ。Lane/ツリー/ビューアの概念を持たないので
        // 以降のディスパッチには流さず、共通のグローバルキーだけをここで拾う
        if !matches!(self.workspace, Workspace::Viewer) {
            self.on_workspace_key(key);
            return;
        }
        // 編集中は q/s/Tab 等のグローバルキーも全て文字入力として扱うため、
        // ここより先のディスパッチには流さない (Ctrl+c と Shift+Tab だけが上に残る)
        if let Lane::Edit(_) = &self.lane {
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
        // stash pop (#25) だけは GIT レーンに縛らない。z (push) は変更を全部退避すると
        // git_available が false になり GIT レーンへ再入場できなくなるため、「push した直後に
        // pop で戻れない」事故を避ける必要がある。git repo でありさえすれば (LOG と同じ
        // log_available 基準) どのレーンからでも呼べるようにする
        if self.log_available() && key.code == KeyCode::Char('Z') {
            self.confirm_stash_pop();
            return;
        }
        // discard/stash push (#25) は GIT レーン限定。対象は Focus に関わらず tree.selected
        // (Space のトグルと同じ考え方) なので、Focus::Tree/Viewer どちらでも同じ挙動にするため
        // focus 別ディスパッチより前で拾う
        if let Lane::Git(_) = &self.lane {
            match key.code {
                KeyCode::Char('X') => {
                    self.confirm_discard();
                    return;
                }
                KeyCode::Char('z') => {
                    self.confirm_stash_push();
                    return;
                }
                _ => {}
            }
        }
        match self.focus {
            // ツリーのキー操作は VIEW / GIT で共通。開く先だけレーンで振り分ける。
            // LOG は左ペインがツリーではなくコミット一覧なので専用ハンドラに分ける
            Focus::Tree => match &self.lane {
                Lane::Log(_) => self.on_log_list_key(key),
                Lane::Git(_) => {
                    // Space (stage/unstage トグル) は GIT レーン限定。on_tree_key は VIEW とも
                    // 共用するハンドラなので、ここで先に拾って VIEW 側の意味 (no-op) を変えない
                    if key.code == KeyCode::Char(' ') {
                        self.toggle_stage_selected();
                        return;
                    }
                    if let Some(path) = self.on_tree_key(key) {
                        self.open_selected(&path);
                    }
                }
                _ => {
                    if let Some(path) = self.on_tree_key(key) {
                        self.open_selected(&path);
                    }
                }
            },
            Focus::Viewer => match &self.lane {
                Lane::Git(_) => self.on_git_key(key, ctrl),
                Lane::Log(_) => self.on_log_diff_key(key, ctrl),
                _ => self.on_viewer_key(key, ctrl),
            },
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

    // on_paste (mod.rs) からも呼ぶため pub(super)
    pub(super) fn live_update_input(&mut self, kind: InputKind) {
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

    // Issues/PR タブ (プレースホルダ) 中に拾うグローバルキー。ツリー・ビューア相当の操作は
    // まだ中身が無いので受けない (#33/#34 で個別のハンドラに置き換わる)
    fn on_workspace_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('s') => self.mode = Mode::Settings(SettingsState::default()),
            _ => {}
        }
    }

    fn on_edit_key(&mut self, key: KeyEvent) {
        // self.lane (EditState) と self.viewer は別フィールドなので同時に借りられる
        let Lane::Edit(state) = &mut self.lane else {
            return;
        };
        // 「wrap 中は hscroll = 0」は Viewport のメソッドと EditState::ensure_visible が
        // 維持するため、閲覧へ戻る際の後始末は不要
        match state.handle_key(key, &mut self.viewer) {
            EditOutcome::Exit => self.lane = Lane::View,
            EditOutcome::Continue => {}
        }
    }

    // 確認中は y/Enter でのみ action を実行する。n/Esc/それ以外の全キーは中止として扱い、
    // どのレーンにも流さない (キー入力による事故実行を防ぐのが目的なので誤操作は必ず中止側に倒す)
    fn on_confirm_key(&mut self, key: KeyEvent) {
        if !matches!(key.code, KeyCode::Char('y') | KeyCode::Enter) {
            self.mode = Mode::Normal;
            return;
        }
        let Mode::Confirm { action, .. } = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return;
        };
        self.run_confirm_action(action);
    }

    fn run_confirm_action(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::Discard { path, is_dir } => self.execute_discard(path, is_dir),
            ConfirmAction::StashPush => self.execute_stash_push(),
            ConfirmAction::StashPop => self.execute_stash_pop(),
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
            4 => self.toggle_github(),
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
                self.mode = Mode::Normal;
                if let Some(path) = path {
                    self.open_selected(&path);
                    self.focus = Focus::Viewer;
                }
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

    /// ツリーから「開く」操作が来たときのファイルパスを返す。開く先はレーンで変わるため
    /// ここでは viewer を直接触らず、呼び出し側 (open_selected) に委ねる
    fn on_tree_key(&mut self, key: KeyEvent) -> Option<PathBuf> {
        // g 待ち状態は viewer と同じフラグを共用する (Tab を跨ぐと on_key 側で破棄される)
        if self.pending_g {
            self.pending_g = false;
            if key.code == KeyCode::Char('g') {
                self.tree.select_top();
                return None;
            }
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.tree.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.tree.move_selection(-1),
            KeyCode::Enter => return self.tree.toggle_or_open(),
            KeyCode::Char('l') | KeyCode::Right => return self.tree.expand_or_enter(),
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
        None
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
        let half_page = (self.viewer.viewport.height / 2).max(1) as isize;
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
            // e は Shift+Tab と別の直接入口。入れない条件は enter_edit が吸収する
            KeyCode::Char('e') if self.viewer.is_text() => {
                self.enter_edit();
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

    // GIT レーンの diff ペイン。検索・履歴・編集は持たないぶん、n/N は hunk ジャンプに使う
    fn on_git_key(&mut self, key: KeyEvent, ctrl: bool) {
        if self.pending_g {
            self.pending_g = false;
            if key.code == KeyCode::Char('g') {
                if let Lane::Git(git) = &mut self.lane {
                    git.jump_to_top();
                }
                return;
            }
        }
        // cycle_base は root を必要とするが、Lane::Git 経由の可変借用と
        // self.root の借用が衝突しないよう rescan() と同じく先に clone しておく
        let root = self.root.clone();
        let Lane::Git(git) = &mut self.lane else {
            return;
        };
        let half_page = (git.viewport.height / 2).max(1) as isize;
        match key.code {
            KeyCode::Char('d') if ctrl => git.scroll_by(half_page),
            KeyCode::Char('u') if ctrl => git.scroll_by(-half_page),
            KeyCode::Char('j') | KeyCode::Down => git.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => git.scroll_by(-1),
            // diff は VIEW とは別ドキュメントなので折返しも独立させる (config には保存しない)
            KeyCode::Char('w') => git.viewport.toggle_wrap(),
            KeyCode::Char('h') | KeyCode::Left => git.hscroll_by(-6),
            KeyCode::Char('l') | KeyCode::Right => git.hscroll_by(6),
            KeyCode::Char('0') => git.hscroll_reset(),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => git.jump_to_bottom(),
            KeyCode::Char('n') | KeyCode::Char(']') => git.next_hunk(),
            KeyCode::Char('N') | KeyCode::Char('[') => git.prev_hunk(),
            // diff 基準の循環 (HEAD → staged → unstaged)。config には保存しない
            KeyCode::Char('t') => git.cycle_base(&root),
            _ => {}
        }
    }

    // LOG レーンの左ペイン (コミット一覧)。j/k は移動のみで diff は開かない
    // (GIT のツリーと同じ理由でキーリピート時に git show を連打しないため)
    fn on_log_list_key(&mut self, key: KeyEvent) {
        if self.pending_g {
            self.pending_g = false;
            if key.code == KeyCode::Char('g') {
                if let Lane::Log(log) = &mut self.lane {
                    log.select_top();
                }
                return;
            }
        }
        let root = self.root.clone();
        let Lane::Log(log) = &mut self.lane else {
            return;
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => log.move_selection(&root, 1),
            KeyCode::Char('k') | KeyCode::Up => log.move_selection(&root, -1),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => log.open_selected(&root),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => log.select_bottom(&root),
            _ => {}
        }
    }

    // LOG レーンの右ペイン (選択コミットの diff)。GIT の diff ペインと同じ操作感だが
    // 基準の切替 (t) は無い (コミットの diff は HEAD/staged のような基準を持たない)
    fn on_log_diff_key(&mut self, key: KeyEvent, ctrl: bool) {
        if self.pending_g {
            self.pending_g = false;
            if key.code == KeyCode::Char('g') {
                if let Lane::Log(log) = &mut self.lane {
                    log.jump_to_top();
                }
                return;
            }
        }
        let Lane::Log(log) = &mut self.lane else {
            return;
        };
        let half_page = (log.viewport.height / 2).max(1) as isize;
        match key.code {
            KeyCode::Char('d') if ctrl => log.scroll_by(half_page),
            KeyCode::Char('u') if ctrl => log.scroll_by(-half_page),
            KeyCode::Char('j') | KeyCode::Down => log.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => log.scroll_by(-1),
            KeyCode::Char('w') => log.viewport.toggle_wrap(),
            KeyCode::Char('h') | KeyCode::Left => log.hscroll_by(-6),
            KeyCode::Char('l') | KeyCode::Right => log.hscroll_by(6),
            KeyCode::Char('0') => log.hscroll_reset(),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => log.jump_to_bottom(),
            KeyCode::Char('n') | KeyCode::Char(']') => log.next_hunk(),
            KeyCode::Char('N') | KeyCode::Char('[') => log.prev_hunk(),
            _ => {}
        }
    }

    /// Space: 選択中のファイル/ディレクトリを stage/unstage トグルする。判定は
    /// 「worktree 側に未ステージ変更が残っているか」で、残っていれば stage、無ければ unstage
    /// (issue #23 の要求通り)。ディレクトリは配下の files を集約して同じ判定に使う
    fn toggle_stage_selected(&mut self) {
        // キーリピート対策。debounce 中の呼び出しは git プロセスを起動せずに捨てる
        if self.last_stage_toggle.elapsed() < super::STAGE_DEBOUNCE {
            return;
        }
        let Some(row) = self.tree.visible.get(self.tree.selected) else {
            return;
        };
        let path = row.path.clone();
        let is_dir = row.is_dir;
        let Some(status) = &self.git else {
            return;
        };
        let (has_worktree_change, has_deletion) = if is_dir {
            let mut worktree = false;
            let mut deletion = false;
            for (p, s) in &status.files {
                if !p.starts_with(&path) {
                    continue;
                }
                worktree |= s.worktree.is_some();
                deletion |= s.worktree == Some(git::StatusKind::Deleted)
                    || s.index == Some(git::StatusKind::Deleted);
            }
            (worktree, deletion)
        } else {
            let Some(s) = status.files.get(&path) else {
                // フィルタに乗っているのに status が無いのは通常起こらない (rescan 直後の
                // ズレなど)。何もせず次の rescan を待つ
                return;
            };
            (
                s.worktree.is_some(),
                s.worktree == Some(git::StatusKind::Deleted)
                    || s.index == Some(git::StatusKind::Deleted),
            )
        };
        self.last_stage_toggle = Instant::now();
        let outcome = if has_worktree_change {
            git::stage_path(&self.root, &path, has_deletion)
        } else {
            git::unstage_path(&self.root, &path, is_dir)
        };
        if outcome.ok {
            // rescan は選択位置を path ベースで維持する (Tree::rescan/set_filter の既存の
            // 復元経路をそのまま使う)。ツリーの XY 表示・絞り込み・diff の全てがここで揃う
            self.rescan();
            self.last_rescan = Instant::now();
            self.rescan_pending = false;
        } else {
            let message = if outcome.message.is_empty() {
                "git の実行に失敗しました".to_string()
            } else {
                outcome.message
            };
            self.set_notice(message, true);
        }
    }

    // 未保存の編集バッファがある間は破棄・stash を実行させない (ディスクを書き換えると
    // 編集内容と食い違うため)。X/z/Z は Lane::Git 限定でしか到達せず、Lane は同時に
    // 一つしか無いため現行のキー経路では実質 true にならないが、issue #25 の安全側の
    // 作法として明示的に防御しておく (belt and suspenders)
    fn refuse_if_edit_dirty(&mut self) -> bool {
        if let Lane::Edit(state) = &self.lane
            && state.buffer.dirty()
        {
            self.set_notice(
                "未保存の変更があります (保存または破棄してから実行してください)",
                true,
            );
            return true;
        }
        false
    }

    // 対象ファイル数と untracked を含むかどうかを集計する。confirm 時の prompt 生成と
    // execute 時の実行対象決定の両方から呼ぶ共通ロジック (件数がずれると事故るため 1 箇所にする)
    fn discard_summary(&self, path: &Path, is_dir: bool) -> Option<(usize, bool)> {
        let status = self.git.as_ref()?;
        let mut count = 0usize;
        let mut has_untracked = false;
        for (p, s) in &status.files {
            let hit = if is_dir {
                p.starts_with(path)
            } else {
                p == path
            };
            if !hit {
                continue;
            }
            count += 1;
            if s.index == Some(git::StatusKind::Untracked) {
                has_untracked = true;
            }
        }
        if count == 0 {
            None
        } else {
            Some((count, has_untracked))
        }
    }

    /// X: 選択ファイル/ディレクトリの変更破棄を確認オーバーレイに乗せる。実行そのものは
    /// execute_discard (y/Enter 確定後) が行う
    fn confirm_discard(&mut self) {
        if self.refuse_if_edit_dirty() {
            return;
        }
        let Some(row) = self.tree.visible.get(self.tree.selected) else {
            return;
        };
        let path = row.path.clone();
        let is_dir = row.is_dir;
        let Some((count, has_untracked)) = self.discard_summary(&path, is_dir) else {
            return;
        };
        let mut prompt = format!("{count} 件の変更を破棄しますか？\n{}", path.display());
        if has_untracked {
            prompt.push_str("\n(untracked ファイルは削除されます。破棄すると復元できません)");
        }
        self.mode = Mode::Confirm {
            prompt,
            action: ConfirmAction::Discard { path, is_dir },
        };
    }

    fn execute_discard(&mut self, path: PathBuf, is_dir: bool) {
        if self.refuse_if_edit_dirty() {
            return;
        }
        let Some(status) = &self.git else {
            return;
        };
        // tracked (restore 対象) と untracked (削除対象) を先に分けておく。ディレクトリは
        // 配下をまとめて扱う (issue の「パスをそのまま渡す」方針どおり restore はディレクトリ
        // ごと 1 回呼べば済むが、untracked の削除は git がタッチしないので個別に消す)
        let mut untracked_files = Vec::new();
        let mut has_tracked = false;
        for (p, s) in &status.files {
            let hit = if is_dir {
                p.starts_with(&path)
            } else {
                p == &path
            };
            if !hit {
                continue;
            }
            if s.index == Some(git::StatusKind::Untracked) {
                untracked_files.push(p.clone());
            } else {
                has_tracked = true;
            }
        }
        // Confirm 表示中の 500ms デバウンス再取得で対象が消えている可能性がある (稀)。
        // 「何もせず成功扱い」にしないよう、対象なしは明示的にエラー扱いで知らせる
        if !has_tracked && untracked_files.is_empty() {
            self.set_notice("破棄対象が見つかりませんでした", true);
            return;
        }
        let mut ok = true;
        let mut message = String::new();
        if has_tracked {
            let outcome = git::discard_path(&self.root, &path, is_dir);
            if !outcome.ok {
                ok = false;
                message = outcome.message;
            }
        }
        if ok {
            for file in &untracked_files {
                if let Err(err) = std::fs::remove_file(file) {
                    ok = false;
                    message = err.to_string();
                    break;
                }
            }
        }
        if ok {
            self.reload_if_affected(&path, is_dir);
            self.rescan();
            self.last_rescan = Instant::now();
            self.rescan_pending = false;
            self.refresh_git_diff_selection();
            self.set_notice("変更を破棄しました", false);
        } else {
            let message = if message.is_empty() {
                "破棄に失敗しました".to_string()
            } else {
                message
            };
            self.set_notice(message, true);
        }
    }

    /// z: stash push (-u) を確認オーバーレイに乗せる
    fn confirm_stash_push(&mut self) {
        if self.refuse_if_edit_dirty() {
            return;
        }
        let count = self.git.as_ref().map_or(0, |status| status.files.len());
        if count == 0 {
            return;
        }
        let prompt = format!(
            "{count} 件の変更を stash に退避しますか？\n(untracked ファイルも含めて退避します)"
        );
        self.mode = Mode::Confirm {
            prompt,
            action: ConfirmAction::StashPush,
        };
    }

    fn execute_stash_push(&mut self) {
        if self.refuse_if_edit_dirty() {
            return;
        }
        // stash 自体が git 上で日時を持つため、message は識別用のラベルで十分
        // (chrono 等の新規依存を増やさないよう epoch 秒のまま出す)
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let message = format!("fv: {secs}");
        let outcome = git::run_git_write(&self.root, ["stash", "push", "-u", "-m", &message]);
        if outcome.ok {
            self.reload_current_view();
            self.rescan();
            self.last_rescan = Instant::now();
            self.rescan_pending = false;
            self.refresh_git_diff_selection();
            self.set_notice("変更を stash に退避しました", false);
        } else {
            let message = if outcome.message.is_empty() {
                "stash push に失敗しました".to_string()
            } else {
                outcome.message
            };
            self.set_notice(message, true);
        }
    }

    /// Z: stash pop を確認オーバーレイに乗せる
    fn confirm_stash_pop(&mut self) {
        if self.refuse_if_edit_dirty() {
            return;
        }
        let prompt =
            "直近の stash を pop しますか？\n(コンフリクト時は stash を残したままエラーを表示します)"
                .to_string();
        self.mode = Mode::Confirm {
            prompt,
            action: ConfirmAction::StashPop,
        };
    }

    fn execute_stash_pop(&mut self) {
        if self.refuse_if_edit_dirty() {
            return;
        }
        let outcome = git::run_git_write(&self.root, ["stash", "pop"]);
        // pop はコンフリクト時に非 0 で終了するが、stash entry を残すのは git 自身の挙動
        // なので追加の後始末は不要。成功・失敗いずれでも worktree は変わりうるので rescan は必ず行う
        self.reload_current_view();
        self.rescan();
        self.last_rescan = Instant::now();
        self.rescan_pending = false;
        self.refresh_git_diff_selection();
        if outcome.ok {
            self.set_notice("stash を復元しました", false);
        } else {
            let message = if outcome.message.is_empty() {
                "stash pop に失敗しました (コンフリクトの可能性があります)".to_string()
            } else {
                outcome.message
            };
            self.set_notice(message, true);
        }
    }

    // GIT レーンの diff は「開いていたファイルの内容そのものが変わった」ケース (discard/stash)
    // では自動追従しない設計のままだと古い内容を映してしまう (通常の j/k 移動時に diff を
    // 追従させない設計とは理由が異なる: あちらはキーリピートで git を連打しないためで、
    // こちらはファイルが破棄され `changed_paths()` から外れた後も GitState::refresh が
    // 同じ path で再取得を試み、その結果を `file_diff` の untracked フォールバックが
    // 「新規ファイルの全行追加」として誤表示してしまうため)。rescan 後にツリー側の新しい
    // 選択へ diff を明示的に向け直す
    fn refresh_git_diff_selection(&mut self) {
        let Some(path) = self.tree.selected_or_first_file() else {
            return;
        };
        let root = self.root.clone();
        if let Lane::Git(git) = &mut self.lane {
            git.open(&root, &path);
        }
    }

    // VIEW で開いていたファイルが破棄対象に含まれる場合だけ reload する (保存時と同じ経路)。
    // GIT レーンの diff 側は refresh_git_diff_selection が別途面倒を見るのでここでは触らない
    fn reload_if_affected(&mut self, path: &Path, is_dir: bool) {
        let Some(open_path) = self.viewer.current.as_ref().map(|open| open.path.clone()) else {
            return;
        };
        let affected = if is_dir {
            open_path.starts_with(path)
        } else {
            open_path == *path
        };
        if affected {
            self.viewer.reload(&open_path);
        }
    }

    // stash は working tree 全体に影響するため、対象パスを絞らずに現在表示中のファイルを
    // 無条件で reload する (discard の reload_if_affected と違い、影響範囲を事前に特定できない)
    fn reload_current_view(&mut self) {
        if let Some(path) = self.viewer.current.as_ref().map(|open| open.path.clone()) {
            self.viewer.reload(&path);
        }
    }
}
