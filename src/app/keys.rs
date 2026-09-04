//! キールーティング。どのキーをどの順で拾うかの優先順位 (on_key) と、レーン・オーバーレイ
//! ごとのキー処理までをここに置く。実際の操作の中身 (コミット・git の書き込み・ブランチ切替・
//! GitHub タブ) は app/commit.rs / git_ops.rs / branch_ops.rs / github_keys.rs へ分けてある。

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::component::editor::EditOutcome;
use crate::component::finder::Finder;

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
        if let Mode::Help { .. } = &self.mode {
            self.on_help_key(key, ctrl);
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
        if let Mode::Grep = &self.mode {
            self.on_grep_key(key, ctrl);
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
        // Ctrl+f: ワークスペース横断検索。Ctrl+p と同じ位置 (レーン・フォーカスを問わない)
        if ctrl && key.code == KeyCode::Char('f') {
            self.pending_g = false;
            self.grep.on_open();
            self.mode = Mode::Grep;
            return;
        }
        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return;
            }
            KeyCode::Char('?') => {
                self.mode = Mode::Help { scroll: 0 };
                return;
            }
            KeyCode::Char('a') => {
                self.pending_g = false;
                self.toggle_hidden();
                return;
            }
            KeyCode::Char('i') => {
                self.pending_g = false;
                self.toggle_ignored();
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
            // それ以外の View/Git からは常に開ける
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
            // L: コミット一覧パネルの on/off。c/C/b と同じくフォーカスを問わないが、
            // レーンは VIEW 限定 (右ペインでコミット diff を出す場所が VIEW にしか無い)。
            // GIT/EDIT で押しても意味を持てないので、そこでは素通りさせて既存の挙動を変えない
            KeyCode::Char('L') if matches!(self.lane, Lane::View) => {
                self.pending_g = false;
                self.toggle_log_panel();
                return;
            }
            KeyCode::Tab => {
                // フォーカスを跨ぐと g 待ちの文脈は失われるので破棄する
                self.pending_g = false;
                self.cycle_focus();
                return;
            }
            _ => {}
        }
        // stash pop (#25) だけは GIT レーンに縛らない。z (push) は変更を全部退避すると
        // git_available が false になり GIT レーンへ再入場できなくなるため、「push した直後に
        // pop で戻れない」事故を避ける必要がある。git repo でありさえすれば (コミット一覧
        // パネルと同じ log_available 基準) どのレーンからでも呼べるようにする
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
            // ツリーのキー操作は VIEW / GIT で共通。開く先だけレーンで振り分ける
            Focus::Tree => match &self.lane {
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
            // コミット一覧ペイン (`L` で出している間だけフォーカスが来る)
            Focus::Log => self.on_log_list_key(key),
            Focus::Viewer => match &self.lane {
                Lane::Git(_) => self.on_git_key(key, ctrl),
                // 右ペインが「最後に開いたもの」で決まるので、キーの宛先も同じ判定
                // (showing_commit_diff) 1 箇所で決める
                _ if self.showing_commit_diff() => self.on_log_diff_key(key, ctrl),
                _ => self.on_viewer_key(key, ctrl),
            },
        }
    }

    /// Tab のフォーカス循環。コミット一覧を出している間だけ 3 ペインを回る
    /// (パネルが無い間の挙動は Tree ⇄ Viewer のままで 1 バイトも変わらない)
    fn cycle_focus(&mut self) {
        let has_log = self.log_panel_visible();
        self.focus = match self.focus {
            Focus::Tree if has_log => Focus::Log,
            Focus::Tree => Focus::Viewer,
            Focus::Log => Focus::Viewer,
            Focus::Viewer => Focus::Tree,
        };
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
        let outcome = state.handle_key(key, &mut self.viewer);
        // 保存で差分が生まれた/消えた分を git status へ反映させる。FS 監視のイベントでも
        // 同じことが起きるが、監視を張れない環境でも効かせるためここでも保留を立てる
        // (ファイルの増減は起きないので全走査は要らない = status_pending だけ)。
        // 再取得自体は on_tick の 500ms デバウンスに任せ、連続保存で git を連打しない
        let saved = state.take_saved();
        let saved_path = state.path.clone();
        match outcome {
            EditOutcome::Exit => self.lane = Lane::View,
            EditOutcome::Continue => {}
        }
        if saved {
            self.status_pending = true;
            // 横断検索の結果も同じ理由で古くなる (監視を張れない環境では watcher 経由の
            // 通知が来ないので、保存の経路からも印を付ける)
            self.grep.touch(&saved_path);
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
    // ヘルプは全レーン分を並べると端末の高さに収まらないので、他のペインと同じ操作感で
    // スクロールできるようにする。ページ送り量・上限は描画側が書き戻した実測 (help_view) を使う
    fn on_help_key(&mut self, key: KeyEvent, ctrl: bool) {
        let (height, total) = self.help_view;
        let max = total.saturating_sub(height);
        let half_page = (height / 2).max(1) as isize;
        if self.pending_g {
            self.pending_g = false;
            if key.code == KeyCode::Char('g') {
                self.scroll_help(-(max as isize), max);
                return;
            }
        }
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            KeyCode::Char('d') if ctrl => self.scroll_help(half_page, max),
            KeyCode::Char('u') if ctrl => self.scroll_help(-half_page, max),
            KeyCode::Char('j') | KeyCode::Down => self.scroll_help(1, max),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_help(-1, max),
            KeyCode::PageDown => self.scroll_help(height as isize, max),
            KeyCode::PageUp => self.scroll_help(-(height as isize), max),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => self.scroll_help(max as isize, max),
            _ => {}
        }
    }

    fn scroll_help(&mut self, delta: isize, max: usize) {
        if let Mode::Help { scroll } = &mut self.mode {
            *scroll = (*scroll as isize + delta).clamp(0, max as isize) as usize;
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
            1 => self.toggle_ignored(),
            2 => self.toggle_icons(),
            3 => self.toggle_wrap(),
            4 => self.cycle_theme(delta),
            5 => self.toggle_github(),
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

    fn on_grep_key(&mut self, key: KeyEvent, ctrl: bool) {
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => self.open_grep_hit(),
            KeyCode::Backspace => self.grep.backspace(),
            KeyCode::Down => self.grep.move_selection(1),
            KeyCode::Up => self.grep.move_selection(-1),
            KeyCode::Char('n') if ctrl => self.grep.move_selection(1),
            KeyCode::Char('p') if ctrl => self.grep.move_selection(-1),
            // Ctrl+u はクエリの全消去 (readline 慣習。1 文字ずつ Backspace で消させない)
            KeyCode::Char('u') if ctrl => self.grep.clear_query(),
            // ctrl 付きの印字キー (上記以外) はクエリに積まない
            KeyCode::Char(c) if !ctrl => self.grep.push_char(c),
            _ => {}
        }
    }

    // 選択中のヒットを VIEW で開き、その行へ飛ぶ。ファイル内検索 (`/`) と同じクエリを
    // 立てた状態にするので、飛んだ先で n/N がそのまま次のヒットへ効く。
    // クエリは入力中の query ではなく、表示中の行を作った result_query — デバウンス待ちの
    // 間は前の結果が残っているので、そこで Enter を押しても行とクエリが食い違わない
    fn open_grep_hit(&mut self) {
        let Some((rel, line, col)) = self.grep.selected_hit() else {
            return;
        };
        let path = self.root.join(rel);
        let query = self.grep.result_query().to_string();
        self.mode = Mode::Normal;
        // GIT レーンで open_selected を呼ぶと diff が開く。ヒットは本文の行なので VIEW へ移す
        // (enter_lane がツリーの絞り込み解除まで面倒を見る)
        if let Lane::Git(_) = &self.lane {
            self.enter_lane(0);
        }
        self.open_selected(&path);
        self.viewer.locate_search(&query, line, col);
        self.focus = Focus::Viewer;
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
                // 行単位選択中の gg は移動ではなく「選択を先頭まで伸ばす」(vim の visual mode)
                if self.viewer.line_selecting() {
                    self.viewer.move_line_selection_to(0);
                } else {
                    self.viewer.jump_to_top();
                }
                return;
            }
        }
        let half_page = (self.viewer.viewport.height / 2).max(1) as isize;
        // v で始めた行単位選択の間だけ、移動キーが選択の伸縮に化ける。マウスのドラッグで
        // 作った char 単位選択は対象外 — スクロールして全体を確かめてから y を押せるよう、
        // 移動キーは通常どおり画面だけを動かす
        if self.viewer.line_selecting() && self.extend_selection_key(key, ctrl, half_page) {
            return;
        }
        match key.code {
            // 移動はカーソルを動かし、画面はそれに追従させる (GIT の diff ペインと同じ)
            KeyCode::Char('d') if ctrl => self.viewer.move_cursor(half_page),
            KeyCode::Char('u') if ctrl => self.viewer.move_cursor(-half_page),
            // Ctrl+o: 履歴を戻る。Backspace は同じ操作の代替キー
            KeyCode::Char('o') if ctrl => self.viewer.back(),
            KeyCode::Backspace => self.viewer.back(),
            // Ctrl+i: 履歴を進む。多くの端末では Ctrl+i が Tab (0x09) と同一バイトで届き
            // KeyCode::Tab として解釈されるため、この分岐が発火しない環境がある。
            // Tab はフォーカス切り替えに使っているため奪えず、この制約は許容する
            KeyCode::Char('i') if ctrl => self.viewer.forward(),
            KeyCode::Char('j') | KeyCode::Down => self.viewer.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.viewer.move_cursor(-1),
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
            // 範囲選択とコピー。マウスを持たない (ssh 越し・トラックパッドが遠い) 使い方でも
            // 同じことができるよう、v の行単位選択をキーボード側の入口として用意する
            KeyCode::Char('v') if self.viewer.is_text() => self.viewer.toggle_line_selection(),
            KeyCode::Char('y') => self.copy_selection(),
            KeyCode::Char('Y') => self.copy_open_file(),
            KeyCode::Esc => self.viewer.clear_selection(),
            _ => {}
        }
    }

    // 行単位選択中の移動キー。消費したら true を返し、それ以外は通常処理へ流す
    // (選択は残したままスクロールできる)
    fn extend_selection_key(&mut self, key: KeyEvent, ctrl: bool, half_page: isize) -> bool {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.viewer.extend_line_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.viewer.extend_line_selection(-1),
            KeyCode::Char('d') if ctrl => self.viewer.extend_line_selection(half_page),
            KeyCode::Char('u') if ctrl => self.viewer.extend_line_selection(-half_page),
            KeyCode::Char('G') => {
                let last = self.viewer.line_count().saturating_sub(1);
                self.viewer.move_line_selection_to(last);
            }
            _ => return false,
        }
        true
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
        // Space: カーソル行が属する hunk を index へ適用/取り消し (hunk 単位ステージ)。
        // Enter: カーソル行 (V の選択中はその範囲) だけを適用/取り消し (行単位ステージ)。
        // どちらも git の実行と rescan を伴うので A/t と同じく Lane::Git の可変借用より前で
        // 拾う。ツリー側 (Focus::Tree) の Space がファイル単位のトグルなのと対になっていて、
        // 粒度だけがフォーカス・キーで変わる
        if key.code == KeyCode::Char(' ') {
            self.stage_current_hunk();
            return;
        }
        if key.code == KeyCode::Enter {
            self.stage_current_lines();
            return;
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
        // notice には &mut self が要るので、Lane の借用を離してから出す
        let mut unsupported_line_selection = false;
        let half_page = (git.viewport.height / 2).max(1) as isize;
        match key.code {
            // 移動はカーソルを動かし、画面はそれに追従させる (選択中は範囲がそのまま伸縮する)
            KeyCode::Char('d') if ctrl => git.move_cursor(half_page),
            KeyCode::Char('u') if ctrl => git.move_cursor(-half_page),
            KeyCode::Char('j') | KeyCode::Down => git.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => git.move_cursor(-1),
            // V: 行単位選択の開始/解除 (vim の visual line 相当)。`v` は side-by-side に
            // 割り当て済みなので大文字を使う。効かない表示では notice で理由を出す
            // (無言の no-op だと「効かないキー」なのか「押し損ねた」のか分からない)
            KeyCode::Char('V') if git.line_selection_available() => git.toggle_line_selection(),
            KeyCode::Char('V') => unsupported_line_selection = true,
            KeyCode::Esc => git.clear_line_selection(),
            // diff は VIEW とは別ドキュメントなので折返しも独立させる (config には保存しない)
            KeyCode::Char('w') => git.toggle_wrap(),
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
        if unsupported_line_selection {
            self.set_notice(
                "この表示では行単位選択を使えません (A の解除 / v で inline に戻してください)",
                true,
            );
        }
    }

    // コミット一覧ペイン。j/k は移動のみで diff は開かない
    // (GIT のツリーと同じ理由でキーリピート時に git show を連打しないため)
    fn on_log_list_key(&mut self, key: KeyEvent) {
        if self.pending_g {
            self.pending_g = false;
            if key.code == KeyCode::Char('g') {
                if let Some(log) = &mut self.log {
                    log.select_top();
                }
                return;
            }
        }
        // Esc はパネルごと閉じる (notice も borrow も要らないので Lane 側より先に処理する)
        if key.code == KeyCode::Esc {
            self.close_log_panel();
            return;
        }
        let root = self.root.clone();
        let Some(log) = &mut self.log else {
            return;
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => log.move_selection(&root, 1),
            KeyCode::Char('k') | KeyCode::Up => log.move_selection(&root, -1),
            // 開いたら読む先は右ペインなので、そのままフォーカスも移す (Enter のたびに
            // Tab を押させない)。パネルを出す時の focus 移動と同じ考え方
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                log.open_selected(&root);
                self.focus = Focus::Viewer;
            }
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => log.select_bottom(&root),
            _ => {}
        }
    }

    // 右ペインにコミット diff を出している間のキー。GIT の diff ペインと同じ操作感だが
    // 基準の切替 (t) は無い (コミットの diff は HEAD/staged のような基準を持たない)
    fn on_log_diff_key(&mut self, key: KeyEvent, ctrl: bool) {
        if self.pending_g {
            self.pending_g = false;
            if key.code == KeyCode::Char('g') {
                if let Some(log) = &mut self.log {
                    log.jump_to_top();
                }
                return;
            }
        }
        // Esc は diff だけ畳んでファイル表示へ戻す (パネルは開いたまま)。
        // 一覧側の Esc がパネルごと閉じるのと対になっていて、深い方から 1 段ずつ戻る
        if key.code == KeyCode::Esc {
            if let Some(log) = &mut self.log {
                log.close_diff();
            }
            self.focus = Focus::Log;
            return;
        }
        let Some(log) = &mut self.log else {
            return;
        };
        let half_page = (log.viewport.height / 2).max(1) as isize;
        match key.code {
            // 移動はカーソルを動かし、画面はそれに追従させる (GIT/VIEW と同じ)
            KeyCode::Char('d') if ctrl => log.move_cursor(half_page),
            KeyCode::Char('u') if ctrl => log.move_cursor(-half_page),
            KeyCode::Char('j') | KeyCode::Down => log.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => log.move_cursor(-1),
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
