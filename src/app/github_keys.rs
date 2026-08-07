//! Workspace::Issues / PullRequests のキー処理と、gh を叩くジョブの起動。
//! Viewer タブ (Lane/ツリー/オーバーレイ) とは文脈を共有しないので、キールーティングも
//! on_key から丸ごとここへ分岐する (app/keys.rs の Workspace 判定を参照)。
//! 一覧・詳細それぞれのスクロールは component/issues/mod.rs / component/prs/mod.rs の状態側に委ねる。

use crossterm::event::{KeyCode, KeyEvent};

use super::{App, Focus, InputKind, Mode, SettingsState};

impl App {
    // issues タブ (#33) のグローバルキー。フォーカスに依らない操作 (o/r/t/フィルタ開始) を先に拾い、
    // 残りは on_tree_key/on_viewer_key と同じ「フォーカスで振り分け」に揃える
    pub(super) fn on_issues_key(&mut self, key: KeyEvent, ctrl: bool) {
        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return;
            }
            KeyCode::Char('?') => {
                self.mode = Mode::Help { scroll: 0 };
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
    // (issues::IssuesState 参照) なので w/h/l/0 は割り当てない
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
        // 構成を増やさないため (issues::IssuesState::begin_comments_fetch 参照)
        let rx = crate::job::spawn(move || {
            let result = crate::github::issue_comments(&root, number)
                .map(|raw| crate::component::issues::build_detail_lines(&raw));
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
    pub(super) fn on_pr_key(&mut self, key: KeyEvent, ctrl: bool) {
        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return;
            }
            KeyCode::Char('?') => {
                self.mode = Mode::Help { scroll: 0 };
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
                self.switch_pr_view(crate::component::prs::DetailView::Diff);
                return;
            }
            KeyCode::Char('S') => {
                self.switch_pr_view(crate::component::prs::DetailView::Checks);
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
            .set_open(number, crate::component::prs::DetailView::Description);
        self.prs.note_opened(number);
        self.dispatch_pr_fetch();
    }

    /// d/S: 表示だけを切り替える。まだ何も開いていなければ選択中の行を対象にする
    /// (Enter を経由しなくても d/S だけで読み始められるようにするため)
    fn switch_pr_view(&mut self, view: crate::component::prs::DetailView) {
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
            crate::component::prs::DetailView::Diff => {
                let rx =
                    crate::job::spawn(move || crate::component::prs::fetch_diff(&root, number));
                self.prs.begin_diff_fetch(rx);
            }
            crate::component::prs::DetailView::Checks => {
                let rx =
                    crate::job::spawn(move || crate::component::prs::fetch_checks(&root, number));
                self.prs.begin_checks_fetch(rx);
            }
            // 先読みは diff/CI だけが対象 (Description は本文がネットワーク不要、
            // コメントは開いた瞬間に dispatch_pr_fetch が既に取りに行っている)
            crate::component::prs::DetailView::Description => {}
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
            crate::component::prs::DetailView::Description => {
                let rx =
                    crate::job::spawn(move || crate::component::prs::fetch_comments(&root, number));
                self.prs.begin_comments_fetch(rx);
            }
            crate::component::prs::DetailView::Diff => {
                let rx =
                    crate::job::spawn(move || crate::component::prs::fetch_diff(&root, number));
                self.prs.begin_diff_fetch(rx);
            }
            crate::component::prs::DetailView::Checks => {
                let rx =
                    crate::job::spawn(move || crate::component::prs::fetch_checks(&root, number));
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
}
