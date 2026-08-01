use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::branch::BranchState;
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

    // issues タブ (#33) のグローバルキー。フォーカスに依らない操作 (o/r/t/フィルタ開始) を先に拾い、
    // 残りは on_tree_key/on_viewer_key と同じ「フォーカスで振り分け」に揃える
    fn on_issues_key(&mut self, key: KeyEvent, ctrl: bool) {
        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return;
            }
            KeyCode::Char('?') => {
                self.mode = Mode::Help;
                return;
            }
            KeyCode::Char('s') => {
                self.mode = Mode::Settings(SettingsState::default());
                return;
            }
            KeyCode::Tab => {
                self.pending_g = false;
                self.focus = match self.focus {
                    Focus::Tree => Focus::Viewer,
                    Focus::Viewer => Focus::Tree,
                };
                return;
            }
            KeyCode::Char('o') => {
                self.open_issue_web();
                return;
            }
            KeyCode::Char('r') => {
                self.refresh_issues();
                return;
            }
            KeyCode::Char('t') => {
                self.issues.cycle_state_filter();
                return;
            }
            // 絞り込みは一覧側にフォーカスがある時だけ (詳細ペインでの / は将来 diff 内検索等に
            // 予約したいので、ここでは今の要求 (一覧のみ) 通りに絞る)
            KeyCode::Char('/') if self.focus == Focus::Tree => {
                self.issues.begin_filter_edit();
                self.mode = Mode::Input {
                    kind: InputKind::Filter,
                    buffer: self.issues.query.clone(),
                };
                return;
            }
            _ => {}
        }
        match self.focus {
            Focus::Tree => self.on_issues_list_key(key, ctrl),
            Focus::Viewer => self.on_issues_detail_key(key, ctrl),
        }
    }

    fn on_issues_list_key(&mut self, key: KeyEvent, ctrl: bool) {
        if self.pending_g {
            self.pending_g = false;
            if key.code == KeyCode::Char('g') {
                self.issues.select_top();
                return;
            }
        }
        let half_page = (self.issues.list_area_height / 2).max(1) as isize;
        match key.code {
            KeyCode::Char('d') if ctrl => self.issues.move_selection(half_page),
            KeyCode::Char('u') if ctrl => self.issues.move_selection(-half_page),
            KeyCode::Char('j') | KeyCode::Down => self.issues.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.issues.move_selection(-1),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => self.issues.select_bottom(),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => self.open_selected_issue(),
            _ => {}
        }
    }

    // 詳細ペインのスクロール。GIT/LOG の diff ペインと同じ操作感だが、折返しは常時 ON 固定
    // (issuesview::IssuesState 参照) なので w/h/l/0 は割り当てない
    fn on_issues_detail_key(&mut self, key: KeyEvent, ctrl: bool) {
        if self.pending_g {
            self.pending_g = false;
            if key.code == KeyCode::Char('g') {
                self.issues.jump_to_top();
                return;
            }
        }
        let half_page = (self.issues.viewport.height / 2).max(1) as isize;
        match key.code {
            KeyCode::Char('d') if ctrl => self.issues.scroll_by(half_page),
            KeyCode::Char('u') if ctrl => self.issues.scroll_by(-half_page),
            KeyCode::Char('j') | KeyCode::Down => self.issues.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.issues.scroll_by(-1),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => self.issues.jump_to_bottom(),
            _ => {}
        }
    }

    /// r / 初回タブ表示: issues 一覧を取得する。実行中の取得があれば二重起動しない
    /// (App::ensure_issues_loaded と同じガードをここでも通す)
    pub(super) fn refresh_issues(&mut self) {
        if self.issues.list_loading() {
            return;
        }
        let root = self.root.clone();
        let rx = crate::job::spawn(move || crate::github::list_issues(&root));
        self.issues.begin_list_fetch(rx);
    }

    /// Enter/l/クリック共通の詳細オープン。本文は request_open が rows から即座に組み立てる
    /// ので、ここで起動するのはコメント取得だけ (キャッシュ済み・取得中なら job を起動しない。
    /// IssuesState::request_open が判定する)
    pub(super) fn open_selected_issue(&mut self) {
        let Some(number) = self.issues.selected_number() else {
            return;
        };
        if !self.issues.request_open(number) {
            return;
        }
        let root = self.root.clone();
        // Line 化 (build_detail_lines) はここ (バックグラウンドスレッド) で済ませておく。
        // DetailSlot<Vec<Line>> は組み立て済みデータを持つ想定で、詳細キャッシュのスレッド
        // 構成を増やさないため (issuesview::IssuesState::begin_comments_fetch 参照)
        let rx = crate::job::spawn(move || {
            let result = crate::github::issue_comments(&root, number)
                .map(|raw| crate::issuesview::build_detail_lines(&raw));
            (number, result)
        });
        self.issues.begin_comments_fetch(rx);
    }

    /// o: ブラウザで開く。多重起動防止のみ行い、成功時は notice を出さない (ブラウザが
    /// 実際に開いたかどうかは OS 側の話で、fv 側からは gh の exit code しか分からないため)
    fn open_issue_web(&mut self) {
        let Some(number) = self.issues.selected_number() else {
            return;
        };
        if self.issues.open_web_in_flight() {
            return;
        }
        let root = self.root.clone();
        let rx = crate::job::spawn(move || crate::github::open_issue_web(&root, number));
        self.issues.begin_open_web(rx);
    }

    // pull requests タブ (#34) のグローバルキー。issues (#33) と同じ形 (フォーカスに依らない
    // 操作を先に拾い、残りはフォーカスで振り分け) に、右ペインの表示切替 (d/S) が追加で入る
    fn on_pr_key(&mut self, key: KeyEvent, ctrl: bool) {
        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return;
            }
            KeyCode::Char('?') => {
                self.mode = Mode::Help;
                return;
            }
            KeyCode::Char('s') => {
                self.mode = Mode::Settings(SettingsState::default());
                return;
            }
            KeyCode::Tab => {
                self.pending_g = false;
                self.focus = match self.focus {
                    Focus::Tree => Focus::Viewer,
                    Focus::Viewer => Focus::Tree,
                };
                return;
            }
            KeyCode::Char('o') => {
                self.open_pr_web();
                return;
            }
            KeyCode::Char('r') => {
                self.refresh_prs();
                return;
            }
            KeyCode::Char('t') => {
                self.prs.cycle_state_filter();
                return;
            }
            KeyCode::Char('/') if self.focus == Focus::Tree => {
                self.prs.begin_filter_edit();
                self.mode = Mode::Input {
                    kind: InputKind::Filter,
                    buffer: self.prs.query.clone(),
                };
                return;
            }
            // d/S は開いている (または選択中の) PR の表示だけを切り替える。Ctrl+d は
            // 半ページスクロールなので !ctrl で明示的に除外する
            KeyCode::Char('d') if !ctrl => {
                self.switch_pr_view(crate::prsview::DetailView::Diff);
                return;
            }
            KeyCode::Char('S') => {
                self.switch_pr_view(crate::prsview::DetailView::Checks);
                return;
            }
            _ => {}
        }
        match self.focus {
            Focus::Tree => self.on_pr_list_key(key, ctrl),
            Focus::Viewer => self.on_pr_detail_key(key, ctrl),
        }
    }

    fn on_pr_list_key(&mut self, key: KeyEvent, ctrl: bool) {
        if self.pending_g {
            self.pending_g = false;
            if key.code == KeyCode::Char('g') {
                self.prs.select_top();
                return;
            }
        }
        let half_page = (self.prs.list_area_height / 2).max(1) as isize;
        match key.code {
            KeyCode::Char('d') if ctrl => self.prs.move_selection(half_page),
            KeyCode::Char('u') if ctrl => self.prs.move_selection(-half_page),
            KeyCode::Char('j') | KeyCode::Down => self.prs.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.prs.move_selection(-1),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => self.prs.select_bottom(),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => self.open_selected_pr(),
            _ => {}
        }
    }

    // 右ペイン (説明/diff/CI) のスクロール。diff 表示中だけ GIT/LOG と同じ wrap・hscroll・
    // hunk ジャンプが効く (説明/CI は issues の詳細と同じくプロースなので wrap 固定)
    fn on_pr_detail_key(&mut self, key: KeyEvent, ctrl: bool) {
        if self.pending_g {
            self.pending_g = false;
            if key.code == KeyCode::Char('g') {
                self.prs.jump_to_top();
                return;
            }
        }
        let half_page = (self.prs.current_viewport().height / 2).max(1) as isize;
        match key.code {
            KeyCode::Char('d') if ctrl => self.prs.scroll_by(half_page),
            KeyCode::Char('u') if ctrl => self.prs.scroll_by(-half_page),
            KeyCode::Char('j') | KeyCode::Down => self.prs.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.prs.scroll_by(-1),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => self.prs.jump_to_bottom(),
            KeyCode::Char('w') => self.prs.toggle_diff_wrap(),
            KeyCode::Char('h') | KeyCode::Left => self.prs.hscroll_by(-6),
            KeyCode::Char('l') | KeyCode::Right => self.prs.hscroll_by(6),
            KeyCode::Char('0') => self.prs.hscroll_reset(),
            KeyCode::Char(']') => self.prs.next_hunk(),
            KeyCode::Char('[') => self.prs.prev_hunk(),
            _ => {}
        }
    }

    /// r / 初回タブ表示: PR 一覧を取得する。実行中の取得があれば二重起動しない
    pub(super) fn refresh_prs(&mut self) {
        if self.prs.list_loading() {
            return;
        }
        let root = self.root.clone();
        let rx = crate::job::spawn(move || crate::github::list_prs(&root));
        self.prs.begin_list_fetch(rx);
    }

    /// Enter/l/クリック共通: 選択中 PR を説明表示で開く (既に別の PR/表示を開いていても
    /// Description へ揃える。新しい対象を選ぶ操作なので既定表示に戻すのが自然)。
    /// diff/CI の先読み (`d`/`S` を押した時の待ち時間を無くす) はこの明示操作だけを起点にする
    /// — j/k の選択移動では note_opened を呼ばないため、キーリピートで gh を連打しない
    pub(super) fn open_selected_pr(&mut self) {
        let Some(number) = self.prs.selected_number() else {
            return;
        };
        self.prs
            .set_open(number, crate::prsview::DetailView::Description);
        self.prs.note_opened(number);
        self.dispatch_pr_fetch();
    }

    /// d/S: 表示だけを切り替える。まだ何も開いていなければ選択中の行を対象にする
    /// (Enter を経由しなくても d/S だけで読み始められるようにするため)
    fn switch_pr_view(&mut self, view: crate::prsview::DetailView) {
        let Some(number) = self
            .prs
            .open_number()
            .or_else(|| self.prs.selected_number())
        else {
            return;
        };
        self.prs.set_open(number, view);
        self.dispatch_pr_fetch();
        // 先読みで diff が既にキャッシュ済みだと dispatch_pr_fetch はジョブを起動しない
        // (=poll での通知が発火しない) ため、表示に切り替えた瞬間にここで打ち切りを知らせる
        if let Some((message, is_error)) = self.prs.truncation_notice_for_current() {
            self.set_notice(message, is_error);
        }
    }

    /// on_tick から毎 tick 呼ぶ。開いている PR の diff/CI を静かに 1 段階だけ先読みする。
    /// advance_prefetch が None を返す間 (タイマー未到達・既に先読み済み等) は何もしない
    pub(super) fn dispatch_pr_prefetch(&mut self) {
        let Some((number, view)) = self.prs.advance_prefetch() else {
            return;
        };
        let root = self.root.clone();
        match view {
            crate::prsview::DetailView::Diff => {
                let rx = crate::job::spawn(move || crate::prsview::fetch_diff(&root, number));
                self.prs.begin_diff_fetch(rx);
            }
            crate::prsview::DetailView::Checks => {
                let rx = crate::job::spawn(move || crate::prsview::fetch_checks(&root, number));
                self.prs.begin_checks_fetch(rx);
            }
            // 先読みは diff/CI だけが対象 (Description は本文がネットワーク不要、
            // コメントは開いた瞬間に dispatch_pr_fetch が既に取りに行っている)
            crate::prsview::DetailView::Description => {}
        }
    }

    // 現在の (open_number, view) が未キャッシュ・未取得中なら対応する gh コマンドの job を
    // 起動する。取得済み・取得中は PrsState::request_current が None を返すので何もしない
    // (Enter/d/S の連打で二重起動しない)
    fn dispatch_pr_fetch(&mut self) {
        let Some((number, view)) = self.prs.request_current() else {
            return;
        };
        let root = self.root.clone();
        match view {
            crate::prsview::DetailView::Description => {
                let rx = crate::job::spawn(move || crate::prsview::fetch_comments(&root, number));
                self.prs.begin_comments_fetch(rx);
            }
            crate::prsview::DetailView::Diff => {
                let rx = crate::job::spawn(move || crate::prsview::fetch_diff(&root, number));
                self.prs.begin_diff_fetch(rx);
            }
            crate::prsview::DetailView::Checks => {
                let rx = crate::job::spawn(move || crate::prsview::fetch_checks(&root, number));
                self.prs.begin_checks_fetch(rx);
            }
        }
    }

    /// o: ブラウザで開く
    fn open_pr_web(&mut self) {
        let Some(number) = self.prs.selected_number() else {
            return;
        };
        if self.prs.open_web_in_flight() {
            return;
        }
        let root = self.root.clone();
        let rx = crate::job::spawn(move || crate::github::open_pr_web(&root, number));
        self.prs.begin_open_web(rx);
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

    /// c / C: コミットオーバーレイを開く。GIT レーンに限定しない (変更を見て回ってから
    /// そのままコミットしたい時に、わざわざ Shift+Tab で GIT へ切り替えさせたくないため)。
    /// 使えない文脈 (repo 外・staged が空) は開かず notice で理由を出す
    fn open_commit(&mut self, amend: bool) {
        if self.git.is_none() {
            return;
        }
        // 型上ここへは実際には来ない (Lane::Edit は印字キーを全て文字入力にするため 'c' は
        // ここまで届かない) が、issue の要求通り明示的にガードしておく
        if let Lane::Edit(state) = &self.lane
            && state.buffer.dirty()
        {
            self.set_notice(
                "未保存の変更があります。保存してからコミットしてください".to_string(),
                true,
            );
            return;
        }
        if amend {
            let buffer = self
                .amend_draft
                .take()
                .or_else(|| git::last_commit_message(&self.root))
                .unwrap_or_default();
            let cursor = buffer.chars().count();
            self.mode = Mode::Commit {
                buffer,
                cursor,
                amend: true,
                error: None,
            };
            return;
        }
        if !self.has_staged_changes() {
            self.set_notice(
                "ステージされた変更がありません (Space でステージ)".to_string(),
                true,
            );
            return;
        }
        let buffer = self.commit_draft.take().unwrap_or_default();
        let cursor = buffer.chars().count();
        self.mode = Mode::Commit {
            buffer,
            cursor,
            amend: false,
            error: None,
        };
    }

    // amend は staged が空でも許可する (issue の要求: メッセージ修正の用途) が、通常コミットは
    // index 側に何か 1 つでも乗っていないと開かせない (--allow-empty は使わない方針のため)
    fn has_staged_changes(&self) -> bool {
        self.git
            .as_ref()
            .is_some_and(|status| status.files.values().any(|f| f.index.is_some()))
    }

    fn on_commit_key(&mut self, key: KeyEvent, ctrl: bool) {
        match key.code {
            KeyCode::Esc => self.close_commit(),
            KeyCode::Char('s') if ctrl => self.submit_commit(),
            KeyCode::Enter => self.commit_insert('\n'),
            KeyCode::Backspace => self.commit_backspace(),
            KeyCode::Left => self.commit_move_char(-1),
            KeyCode::Right => self.commit_move_char(1),
            KeyCode::Up => self.commit_move_line(-1),
            KeyCode::Down => self.commit_move_line(1),
            KeyCode::Home => self.commit_move_home(),
            KeyCode::End => self.commit_move_end(),
            KeyCode::Char(c) if !ctrl => self.commit_insert(c),
            _ => {}
        }
    }

    // Esc は内容を破棄しない。amend/通常で保存先を分けるのは、次に C を押した時に
    // 前回の amend 編集を (git log の再フェッチではなく) そのまま復元するため
    fn close_commit(&mut self) {
        let Mode::Commit { buffer, amend, .. } = std::mem::replace(&mut self.mode, Mode::Normal)
        else {
            return;
        };
        if amend {
            self.amend_draft = Some(buffer);
        } else {
            self.commit_draft = Some(buffer);
        }
    }

    fn commit_insert(&mut self, c: char) {
        let Mode::Commit { buffer, cursor, .. } = &mut self.mode else {
            return;
        };
        let idx = char_byte_index(buffer, *cursor);
        buffer.insert(idx, c);
        *cursor += 1;
    }

    fn commit_backspace(&mut self) {
        let Mode::Commit { buffer, cursor, .. } = &mut self.mode else {
            return;
        };
        if *cursor == 0 {
            return;
        }
        let idx = char_byte_index(buffer, *cursor - 1);
        buffer.remove(idx);
        *cursor -= 1;
    }

    fn commit_move_char(&mut self, delta: isize) {
        let Mode::Commit { buffer, cursor, .. } = &mut self.mode else {
            return;
        };
        let len = buffer.chars().count() as isize;
        *cursor = (*cursor as isize + delta).clamp(0, len) as usize;
    }

    // Up/Down は同じ桁位置を保とうとし、行が短ければ行末に丸める (一般的なエディタと同じ挙動)
    fn commit_move_line(&mut self, delta: isize) {
        let Mode::Commit { buffer, cursor, .. } = &mut self.mode else {
            return;
        };
        let lines: Vec<&str> = buffer.split('\n').collect();
        let (line, col) = line_col(buffer, *cursor);
        let target_line = (line as isize + delta).clamp(0, lines.len() as isize - 1) as usize;
        let target_col = col.min(lines[target_line].chars().count());
        *cursor = char_index_of(&lines, target_line, target_col);
    }

    fn commit_move_home(&mut self) {
        let Mode::Commit { buffer, cursor, .. } = &mut self.mode else {
            return;
        };
        let lines: Vec<&str> = buffer.split('\n').collect();
        let (line, _) = line_col(buffer, *cursor);
        *cursor = char_index_of(&lines, line, 0);
    }

    fn commit_move_end(&mut self) {
        let Mode::Commit { buffer, cursor, .. } = &mut self.mode else {
            return;
        };
        let lines: Vec<&str> = buffer.split('\n').collect();
        let (line, _) = line_col(buffer, *cursor);
        let end = lines[line].chars().count();
        *cursor = char_index_of(&lines, line, end);
    }

    // bracketed paste (App::on_paste から呼ぶ)。改行以外の制御文字は落とす (Input/Finder と同じ方針)
    pub(super) fn commit_paste(&mut self, text: &str) {
        let Mode::Commit { buffer, cursor, .. } = &mut self.mode else {
            return;
        };
        let idx = char_byte_index(buffer, *cursor);
        let filtered: String = text
            .chars()
            .filter(|c| !c.is_control() || *c == '\n')
            .collect();
        let inserted = filtered.chars().count();
        buffer.insert_str(idx, &filtered);
        *cursor += inserted;
    }

    // Ctrl+s: amend は履歴を書き換える (push 済みの可能性がある) ので確認オーバーレイを経由させ、
    // 通常コミットは直接実行する
    fn submit_commit(&mut self) {
        let Mode::Commit { buffer, amend, .. } = &self.mode else {
            return;
        };
        let message = buffer.clone();
        let amend = *amend;
        if amend {
            // 確認をキャンセルしても下書きが残るよう、実行前に退避しておく
            self.amend_draft = Some(message.clone());
            let subject = message.lines().next().unwrap_or("").to_string();
            self.mode = Mode::Confirm {
                prompt: format!("amend commit: 「{subject}」"),
                action: ConfirmAction::Amend { message },
            };
            return;
        }
        self.perform_commit(&message, false);
    }

    // git::commit の実行と結果反映。amend は確認オーバーレイ (run_confirm_action) から、
    // 通常コミットは Mode::Commit のまま (submit_commit) から呼ばれる
    fn perform_commit(&mut self, message: &str, amend: bool) {
        let outcome = git::commit(&self.root, message, amend);
        if outcome.ok {
            self.mode = Mode::Normal;
            if amend {
                self.amend_draft = None;
            } else {
                self.commit_draft = None;
            }
            self.set_notice(outcome.message, false);
            // stage/unstage と同じ入口に相乗りさせる (ツリーの変更ファイル一覧・diff を揃える)
            self.rescan();
            self.last_rescan = Instant::now();
            self.rescan_pending = false;
        } else if let Mode::Commit { error, .. } = &mut self.mode {
            // 通常コミット (Mode::Commit のまま) の失敗はオーバーレイ内にエラーを出し、
            // 書きかけのメッセージを保ったまま再試行できるようにする
            *error = Some(outcome.message);
        } else {
            // amend は確認オーバーレイ経由で mode が既に Normal に戻っているため、
            // 下書き (amend_draft) はここまでの経路で保持済み。notice で失敗を出す
            self.set_notice(outcome.message, true);
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
        // 絶対パスは確認枠をはみ出して肝心のファイル名が読めなくなるので repo 相対で出す
        // (GIT ペインのタイトルと同じ扱い)
        let shown = path.strip_prefix(&self.root).unwrap_or(&path);
        let mut prompt = format!("{count} 件の変更を破棄しますか？\n{}", shown.display());
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

    /// b: ブランチ一覧オーバーレイを開く。使えない文脈 (非 git repo) は開かず no-op
    fn open_branch(&mut self) {
        if !self.branch_available() {
            return;
        }
        // 型上ここへは実際には来ない (Lane::Edit は印字キーを全て文字入力にするため 'b' は
        // ここまで届かない) が、open_commit と同じく issue の要求通り明示的にガードしておく
        if let Lane::Edit(state) = &self.lane
            && state.buffer.dirty()
        {
            self.set_notice(
                "未保存の変更があります。保存してから切り替えてください".to_string(),
                true,
            );
            return;
        }
        let current = self
            .branch_status
            .as_ref()
            .filter(|s| !s.detached)
            .map(|s| s.name.as_str());
        self.mode = Mode::Branch(BranchState::new(&self.root, current));
    }

    fn on_branch_key(&mut self, key: KeyEvent, ctrl: bool) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                return;
            }
            KeyCode::Enter => {
                self.checkout_selected_branch();
                return;
            }
            KeyCode::Char('n') if ctrl => {
                self.create_new_branch();
                return;
            }
            _ => {}
        }
        let Mode::Branch(state) = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Backspace => state.backspace(),
            KeyCode::Down => state.move_selection(1),
            KeyCode::Up => state.move_selection(-1),
            KeyCode::Char('p') if ctrl => state.move_selection(-1),
            // ctrl 付きの印字キー (Ctrl+n/p 以外) はクエリに積まない (Finder と同じ方針)
            KeyCode::Char(c) if !ctrl => state.push_char(c),
            _ => {}
        }
    }

    // Enter: 選択中のブランチへ切り替える。remote 由来なら `switch --track` 相当で
    // ローカル追跡ブランチを作りつつ切り替える
    fn checkout_selected_branch(&mut self) {
        let Mode::Branch(state) = &self.mode else {
            return;
        };
        let Some(row) = state.selected_row() else {
            return;
        };
        let target = row.entry.name.clone();
        let remote = row.entry.remote;
        let outcome = if remote {
            git::switch_track_branch(&self.root, &target)
        } else {
            git::switch_branch(&self.root, &target)
        };
        self.finish_branch_action(outcome);
    }

    // Ctrl+n: 入力文字列が既存のローカルブランチと一致しない間だけ新規作成する。
    // 一致する間は誤って上書きしないよう何もせず notice で理由を示し、オーバーレイは開いたままにする
    fn create_new_branch(&mut self) {
        let Mode::Branch(state) = &self.mode else {
            return;
        };
        if state.query.is_empty() {
            return;
        }
        if state.matches_existing_local() {
            let name = state.query.clone();
            self.set_notice(
                format!("ブランチ「{name}」は既に存在します (Enter で切替)"),
                true,
            );
            return;
        }
        let name = state.query.clone();
        let outcome = git::create_branch(&self.root, &name);
        self.finish_branch_action(outcome);
    }

    // checkout/create 共通の後処理。未保存バッファの拒否もここに寄せず呼び出し元で先に弾く
    // (dirty チェックは open_branch 側にあり、型上ここへは既に届かない状態でしか呼ばれない)
    fn finish_branch_action(&mut self, outcome: git::GitOutcome) {
        self.mode = Mode::Normal;
        if !outcome.ok {
            let message = if outcome.message.is_empty() {
                "git の実行に失敗しました".to_string()
            } else {
                outcome.message
            };
            self.set_notice(message, true);
            return;
        }
        // 切替先に開いていたファイルが無ければ右ペインを空にする (issue の提案通り)
        let stale = self
            .viewer
            .current
            .as_ref()
            .is_some_and(|open| !open.path.exists());
        if stale {
            self.viewer.close();
        }
        // stage/unstage・commit と同じ入口に相乗りさせる (ツリー・git status・branch_status を揃える)
        self.rescan();
        self.last_rescan = Instant::now();
        self.rescan_pending = false;
        let branch = self
            .branch_status
            .as_ref()
            .map(|s| s.name.as_str())
            .unwrap_or("?");
        let message = if stale {
            format!("{branch} に切り替えました (開いていたファイルが見つからないため閉じました)")
        } else {
            format!("{branch} に切り替えました")
        };
        self.set_notice(message, false);
    }

    /// f: リモートの更新を取得する。fetch はローカルを変更しないので確認は不要 (issue の要求通り)
    fn start_fetch(&mut self) {
        if !self.branch_available() {
            return;
        }
        let root = self.root.clone();
        self.start_remote_job(git::RemoteJobKind::Fetch, move || git::fetch(&root));
    }

    /// p: fast-forward のみで取り込む。マージ・リベースが必要な状況は fv が引き受けず、
    /// fast-forward できないときの git のエラーをそのまま notice に出す (issue の要求通り)
    fn start_pull(&mut self) {
        if !self.branch_available() {
            return;
        }
        let root = self.root.clone();
        self.start_remote_job(git::RemoteJobKind::Pull, move || git::pull(&root));
    }

    /// P: push は確認オーバーレイを必須にする (issue の要求。fetch/pull と違いリモートの
    /// 履歴・ブランチ構成を変えるため)。未保存の EDIT バッファは拒否まではせず、
    /// prompt に警告を足すだけに留める (issue の要求通り)。既にジョブが実行中なら
    /// 確認オーバーレイ自体を開かない (開いても実行時に start_remote_job が無視するだけで
    /// ユーザーには何も起きなかったように見えてしまうため、ここで先に弾く)。
    /// dirty チェックは open_commit/open_branch と同じ理由で型上ここへは実際には来ない
    /// (Lane::Edit は印字キーを全て文字入力にするため 'P' はここまで届かない) が、
    /// issue の要求通り明示的にガードしておく (belt and suspenders)
    fn confirm_push(&mut self) {
        if !self.branch_available() || self.pending_remote_job.is_some() {
            return;
        }
        let Some(status) = &self.branch_status else {
            return;
        };
        let target = if status.has_upstream {
            format!("origin/{}", status.name)
        } else {
            format!("origin/{} (new upstream)", status.name)
        };
        let mut prompt = format!("push を実行しますか？\n{target}");
        if let Lane::Edit(state) = &self.lane
            && state.buffer.dirty()
        {
            prompt.push_str("\n(未保存の編集があります。保存を忘れずに)");
        }
        self.mode = Mode::Confirm {
            prompt,
            action: ConfirmAction::Push,
        };
    }

    fn execute_push(&mut self) {
        let Some(status) = &self.branch_status else {
            return;
        };
        let branch = status.name.clone();
        let has_upstream = status.has_upstream;
        let root = self.root.clone();
        self.start_remote_job(git::RemoteJobKind::Push, move || {
            git::push(&root, &branch, has_upstream)
        });
    }
}

// commit オーバーレイの改行込みバッファは char インデックスで扱う (バイトインデックスだと
// 日本語等の複数バイト文字でカーソル位置がずれるため)

fn char_byte_index(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

fn line_col(s: &str, char_idx: usize) -> (usize, usize) {
    let mut line = 0usize;
    let mut col = 0usize;
    for (i, ch) in s.chars().enumerate() {
        if i == char_idx {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

// lines ([&str]、buffer.split('\n') の結果) 上の (line, col) を buffer 全体の char インデックスへ戻す
fn char_index_of(lines: &[&str], line: usize, col: usize) -> usize {
    let mut idx = 0;
    for l in &lines[..line] {
        idx += l.chars().count() + 1; // +1 は行を繋いでいた '\n' の分
    }
    idx + col
}
