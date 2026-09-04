//! GitHub issues タブ (#33) の状態。左ペインは一覧 (フィルタ + キャッシュ)、右ペインは
//! 選択 issue の詳細で、VIEW/EDIT/GIT/LOG のいずれとも独立した Viewport を持つ
//! (別ドキュメントなので位置を共有する意味がなく、Viewer タブへ戻った時の読み位置も壊さない)。
//! 一覧・コメント取得・ブラウザで開く、の 3 操作はすべて job.rs (#27 の非同期基盤) に乗せ、
//! イベントループをブロックしない。フィルタは component/finder/mod.rs の fuzzy_match を再利用し、
//! 新しいマッチャは書かない (component/branch/mod.rs::BranchState と同じ方針)。
//!
//! #34 (pull requests タブ) が一覧まわりをそのまま再利用できるよう、フィルタ・スコアリング
//! (`remotelist::filter_rows`) と詳細の非同期キャッシュ (`remotelist::DetailSlot`) を
//! 共有モジュールへ切り出してある。issue 固有なのは詳細の組み立てと state 絞り込みの
//! カーディナリティ (open/closed/all) だけ。
//!
//! **体感速度改善**: 詳細を開くのに以前は `gh issue view` (本文) + `gh issue view --comments`
//! (コメント) の 2 往復が要った。本文は一覧取得 (`gh issue list`) の時点で `RemoteItem::body`
//! に受け取っておき、Enter を押した瞬間は rows から即座に組み立てて描画する (ネットワーク
//! 往復ゼロ)。コメントだけを非同期の 1 往復で取りに行き、届くまでは「コメント読み込み中…」を
//! 本文の下に添える (`build_detail_display`)。DetailSlot の役割はコメントキャッシュに変わった
//! だけで、キャッシュ・二重起動防止・poll の仕組みはそのまま使う
pub mod view;

use std::sync::mpsc::Receiver;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::ListState;

use crate::component::remotelist::{DetailSlot, ListMatch, PollOutcome, filter_rows};
use crate::component::viewer::Viewport;
use crate::github::RemoteItem;
use crate::lang::t;
use crate::text;

/// `t` で循環する state 絞り込み。一覧そのものは常に `--state all` で 1 回だけ取得し、
/// ここはローカルフィルタに徹する (gh を余計に叩かないため)
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StateFilter {
    Open,
    Closed,
    All,
}

impl StateFilter {
    pub fn next(self) -> Self {
        match self {
            StateFilter::Open => StateFilter::Closed,
            StateFilter::Closed => StateFilter::All,
            StateFilter::All => StateFilter::Open,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StateFilter::Open => "open",
            StateFilter::Closed => "closed",
            StateFilter::All => "all",
        }
    }

    fn accepts(self, state: &str) -> bool {
        match self {
            StateFilter::All => true,
            StateFilter::Open => state.eq_ignore_ascii_case("open"),
            StateFilter::Closed => !state.eq_ignore_ascii_case("open"),
        }
    }
}

pub struct IssuesState {
    // component/remotelist/view.rs::draw_remote_list へ直接フィールドとして渡す (list_state と
    // 同時に借りるため、メソッド越しだと不透明な借用になり同時に借りられない)
    pub rows: Vec<RemoteItem>,
    /// 初回取得が完了したかどうか。true になったらタブを往復しても自動では再取得しない
    fetched: bool,
    list_loading: bool,
    list_rx: Option<Receiver<Result<Vec<RemoteItem>, String>>>,
    list_error: Option<String>,
    pub state_filter: StateFilter,
    pub query: String,
    /// `/` で編集中の間だけ Some。Esc でここへ戻す (viewer の cancel_search と違い
    /// 一覧の絞り込みは常設状態なので、キャンセル時は編集前のクエリへ復元する)
    filter_snapshot: Option<String>,
    pub matches: Vec<ListMatch>,
    pub selected: usize,
    pub list_state: ListState,
    /// 一覧ペインの実測高さ。ui 側が毎フレーム書き戻す (viewport.height と同じ ui→app パターン)。
    /// Ctrl+d/u の半ページ移動に使う
    pub list_area_height: usize,

