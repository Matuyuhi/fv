//! GitHub issues タブ (#33) の状態。左ペインは一覧 (フィルタ + キャッシュ)、右ペインは
//! 選択 issue の詳細で、VIEW/EDIT/GIT/LOG のいずれとも独立した Viewport を持つ
//! (別ドキュメントなので位置を共有する意味がなく、Viewer タブへ戻った時の読み位置も壊さない)。
//! 一覧・詳細・ブラウザで開く、の 3 操作はすべて job.rs (#27 の非同期基盤) に乗せ、
//! イベントループをブロックしない。フィルタは finder.rs の fuzzy_match を再利用し、
//! 新しいマッチャは書かない (branch.rs::BranchState と同じ方針)。
//!
//! #34 (pull requests タブ) が一覧まわりをそのまま再利用できるよう、issue 固有の要素は
//! detail (`gh issue view` のプレーン出力) だけに閉じ、一覧のフィルタ・キャッシュ・
//! ジョブ管理は `github::RemoteItem` 型の集合として扱う (issue/PR で行の型を分けない)。

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use ratatui::text::{Line, Span};
use ratatui::widgets::ListState;

use crate::finder::fuzzy_match;
use crate::github::RemoteItem;
use crate::text;
use crate::viewer::Viewport;

// clippy::type_complexity 対策。詳細取得は「どの番号への応答か」を結果と一緒に持ち帰る必要が
// あるため (list/open と違い対象が複数ありうる)、タプルのまま Receiver の型引数にする
type DetailResult = (u64, Result<Vec<String>, String>);

/// 一覧のフィルタ結果 1 行。row は rows の index、positions はタイトル内でマッチした
/// char インデックス (branch.rs::BranchMatch と同じ形)
pub struct IssueMatch {
    pub row: usize,
    pub positions: Vec<usize>,
}

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
    rows: Vec<RemoteItem>,
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
    pub matches: Vec<IssueMatch>,
    pub selected: usize,
    pub list_state: ListState,
    /// 一覧ペインの実測高さ。ui 側が毎フレーム書き戻す (viewport.height と同じ ui→app パターン)。
    /// Ctrl+d/u の半ページ移動に使う
    pub list_area_height: usize,

    /// 番号ごとの詳細キャッシュ (`gh issue view` のプレーン出力を Line 化したもの)
    detail_cache: HashMap<u64, Vec<Line<'static>>>,
    /// 取得失敗した番号のエラー文言。再度 Enter で再試行できるよう cache とは別に持つ
    detail_errors: HashMap<u64, String>,
    /// 右ペインに表示中の issue 番号。selected (一覧側カーソル) とは別に持ち、
    /// j/k では追従させない (Enter/l/クリックでのみ開く。GIT ツリー・LOG 一覧と同じ理由)
    open_number: Option<u64>,
    detail_loading: Option<u64>,
    detail_rx: Option<Receiver<DetailResult>>,
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
            detail_cache: HashMap::new(),
            detail_errors: HashMap::new(),
            open_number: None,
            detail_loading: None,
            detail_rx: None,
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

    // ui/issues_pane.rs が一覧描画で match.row から元データを引くために公開する
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

    // クエリでスコアリング → state_filter で絞り込み、という branch.rs::BranchState と
    // 対称の 2 段階。gh 側の並び順 (updated 降順) をクエリ空の間はそのまま保つ
    fn rescan(&mut self) {
        let mut matches: Vec<IssueMatch> = if self.query.is_empty() {
            self.rows
                .iter()
                .enumerate()
                .filter(|(_, r)| self.state_filter.accepts(&r.state))
                .map(|(i, _)| IssueMatch {
                    row: i,
                    positions: Vec::new(),
                })
                .collect()
        } else {
            let mut scored: Vec<(i64, IssueMatch)> = self
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| self.state_filter.accepts(&r.state))
                .filter_map(|(i, r)| {
                    let (score, positions) = fuzzy_match(&r.title, &self.query)?;
                    Some((score, IssueMatch { row: i, positions }))
                })
                .collect();
            scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
            scored.into_iter().map(|(_, m)| m).collect()
        };
        self.selected = self.selected.min(matches.len().saturating_sub(1));
        std::mem::swap(&mut self.matches, &mut matches);
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

    /// Enter/l/クリック: 選択中 issue の詳細を開く。true を返したら呼び出し側 (App) が
    /// job::spawn でジョブを起動する必要がある (未キャッシュ・未取得中のとき)
    pub fn request_open(&mut self, number: u64) -> bool {
        self.open_number = Some(number);
        self.viewport.scroll = 0;
        self.viewport.hscroll = 0;
        if self.detail_cache.contains_key(&number) || self.detail_loading == Some(number) {
            return false;
        }
        self.detail_errors.remove(&number);
        self.detail_loading = Some(number);
        true
    }

    pub fn begin_detail_fetch(&mut self, rx: Receiver<DetailResult>) {
        self.detail_rx = Some(rx);
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

    pub fn detail_loading_current(&self) -> bool {
        self.open_number.is_some() && self.open_number == self.detail_loading
    }

    pub fn detail_error(&self) -> Option<&str> {
        let number = self.open_number?;
        self.detail_errors.get(&number).map(String::as_str)
    }

    pub fn lines(&self) -> &[Line<'static>] {
        match self.open_number.and_then(|n| self.detail_cache.get(&n)) {
            Some(lines) => lines,
            None => &[],
        }
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
    pub fn poll(&mut self) -> Option<(String, bool)> {
        if let Some(rx) = &self.list_rx
            && let Ok(result) = rx.try_recv()
        {
            self.list_rx = None;
            self.list_loading = false;
            match result {
                Ok(rows) => {
                    self.rows = rows;
                    self.fetched = true;
                    self.rescan();
                }
                Err(message) => self.list_error = Some(message),
            }
        }
        if let Some(rx) = &self.detail_rx
            && let Ok((number, result)) = rx.try_recv()
        {
            self.detail_rx = None;
            if self.detail_loading == Some(number) {
                self.detail_loading = None;
            }
            match result {
                Ok(raw) => {
                    self.detail_cache.insert(number, build_detail_lines(&raw));
                    if self.open_number == Some(number) {
                        self.viewport.scroll = 0;
                        self.viewport.hscroll = 0;
                    }
                }
                Err(message) => {
                    self.detail_errors.insert(number, message);
                }
            }
        }
        if let Some(rx) = &self.open_rx
            && let Ok(result) = rx.try_recv()
        {
            self.open_rx = None;
            if let Err(message) = result {
                return Some((message, true));
            }
        }
        None
    }
}

impl Default for IssuesState {
    fn default() -> Self {
        Self::new()
    }
}

// gh の出力をそのまま Line 化する。gutter (span[0]) は行番号を持たないので空のままにするが、
// 「span[0] = gutter 固定」というインバリアント自体は崩さない (TextPane が前提にするため)
fn build_detail_lines(raw: &[String]) -> Vec<Line<'static>> {
    raw.iter()
        .map(|line| {
            let content = text::normalize(line);
            Line::from(vec![Span::raw(""), Span::raw(content)])
        })
        .collect()
}
