//! キールーティング。どのキーをどの順で拾うかの優先順位 (on_key) と、レーン・オーバーレイ
//! ごとのキー処理までをここに置く。実際の操作の中身 (コミット・git の書き込み・ブランチ切替・
//! GitHub タブ) は app/commit.rs / git_ops.rs / branch_ops.rs / github_keys.rs へ分けてある。

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::editor::EditOutcome;
use crate::finder::Finder;

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
        if let Mode::Commit { .. } = &self.mode {
            self.on_commit_key(key, ctrl);
            return;
        }
        if let Mode::Branch(_) = &self.mode {
            self.on_branch_key(key, ctrl);
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
        // Viewer 以外のタブは Lane/ツリー/ビューアの概念を持たないので、以降の共通ディスパッチには
        // 流さずここで専用のハンドラに振り分ける
        if !matches!(self.workspace, Workspace::Viewer) {
            match self.workspace {
                Workspace::Issues => self.on_issues_key(key, ctrl),
                Workspace::PullRequests => self.on_pr_key(key, ctrl),
                Workspace::Viewer => {}
            }
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
            // c/C はレーンを問わない (GIT に切り替えさせず「見て回ってからそのままコミット」を
            // 許すため)。使えない文脈 (repo 外・staged 空等) は open_commit 内で notice/no-op に倒す
            KeyCode::Char('c') => {
                self.pending_g = false;
                self.open_commit(false);
                return;
            }
            KeyCode::Char('C') => {
                self.pending_g = false;
                self.open_commit(true);
                return;
            }
            // b もレーンを問わない (issue #26: どのレーンからでも開ける独立オーバーレイ)。
            // c/C と同じく Lane::Edit は印字キーを全て文字入力にするためここまで届かないが、
            // それ以外の View/Git/Log からは常に開ける
            KeyCode::Char('b') => {
                self.pending_g = false;
                self.open_branch();
                return;
            }
            // f/p/P (#27) もレーンを問わない。使えない文脈 (非 git repo・実行中の別ジョブ) は
            // 各関数側 (branch_available / start_remote_job のガード) で no-op に倒す
            KeyCode::Char('f') => {
                self.pending_g = false;
                self.start_fetch();
                return;
            }
            KeyCode::Char('p') => {
                self.pending_g = false;
                self.start_pull();
                return;
            }
            KeyCode::Char('P') => {
                self.pending_g = false;
                self.confirm_push();
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

    // Search の確定先は Lane で振り分ける (#31: GIT レーンの diff 内検索は GitState 側に持つ)。
    // Goto は View レーンの `:` からしか届かないので lane 分岐は要らない。Filter は Workspace
    // (issues/PR タブ) 側の状態なので Lane ではなく workspace で振り分ける
    fn cancel_input(&mut self, kind: InputKind) {
        match kind {
            InputKind::Search => match &mut self.lane {
                Lane::Git(git) => git.cancel_search(),
                _ => self.viewer.cancel_search(),
            },
            // Goto は確定時にしか状態を変えないので、キャンセル時に戻すものがない
            InputKind::Goto => {}
            InputKind::Filter => match self.workspace {
                Workspace::Issues => self.issues.cancel_filter_edit(),
                Workspace::PullRequests => self.prs.cancel_filter_edit(),
                Workspace::Viewer => {}
            },
        }
    }

    fn confirm_input(&mut self, kind: InputKind) {
        match kind {
            InputKind::Search => match &mut self.lane {
                Lane::Git(git) => git.confirm_search(),
                _ => self.viewer.confirm_search(),
            },
            InputKind::Goto => {
                // buffer は数字のみ。空文字列や "0" は parse/goto_line 側で no-op になる
                if let Mode::Input { buffer, .. } = &self.mode
                    && let Ok(line_no) = buffer.parse::<usize>()
                {
                    self.viewer.goto_line(line_no);
                }
            }
            InputKind::Filter => match self.workspace {
                Workspace::Issues => self.issues.confirm_filter_edit(),
                Workspace::PullRequests => self.prs.confirm_filter_edit(),
                Workspace::Viewer => {}
            },
        }
    }

    // on_paste (mod.rs) からも呼ぶため pub(super)
    pub(super) fn live_update_input(&mut self, kind: InputKind) {
        match kind {
            InputKind::Search => {
                if let Mode::Input { buffer, .. } = &self.mode {
                    let query = buffer.clone();
                    match &mut self.lane {
                        Lane::Git(git) => git.update_search(&query),
                        _ => self.viewer.update_search(&query),
                    }
                }
            }
            // Goto はステータスバーが buffer をそのまま表示するのでライブ更新は不要
            InputKind::Goto => {}
            InputKind::Filter => {
                let Mode::Input { buffer, .. } = &self.mode else {
                    return;
                };
                let query = buffer.clone();
                match self.workspace {
                    Workspace::Issues => self.issues.set_query(query),
                    Workspace::PullRequests => self.prs.set_query(query),
                    Workspace::Viewer => {}
                }
            }
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
            ConfirmAction::Amend { message } => self.perform_commit(&message, true),
            ConfirmAction::Discard { path, is_dir } => self.execute_discard(path, is_dir),
            ConfirmAction::StashPush => self.execute_stash_push(),
            ConfirmAction::StashPop => self.execute_stash_pop(),
            ConfirmAction::Push => self.execute_push(),
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

    // 候補は root 全体を歩いた FileIndex から。走査がまだ終わっていなければ
    // 待たずにツリーの読み込み済み分で開き、完了時に on_tick が差し替える
    fn open_finder(&mut self) {
        let candidates = match self.file_index.request() {
            Some(files) => super::to_candidates(files),
            None => super::to_candidates(&self.tree.collect_file_paths()),
        };
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
                self.rescan_now();
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

    // GIT レーンの diff ペイン。hunk ジャンプは ]/[ に一本化し (#31)、n/N は検索の
    // 次候補/前候補に譲る (現状 VIEW の検索と同じキー配置)
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
        // A (まとめ diff トグル) / t (基準循環) は git diff の取り直しを伴いうるため、untracked
        // 一覧を Lane::Git の可変借用より前に集めておく (self.git と self.lane は別フィールドだが
        // メソッド呼び出し越しの借用はここで済ませないと両立しない。rescan() の root.clone() と同じ理由)
        if matches!(key.code, KeyCode::Char('A') | KeyCode::Char('t')) {
            let root = self.root.clone();
            let untracked = self.untracked_paths();
            let mut truncated = false;
            if let Lane::Git(git) = &mut self.lane {
                truncated = match key.code {
                    KeyCode::Char('A') => git.toggle_all(&root, &untracked),
                    KeyCode::Char('t') => git.cycle_base(&root, &untracked),
                    _ => unreachable!(),
                };
            }
            // 打ち切りは明示操作 (A/t) の直後だけ notice で知らせる。rescan 経由の背景更新は
            // 500ms デバウンス毎にスパムしないよう黙って再取得するだけにしてある (GitState::refresh)
            if truncated {
                self.set_notice(
                    "diff が大きいため表示を打ち切りました (20000 行 / 2MB)",
                    true,
                );
            }
            return;
        }
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
            // hunk ジャンプは ]/[ に一本化 (#31)
            KeyCode::Char(']') => git.next_hunk(),
            KeyCode::Char('[') => git.prev_hunk(),
            // 未確定 (Enter していない) 状態では no-op。next_match/prev_match が保証する
            KeyCode::Char('n') => git.next_match(),
            KeyCode::Char('N') => git.prev_match(),
            // まとめ diff 中のファイル境界ジャンプ。単一ファイル表示中は boundaries が空なので no-op
            KeyCode::Char('}') => git.next_file(),
            KeyCode::Char('{') => git.prev_file(),
            // side-by-side 中は左右が独立ドキュメントで一意な行位置を持たないため検索を出さない
            KeyCode::Char('/') if !git.side_by_side_active() => {
                self.mode = Mode::Input {
                    kind: InputKind::Search,
                    buffer: String::new(),
                };
            }
            // inline ⇔ side-by-side (#30)。w と同じく config には保存しない
            KeyCode::Char('v') => git.toggle_side_by_side(),
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
}