    /// 番号ごとのコメントキャッシュ (`gh issue view --comments` のプレーン出力を Line 化したもの)。
    /// 本文は一覧取得時点で RemoteItem::body に入っているため、ここはコメントだけを持つ
    /// (以前は本文込みの detail をここに持っていたが、体感速度改善で本文は即座に組み立てる側へ
    /// 移した)。PR タブの説明/diff/CI と同じ形なので remotelist::DetailSlot を共有する
    comments: DetailSlot<Vec<Line<'static>>>,
    /// 右ペインに表示中の issue 番号。selected (一覧側カーソル) とは別に持ち、
    /// j/k では追従させない (Enter/l/クリックでのみ開く。GIT ツリー・LOG 一覧と同じ理由)
    open_number: Option<u64>,
    /// header + body + comments (取得中/エラーならその旨) を組み立て済みの表示行。
    /// request_open / poll のたびに rebuild_display で作り直す。lines() はこれを返すだけ
    display: Vec<Line<'static>>,
    /// `o` (ブラウザで開く) の結果待ち。多重起動防止だけが目的で、成功時は何も表示しない
    open_rx: Option<Receiver<Result<(), String>>>,

    /// 詳細は prose なので GIT/LOG の diff Viewport と違い config の wrap_default に連動させず
    /// 常に wrap=true で始める (折返しを切るキーを割り当てていないので変更経路も無い)
    pub viewport: Viewport,
}

impl IssuesState {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            fetched: false,
            list_loading: false,
            list_rx: None,
            list_error: None,
            state_filter: StateFilter::Open,
            query: String::new(),
            filter_snapshot: None,
            matches: Vec::new(),
            selected: 0,
            list_state: ListState::default(),
            list_area_height: 0,
            comments: DetailSlot::new(),
            open_number: None,
            display: Vec::new(),
            open_rx: None,
            viewport: Viewport::new(true),
        }
    }

    pub fn fetched(&self) -> bool {
        self.fetched
    }

    pub fn list_loading(&self) -> bool {
        self.list_loading
    }

    pub fn list_error(&self) -> Option<&str> {
        self.list_error.as_deref()
    }

    pub fn total(&self) -> usize {
        self.rows.len()
    }

    pub fn visible_count(&self) -> usize {
        self.matches.len()
    }

    // component/issues/view.rs が一覧描画で match.row から元データを引くために公開する
    pub fn row(&self, idx: usize) -> Option<&RemoteItem> {
        self.rows.get(idx)
    }

    pub fn selected_row(&self) -> Option<&RemoteItem> {
        let m = self.matches.get(self.selected)?;
        self.row(m.row)
    }

    pub fn selected_number(&self) -> Option<u64> {
        self.selected_row().map(|r| r.number)
    }

    /// App::refresh_issues が呼ぶ。実際のジョブ起動は呼び出し側 (root が要る) が行うので、
    /// ここでは「取得を始める」という状態遷移だけを持つ
    pub fn begin_list_fetch(&mut self, rx: Receiver<Result<Vec<RemoteItem>, String>>) {
        self.list_loading = true;
        self.list_error = None;
        self.list_rx = Some(rx);
    }

    pub fn cycle_state_filter(&mut self) {
        self.state_filter = self.state_filter.next();
        self.rescan();
    }

    pub fn set_query(&mut self, query: String) {
        self.query = query;
        self.rescan();
    }

    /// `/` を開いた瞬間に呼び、Esc で戻す先を確定する
    pub fn begin_filter_edit(&mut self) {
        self.filter_snapshot = Some(self.query.clone());
    }

    /// Esc: 編集前のクエリへ戻す
    pub fn cancel_filter_edit(&mut self) {
        let restore = self.filter_snapshot.take().unwrap_or_default();
        self.set_query(restore);
    }

    /// Enter: 編集中の内容をそのまま確定する (既に live 反映済みなので snapshot を捨てるだけ)
    pub fn confirm_filter_edit(&mut self) {
        self.filter_snapshot = None;
    }

    // フィルタ・スコアリングの実アルゴリズムは remotelist::filter_rows (#34 と共有)。
    // ここでは state_filter の意味 (open/closed/all) を accepts 述語として渡すだけ
    fn rescan(&mut self) {
        let state_filter = self.state_filter;
        self.matches = filter_rows(&self.rows, &self.query, |s| state_filter.accepts(s));
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.matches.is_empty() {
            return;
        }
        let last = self.matches.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
    }

    pub fn select_top(&mut self) {
        self.selected = 0;
    }

    pub fn select_bottom(&mut self) {
        self.selected = self.matches.len().saturating_sub(1);
    }

    /// Enter/l/クリック: 選択中 issue の詳細を開く。本文は即座に (rebuild_display で)
    /// 描画できる状態にし、true を返したら呼び出し側 (App) が job::spawn でコメント取得の
    /// ジョブを起動する必要がある (未キャッシュ・未取得中のとき)
    pub fn request_open(&mut self, number: u64) -> bool {
        self.open_number = Some(number);
        self.viewport.scroll = 0;
        self.viewport.hscroll = 0;
        let need_fetch = self.comments.request(number);
        self.rebuild_display();
        need_fetch
    }

    // ジョブ側 (App::open_selected_issue) が gh の生行を Line 化してから送ってくるので、
    // ここは DetailSlot へそのまま渡すだけ (詳細キャッシュのスレッド構成を増やさない)
    pub fn begin_comments_fetch(
        &mut self,
        rx: Receiver<(u64, Result<Vec<Line<'static>>, String>)>,
    ) {
        self.comments.begin_fetch(rx);
    }

    // request_open (コメント要求直後) と poll (コメント到着時) の両方から呼ぶ。本文は常に
    // rows から即座に組み立て、コメント部分だけキャッシュ/取得中/エラーで出し分ける
    fn rebuild_display(&mut self) {
        let Some(number) = self.open_number else {
            self.display = Vec::new();
            return;
        };
        let Some(row) = self.rows.iter().find(|r| r.number == number) else {
            self.display = Vec::new();
            return;
        };
        self.display = build_detail_display(
            row,
            self.comments.get(number).map(Vec::as_slice),
            self.comments.loading(number),
            self.comments.error(number),
        );
    }

    pub fn begin_open_web(&mut self, rx: Receiver<Result<(), String>>) {
        self.open_rx = Some(rx);
    }

    pub fn open_web_in_flight(&self) -> bool {
        self.open_rx.is_some()
    }

    /// タイトルバー表示用。開いている issue が無ければ None
    pub fn title(&self) -> Option<String> {
        let number = self.open_number?;
        let row = self.rows.iter().find(|r| r.number == number)?;
        Some(format!("#{}  {}", row.number, row.title))
    }

    // 本文は rebuild_display で常に即座に組み立て済みなので、描画側 (draw_issues_detail) を
    // 全体ブロックするような loading/error はもう無い。コメント取得中/失敗はここではなく
    // display 内の該当行として埋め込む (build_detail_display 参照)
    pub fn lines(&self) -> &[Line<'static>] {
        &self.display
    }

    pub fn line_count(&self) -> usize {
        self.lines().len()
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let last = self.line_count().saturating_sub(1);
        self.viewport.scroll_by(delta, last);
    }

    pub fn jump_to_top(&mut self) {
        self.viewport.scroll = 0;
    }

    pub fn jump_to_bottom(&mut self) {
        let total = self.line_count();
        let last = total.saturating_sub(1);
        self.viewport.scroll = total.saturating_sub(self.viewport.height).min(last);
    }

    /// on_tick から毎 tick 呼ぶ。list/detail/open の 3 ジョブを drain するだけで専用タイマーは
    /// 作らない (job.rs の既存方針)。open (ブラウザ起動) の失敗だけは一覧・詳細と違って
    /// 表示する専用の場所が無いので、呼び出し側の一時 notice に転送してもらう
    pub fn poll(&mut self) -> PollOutcome {
        let mut outcome = PollOutcome::default();
        if let Some(rx) = &self.list_rx
            && let Ok(result) = rx.try_recv()
        {
            self.list_rx = None;
            self.list_loading = false;
            outcome.changed = true;
            match result {
                Ok(rows) => {
                    self.rows = rows;
                    self.fetched = true;
                    self.rescan();
                }
                Err(message) => self.list_error = Some(message),
            }
        }
        if let Some(number) = self.comments.poll() {
            outcome.changed = true;
            // 本文は既に表示済み (request_open で即描画済み) なので、コメント到着時に
            // viewport をリセットしない — 読んでいる途中でスクロール位置が飛ぶのを避ける
            if self.open_number == Some(number) {
                self.rebuild_display();
            }
        }
        if let Some(rx) = &self.open_rx
            && let Ok(result) = rx.try_recv()
        {
            self.open_rx = None;
            outcome.changed = true;
            if let Err(message) = result {
                outcome.notice = Some((message, true));
            }
        }
        outcome
    }
}

impl Default for IssuesState {
    fn default() -> Self {
        Self::new()
    }
}

// gh の出力をそのまま Line 化する。gutter (span[0]) は行番号を持たないので空のままにするが、
// 「span[0] = gutter 固定」というインバリアント自体は崩さない (TextPane が前提にするため)。
// PR タブ (#34) の説明/CI ステータス表示もプレーンテキストという点で同じなので共有する
pub(crate) fn build_detail_lines(raw: &[String]) -> Vec<Line<'static>> {
    raw.iter()
        .map(|line| {
            let content = text::normalize(line);
            Line::from(vec![Span::raw(""), Span::raw(content)])
        })
        .collect()
}

fn detail_line(content: String, style: Style) -> Line<'static> {
    Line::from(vec![Span::raw(""), Span::styled(content, style)])
}

// RemoteItem (一覧取得済みの行データ) からヘッダー + 本文を組み立てる。ネットワークを
// 一切使わない (gh を待たない) ので Enter を押した瞬間にそのまま描ける。issues の詳細と
// PR タブ (#34) の説明表示のどちらもこれを使う (row の実体はどちらも RemoteItem)
pub(crate) fn build_body_lines(row: &RemoteItem) -> Vec<Line<'static>> {
    let mut lines = vec![
        detail_line(
            format!("#{}  {}", row.number, row.title),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        detail_line(
            format!("{}  @{}  {}", row.state, row.author, row.updated_at),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    if !row.labels.is_empty() {
        lines.push(detail_line(
            format!("labels: {}", row.labels.join(", ")),
            Style::default().fg(Color::Cyan),
        ));
    }
    lines.push(Line::default());
    if row.body.trim().is_empty() {
        lines.push(detail_line(
            "(no description)".to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        lines.extend(
            row.body
                .lines()
                .map(|raw| detail_line(text::normalize(raw), Style::default())),
        );
    }
    lines
}

// build_body_lines (本文、常に即座に組み立て済み) の下に、コメントの状態
// (キャッシュ済み/取得中/エラー) を継ぎ足す。issues の詳細と PR の説明表示が共有する
// (どちらも「本文は即描画、コメントだけ非同期」という同じ形のため)
pub(crate) fn build_detail_display(
    row: &RemoteItem,
    comments: Option<&[Line<'static>]>,
    loading: bool,
    error: Option<&str>,
) -> Vec<Line<'static>> {
    let mut lines = build_body_lines(row);
    lines.push(Line::default());
    lines.push(detail_line(
        "─── comments ───".to_string(),
        Style::default().fg(Color::DarkGray),
    ));
    lines.push(Line::default());
    if let Some(comments) = comments {
        if comments.is_empty() {
            lines.push(detail_line(
                "(no comments)".to_string(),
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            lines.extend(comments.iter().cloned());
        }
    } else if loading {
        lines.push(detail_line(
            t("コメント読み込み中…", "loading comments…").to_string(),
            Style::default().fg(Color::DarkGray),
        ));
    } else if let Some(err) = error {
        lines.push(detail_line(
            crate::tr!(
                "コメント取得に失敗しました: {err}",
                "failed to fetch comments: {err}"
            ),
            Style::default().fg(Color::Red),
        ));
    }
    lines
}
