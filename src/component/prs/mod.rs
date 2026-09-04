//! GitHub pull requests タブ (#34) の状態。左ペインは一覧 (issues タブ #33 と同じ
//! remotelist::{filter_rows, DetailSlot} を再利用したフィルタ + キャッシュ)、右ペインは
//! 選択 PR の 3 表示 (説明・diff・CI ステータス) を切り替える。PR 固有なのは
//! headRefName/isDraft (github::PrRow) と、右ペイン 3 種の切替・diff の hunk/wrap/hscroll
//! だけに閉じており、一覧側の実装は issues タブと完全に共有する。
//!
//! **体感速度改善**: 説明表示も issues の詳細と同じ理由で本文を即座に組み立てる。
//! `RemoteItem::body` (一覧取得時点で受け取り済み) から `issues::build_body_lines` で
//! ヘッダー + 本文を組み立て、コメントだけ `gh pr view --comments` の非同期 1 往復で取りに行く
//! (`description` フィールドの役割はコメントキャッシュに変わった)。diff と CI ステータスは
//! 一覧に含まれないデータなので取得が要るが、`d`/`S` を押した瞬間の 1 往復待ちを無くすため
//! PR を開いた時点でバックグラウンド先読みする (`PrefetchStage` 節参照)
pub mod view;

use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use ratatui::text::Line;

use crate::component::gitlane;
use crate::component::issues::{build_detail_display, build_detail_lines};
use crate::component::remotelist::{DetailSlot, ListMatch, ListRow, PollOutcome, filter_rows};
use crate::component::viewer::{Viewport, rowcursor};
use crate::git;
use crate::github::{self, PrRow};
use crate::lang::t;
use crate::text;
use crate::widget::text_pane::line_body;

impl ListRow for PrRow {
    fn title(&self) -> &str {
        &self.item.title
    }
    fn state(&self) -> &str {
        &self.item.state
    }
}

/// `t` で循環する state 絞り込み。PR は issues の open/closed/all と違い merged を独立の
/// 状態として持つ (`gh pr list` の state は OPEN/CLOSED/MERGED)
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PrStateFilter {
    Open,
    Closed,
    Merged,
    All,
}

impl PrStateFilter {
    pub fn next(self) -> Self {
        match self {
            PrStateFilter::Open => PrStateFilter::Closed,
            PrStateFilter::Closed => PrStateFilter::Merged,
            PrStateFilter::Merged => PrStateFilter::All,
            PrStateFilter::All => PrStateFilter::Open,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PrStateFilter::Open => "open",
            PrStateFilter::Closed => "closed",
            PrStateFilter::Merged => "merged",
            PrStateFilter::All => "all",
        }
    }

    fn accepts(self, state: &str) -> bool {
        match self {
            PrStateFilter::All => true,
            PrStateFilter::Open => state.eq_ignore_ascii_case("open"),
            PrStateFilter::Closed => state.eq_ignore_ascii_case("closed"),
            PrStateFilter::Merged => state.eq_ignore_ascii_case("merged"),
        }
    }
}

/// 右ペインの表示切替 (既定 = 説明、`d` = diff、`S` = CI ステータス)
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DetailView {
    Description,
    Diff,
    Checks,
}

/// Enter/l/クリックで PR を開いた ~400ms 後に diff → CI の順で静かに先読みする状態機械。
/// Enter 連打で一覧を流し読みする使い方で無駄弾を撃たないよう、`Pending` の間は何も撃たず
/// タイマーが切れるまで待つ。同時に走らせるジョブを高々1本にするため、diff のジョブが
/// 終わるまで CI のジョブは起動しない (`DiffInFlight` → `ChecksInFlight` の順に一方向に進む)
#[derive(Clone, Copy)]
enum PrefetchStage {
    Idle,
    Pending(u64, Instant),
    DiffInFlight(u64),
    ChecksInFlight(u64),
}

const PREFETCH_DELAY: Duration = Duration::from_millis(400);

/// diff 表示用に組み立て済みのデータ。gitlane::render_commit の戻り値 (LOG レーンの
/// CommitDiff と同じ形) に、打ち切りが発生したかどうかを添えたもの
pub struct PrDiffData {
    lines: Vec<Line<'static>>,
    hunks: Vec<usize>,
    gutter_width: usize,
    max_width: usize,
    boundaries: Vec<(usize, String)>,
    pub truncated: bool,
}

pub struct PrsState {
    // component/remotelist/view.rs::draw_remote_list へ直接フィールドとして渡す (list_state と
    // 同時に借りるため、メソッド越しだと不透明な借用になり同時に借りられない。issues::IssuesState
    // と同じ理由)
    pub rows: Vec<PrRow>,
    fetched: bool,
    list_loading: bool,
    list_rx: Option<Receiver<Result<Vec<PrRow>, String>>>,
    list_error: Option<String>,
    pub state_filter: PrStateFilter,
    pub query: String,
    filter_snapshot: Option<String>,
    pub matches: Vec<ListMatch>,
    pub selected: usize,
    pub list_state: ratatui::widgets::ListState,
    pub list_area_height: usize,

    pub view: DetailView,
    open_number: Option<u64>,
    /// 説明表示のコメントキャッシュ (以前は本文込みの detail をここに持っていたが、体感速度
    /// 改善で本文は RemoteItem::body から即座に組み立てる側へ移した。issues::IssuesState
    /// と同じ理由)
    comments: DetailSlot<Vec<Line<'static>>>,
    /// header + body + comments を組み立て済みの表示行 (説明表示のみ使う。diff/checks は
    /// 従来通り DetailSlot のキャッシュをそのまま描く)
    description_display: Vec<Line<'static>>,
    diff: DetailSlot<PrDiffData>,
    checks: DetailSlot<Vec<Line<'static>>>,
    open_rx: Option<Receiver<Result<(), String>>>,

    /// diff/CI の先読み状態機械 (`PrefetchStage` 参照)
    prefetch: PrefetchStage,
    /// 打ち切り notice を「実際に diff を表示した時」に一度だけ出すための既通知集合。
    /// 先読み経由で静かにキャッシュへ入った場合はここに入れず、後から `d` で表示した
    /// 瞬間に初めて通知する (poll 完了時に notice を出す既存経路と合流させる、後述)
    notified_truncation: std::collections::HashSet<u64>,

    /// 説明/CI は issues の詳細と同じくプロースなので常時 wrap 固定の Viewport を共有する。
    /// diff だけ GIT/LOG と同じ wrap トグル・hscroll・hunk ジャンプを持つため別の Viewport にする
    /// (別ドキュメントなので位置を共有する意味がなく、表示を切り替えても互いの読み位置を壊さない)
    pub text_viewport: Viewport,
    pub diff_viewport: Viewport,
    /// diff 表示の行カーソル。説明/CI (プロース) には持たせない — 折返しの効いた散文では
    /// 「1 論理行」が段落まるごとになり、行を単位にした帯もカーソル移動も単位が合わない。
    /// issues の詳細ペインを行カーソルの対象外にしているのと同じ理由
    diff_cursor: usize,
}

impl PrsState {
    pub fn new(wrap: bool) -> Self {
        Self {
            rows: Vec::new(),
            fetched: false,
            list_loading: false,
            list_rx: None,
            list_error: None,
            state_filter: PrStateFilter::Open,
            query: String::new(),
            filter_snapshot: None,
            matches: Vec::new(),
            selected: 0,
            list_state: ratatui::widgets::ListState::default(),
            list_area_height: 0,
            view: DetailView::Description,
            open_number: None,
            comments: DetailSlot::new(),
            description_display: Vec::new(),
            diff: DetailSlot::new(),
            checks: DetailSlot::new(),
            open_rx: None,
            prefetch: PrefetchStage::Idle,
            notified_truncation: std::collections::HashSet::new(),
            text_viewport: Viewport::new(true),
            diff_viewport: Viewport::new(wrap),
            diff_cursor: 0,
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

    pub fn row(&self, idx: usize) -> Option<&PrRow> {
        self.rows.get(idx)
    }

    pub fn selected_row(&self) -> Option<&PrRow> {
        let m = self.matches.get(self.selected)?;
        self.row(m.row)
    }

    pub fn selected_number(&self) -> Option<u64> {
        self.selected_row().map(|r| r.item.number)
    }

    pub fn begin_list_fetch(&mut self, rx: Receiver<Result<Vec<PrRow>, String>>) {
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

    pub fn begin_filter_edit(&mut self) {
        self.filter_snapshot = Some(self.query.clone());
    }

    pub fn cancel_filter_edit(&mut self) {
        let restore = self.filter_snapshot.take().unwrap_or_default();
        self.set_query(restore);
    }

    pub fn confirm_filter_edit(&mut self) {
        self.filter_snapshot = None;
    }

    // issues タブと同じ remotelist::filter_rows を使う (絞り込みロジックを 2 回書かない)
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

    pub fn open_number(&self) -> Option<u64> {
        self.open_number
    }

    /// Enter/l/クリック (常に説明表示) と d/S (diff/CI へ表示切替) が共通で呼ぶ。
    /// 対象 PR・表示が変わった時だけ、その表示のドキュメントとしての読み位置をリセットする
    /// タブに入った時点で、まだ描かれたことのない Viewport に右ペインの実測値を入れる。
    /// 既に描かれている側 (height != 0) は次の描画で正しく上書きされるので触らない
    pub fn seed_viewport_size(&mut self, height: usize, width: usize) {
        for vp in [&mut self.text_viewport, &mut self.diff_viewport] {
            if vp.height == 0 {
                vp.height = height;
                vp.width = width;
            }
        }
    }

    pub fn set_open(&mut self, number: u64, view: DetailView) {
        let changed = self.open_number != Some(number) || self.view != view;
        // 3 種の表示は同じ Rect を使うので、切替先の Viewport にも今表示していた側の実測値を
        // 引き継ぐ。切替直後の 1 打鍵 (次の描画で書き戻されるより前) でカーソル追従が
        // 暴れないようにするため (GitState::new が右ペインのサイズを受け取るのと同じ理由)
        let measured = (
            self.current_viewport().height,
            self.current_viewport().width,
        );
        self.open_number = Some(number);
        self.view = view;
        if changed {
            let vp = self.current_viewport_mut();
            vp.scroll = 0;
            vp.hscroll = 0;
            vp.height = measured.0;
            vp.width = measured.1;
            self.diff_cursor = 0;
        }
    }

    /// App::open_selected_pr (Enter/l/クリック) だけが呼ぶ。j/k による選択移動では呼ばれない
    /// ため、先読みはキーリピートで撃たれない。同じ PR を開き直しても最初からやり直す
    /// (直前の先読みが CI 待ちの途中でも、対象がずれていなければ実害は無くタイマーが
    /// リセットされるだけ)
    pub fn note_opened(&mut self, number: u64) {
        self.prefetch = PrefetchStage::Pending(number, Instant::now());
    }

    /// on_tick から毎 tick 呼ぶ。先読みの状態機械を高々 1 段階だけ進め、ジョブを起動すべき
    /// なら (number, view) を返す (App::dispatch_pr_prefetch が job::spawn する)。
    /// `DetailSlot::request` の既存の重複防止をそのまま使うので、既にキャッシュ済み・
    /// 取得中の対象へは重複して起動しない
    pub fn advance_prefetch(&mut self) -> Option<(u64, DetailView)> {
        match self.prefetch {
            PrefetchStage::Idle => None,
            PrefetchStage::Pending(number, started) => {
                // 別の PR を開き直していたら、古い対象への先読みはもう意味が無い
                if self.open_number != Some(number) {
                    self.prefetch = PrefetchStage::Idle;
                    return None;
                }
                if started.elapsed() < PREFETCH_DELAY {
                    return None;
                }
                self.start_diff_prefetch(number)
            }
            PrefetchStage::DiffInFlight(number) => {
                if self.diff.loading(number) {
                    return None; // diff のジョブがまだ終わっていない
                }
                if self.open_number != Some(number) {
                    self.prefetch = PrefetchStage::Idle;
                    return None;
                }
                self.start_checks_prefetch(number)
            }
            PrefetchStage::ChecksInFlight(number) => {
                if self.checks.loading(number) {
                    return None;
                }
                self.prefetch = PrefetchStage::Idle;
                None
            }
        }
    }

    fn start_diff_prefetch(&mut self, number: u64) -> Option<(u64, DetailView)> {
        if self.diff.request(number) {
            self.prefetch = PrefetchStage::DiffInFlight(number);
            return Some((number, DetailView::Diff));
        }
        // 既にキャッシュ済み・取得中なら diff は撃たず、続けて CI を試す
        self.start_checks_prefetch(number)
    }

    fn start_checks_prefetch(&mut self, number: u64) -> Option<(u64, DetailView)> {
        if self.checks.request(number) {
            self.prefetch = PrefetchStage::ChecksInFlight(number);
            return Some((number, DetailView::Checks));
        }
        self.prefetch = PrefetchStage::Idle;
        None
    }

    /// diff を実際に表示している瞬間 (`d` を押した直後、または先読み済みキャッシュへの
    /// 表示切替) だけ打ち切り notice を出す。先読みがバックグラウンドで完了した時点
    /// (view はまだ Description) では素通りし、番号ごとに 1 度だけ通知する
    fn truncation_notice_if_needed(&mut self, number: u64) -> Option<(String, bool)> {
        if self.view != DetailView::Diff || self.open_number != Some(number) {
            return None;
        }
        if !self.diff.get(number).is_some_and(|d| d.truncated) {
            return None;
        }
        if !self.notified_truncation.insert(number) {
            return None;
        }
        Some((
            t(
                "diff が大きいため表示を打ち切りました (20000 行 / 2MB)",
                "diff too large — truncated (20000 lines / 2MB)",
            )
            .to_string(),
            true,
        ))
    }

    /// App::switch_pr_view (d) が dispatch_pr_fetch の直後に呼ぶ。先読みで既にキャッシュ済み
    /// の diff を初めて表示する瞬間はジョブが起動しない (request が false)ため、poll 側の
    /// 通知だけでは打ち切りを知らせ損なう。ここで現在表示中の対象に対して同じ判定をかける
    pub fn truncation_notice_for_current(&mut self) -> Option<(String, bool)> {
        let number = self.open_number?;
        self.truncation_notice_if_needed(number)
    }

    /// App::dispatch_pr_fetch が呼ぶ。今の (open_number, view) が未キャッシュ・未取得中なら
    /// Some を返し、呼び出し側が対応する gh コマンドの job を起動する。Description は
    /// 本文がネットワーク不要 (RemoteItem::body から即座に組み立て) なので、キャッシュ済み・
    /// 取得中に関わらず毎回 rebuild_description_display で表示を最新化してから判定する
    /// (set_open 直後の対象・表示切替を逃さず本文をすぐ描くため)
    pub fn request_current(&mut self) -> Option<(u64, DetailView)> {
        let number = self.open_number?;
        let need = match self.view {
            DetailView::Description => {
                let need = self.comments.request(number);
                self.rebuild_description_display();
                need
            }
            DetailView::Diff => self.diff.request(number),
            DetailView::Checks => self.checks.request(number),
        };
        need.then_some((number, self.view))
    }

    // request_current (コメント要求直後) と poll (コメント到着時) の両方から呼ぶ。
    // issues::build_detail_display と同じ組み立てを PrRow::item (RemoteItem) に対して行う
    fn rebuild_description_display(&mut self) {
        let Some(number) = self.open_number else {
            self.description_display = Vec::new();
            return;
        };
        let Some(row) = self.rows.iter().find(|r| r.item.number == number) else {
            self.description_display = Vec::new();
            return;
        };
        self.description_display = build_detail_display(
            &row.item,
            self.comments.get(number).map(Vec::as_slice),
            self.comments.loading(number),
            self.comments.error(number),
        );
    }

    pub fn begin_comments_fetch(
        &mut self,
        rx: Receiver<(u64, Result<Vec<Line<'static>>, String>)>,
    ) {
        self.comments.begin_fetch(rx);
    }

    pub fn begin_diff_fetch(&mut self, rx: Receiver<(u64, Result<PrDiffData, String>)>) {
        self.diff.begin_fetch(rx);
    }

    pub fn begin_checks_fetch(&mut self, rx: Receiver<(u64, Result<Vec<Line<'static>>, String>)>) {
        self.checks.begin_fetch(rx);
    }

    pub fn begin_open_web(&mut self, rx: Receiver<Result<(), String>>) {
        self.open_rx = Some(rx);
    }

    pub fn open_web_in_flight(&self) -> bool {
        self.open_rx.is_some()
    }

    /// タイトルバー表示用。表示中の種類も添える (issues と違い右ペインが 3 種あるため)
    pub fn title(&self) -> Option<String> {
        let number = self.open_number?;
        let row = self.rows.iter().find(|r| r.item.number == number)?;
        let kind = match self.view {
            DetailView::Description => "description",
            DetailView::Diff => "diff",
            DetailView::Checks => "checks",
        };
        Some(format!(
            "#{}  {}  [{kind}]",
            row.item.number, row.item.title
        ))
    }

    // Description は本文がネットワーク不要で常に即座に描けるので、ここでは常に false を返す
    // (コメントの取得中/失敗は description_display の中に埋め込まれている。issues の詳細と同じ形)
    pub fn loading_current(&self) -> bool {
        let Some(number) = self.open_number else {
            return false;
        };
        match self.view {
            DetailView::Description => false,
            DetailView::Diff => self.diff.loading(number),
            DetailView::Checks => self.checks.loading(number),
        }
    }

    pub fn error_current(&self) -> Option<&str> {
        let number = self.open_number?;
        match self.view {
            DetailView::Description => None,
            DetailView::Diff => self.diff.error(number),
            DetailView::Checks => self.checks.error(number),
        }
    }

    pub fn truncated_current(&self) -> bool {
        let Some(number) = self.open_number else {
            return false;
        };
        self.view == DetailView::Diff && self.diff.get(number).is_some_and(|d| d.truncated)
    }

    pub fn lines(&self) -> &[Line<'static>] {
        let Some(number) = self.open_number else {
            return &[];
        };
        match self.view {
            DetailView::Description => self.description_display.as_slice(),
            DetailView::Checks => self.checks.get(number).map_or(&[], |v| v.as_slice()),
            DetailView::Diff => self.diff.get(number).map_or(&[], |d| d.lines.as_slice()),
        }
    }

    pub fn line_count(&self) -> usize {
        self.lines().len()
    }

    pub fn gutter_width(&self) -> usize {
        if self.view != DetailView::Diff {
            return 0;
        }
        let Some(number) = self.open_number else {
            return 0;
        };
        self.diff.get(number).map_or(0, |d| d.gutter_width)
    }

    pub fn boundaries(&self) -> &[(usize, String)] {
        if self.view != DetailView::Diff {
            return &[];
        }
        let Some(number) = self.open_number else {
            return &[];
        };
        self.diff
            .get(number)
            .map_or(&[], |d| d.boundaries.as_slice())
    }

    pub fn has_file_boundary(&self) -> bool {
        !self.boundaries().is_empty()
    }

    /// LOG/GIT のまとめ diff と同じ探索 (gitlane::sticky_label) を共有する
    pub fn sticky_label(&self) -> Option<&str> {
        gitlane::sticky_label(self.boundaries(), self.diff_viewport.scroll)
    }

    pub fn current_viewport(&self) -> &Viewport {
        match self.view {
            DetailView::Diff => &self.diff_viewport,
            _ => &self.text_viewport,
        }
    }

    pub fn current_viewport_mut(&mut self) -> &mut Viewport {
        match self.view {
            DetailView::Diff => &mut self.diff_viewport,
            _ => &mut self.text_viewport,
        }
    }

    /// ホイール等の「画面を動かす」操作。diff 表示中はカーソルを画面内へ引き戻して連れて動かす
    pub fn scroll_by(&mut self, delta: isize) {
        let last = self.line_count().saturating_sub(1);
        self.current_viewport_mut().scroll_by(delta, last);
        if self.view != DetailView::Diff {
            return;
        }
        let (count, wrapped, width) = self.cursor_metrics();
        self.diff_cursor =
            rowcursor::clamp_cursor(&self.diff_viewport, self.diff_cursor, count, wrapped, |i| {
                self.rows_at(i, width)
            });
    }

    /// j/k・Ctrl+d/u。diff はカーソルを動かして画面を追従させ、説明/CI は素直にスクロールする
    /// (プロースに行カーソルを持たせない理由は diff_cursor のコメント参照)
    pub fn move_cursor(&mut self, delta: isize) {
        if self.view != DetailView::Diff {
            self.scroll_by(delta);
            return;
        }
        let last = self.line_count().saturating_sub(1) as isize;
        self.diff_cursor = (self.diff_cursor as isize + delta).clamp(0, last.max(0)) as usize;
        self.ensure_cursor_visible();
    }

    /// diff 表示中の行カーソル。説明/CI 表示中は None (帯を出さない)
    pub fn cursor(&self) -> Option<usize> {
        (self.view == DetailView::Diff).then_some(self.diff_cursor)
    }

    /// クリックしたペイン内 row → カーソル (sticky header の 1 行は呼び出し側が差し引く)
    pub fn click_row(&mut self, row: usize) {
        if self.view != DetailView::Diff {
            return;
        }
        let (count, wrapped, width) = self.cursor_metrics();
        self.diff_cursor = rowcursor::line_at_row(&self.diff_viewport, row, count, wrapped, |i| {
            self.rows_at(i, width)
        });
        self.ensure_cursor_visible();
    }

    fn ensure_cursor_visible(&mut self) {
        let (count, wrapped, width) = self.cursor_metrics();
        let scroll =
            rowcursor::scroll_for(&self.diff_viewport, self.diff_cursor, count, wrapped, |i| {
                self.rows_at(i, width)
            });
        self.diff_viewport.scroll = scroll;
    }

    fn cursor_metrics(&self) -> (usize, bool, usize) {
        let width = self
            .diff_viewport
            .width
            .saturating_sub(self.gutter_width())
            .max(1);
        (self.line_count(), self.diff_viewport.wrap, width)
    }

    fn rows_at(&self, i: usize, width: usize) -> usize {
        match self.lines().get(i) {
            Some(line) => text::wrap_rows(&line_body(line), width),
            None => 1,
        }
    }

    pub fn jump_to_top(&mut self) {
        self.diff_cursor = 0;
        self.current_viewport_mut().scroll = 0;
    }

    pub fn jump_to_bottom(&mut self) {
        if self.view != DetailView::Diff {
            let total = self.line_count();
            let last = total.saturating_sub(1);
            let height = self.current_viewport().height;
            self.current_viewport_mut().scroll = total.saturating_sub(height).min(last);
            return;
        }
        self.diff_cursor = self.line_count().saturating_sub(1);
        self.ensure_cursor_visible();
    }

    /// diff 表示中だけ有効 (説明/CI は issues と同じく wrap 固定で hscroll を割り当てない)
    pub fn hscroll_by(&mut self, delta: isize) {
        if self.view != DetailView::Diff {
            return;
        }
        let Some(number) = self.open_number else {
            return;
        };
        let max_width = self.diff.get(number).map_or(0, |d| d.max_width);
        let max = max_width.saturating_sub(self.diff_viewport.width / 2);
        self.diff_viewport.hscroll_by(delta, max);
    }

    pub fn hscroll_reset(&mut self) {
        if self.view == DetailView::Diff {
            self.diff_viewport.hscroll = 0;
        }
    }

    /// diff 表示中だけ有効 (説明/CI は wrap 固定でトグルを割り当てない)
    pub fn toggle_diff_wrap(&mut self) {
        if self.view == DetailView::Diff {
            self.diff_viewport.toggle_wrap();
        }
    }

    pub fn next_hunk(&mut self) {
        if self.view != DetailView::Diff {
            return;
        }
        let Some(number) = self.open_number else {
            return;
        };
        let cursor = self.diff_cursor;
        let Some(target) = self
            .diff
            .get(number)
            .and_then(|d| d.hunks.iter().find(|&&i| i > cursor).copied())
        else {
            return;
        };
        self.diff_cursor = target;
        self.diff_viewport.scroll = target;
    }

    pub fn prev_hunk(&mut self) {
        if self.view != DetailView::Diff {
            return;
        }
        let Some(number) = self.open_number else {
            return;
        };
        let cursor = self.diff_cursor;
        let Some(target) = self
            .diff
            .get(number)
            .and_then(|d| d.hunks.iter().rev().find(|&&i| i < cursor).copied())
        else {
            return;
        };
        self.diff_cursor = target;
        self.diff_viewport.scroll = target;
    }

    /// on_tick から毎 tick 呼ぶ。list/description/diff/checks/open の 5 ジョブを drain する
    /// (issues の poll と同じ形。専用タイマーは作らない)
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
            // 本文は既に表示済み (set_open 直後に rebuild_description_display で即描画済み)
            // なので、コメント到着時に viewport をリセットしない (issues と同じ理由)
            if self.open_number == Some(number) && self.view == DetailView::Description {
                self.rebuild_description_display();
            }
        }
        if let Some(number) = self.checks.poll() {
            outcome.changed = true;
            if self.open_number == Some(number) && self.view == DetailView::Checks {
                self.text_viewport.scroll = 0;
                self.text_viewport.hscroll = 0;
            }
        }
        if let Some(number) = self.diff.poll() {
            outcome.changed = true;
            if self.open_number == Some(number) && self.view == DetailView::Diff {
                self.diff_viewport.scroll = 0;
                self.diff_viewport.hscroll = 0;
            }
            // 先読み経由 (view がまだ Description) の完了では通知しない。実際に表示した
            // 瞬間の通知は truncation_notice_for_current 側 (App::switch_pr_view) が兼ねる
            outcome.notice = self.truncation_notice_if_needed(number);
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

impl Default for PrsState {
    fn default() -> Self {
        Self::new(false)
    }
}

/// 説明表示のコメント (`gh pr view --comments`) をジョブスレッド側で Line 化する。本文は
/// 一覧取得済みの RemoteItem::body から即座に組み立てるのでここでは取りに行かない。
/// issues の詳細と同じ build_detail_lines を再利用する (プレーンテキスト → Line の組み立てを
/// 2 回書かない)
pub fn fetch_comments(root: &Path, number: u64) -> (u64, Result<Vec<Line<'static>>, String>) {
    let result = github::pr_comments(root, number).map(|raw| build_detail_lines(&raw));
    (number, result)
}

/// diff (`gh pr diff`) をジョブスレッド側で打ち切り + render_commit まで済ませる。
/// GIT/LOG レーンと同じレンダラをそのまま再利用する (行の組み立てを複製しない)
pub fn fetch_diff(root: &Path, number: u64) -> (u64, Result<PrDiffData, String>) {
    (number, github::pr_diff(root, number).map(build_diff_data))
}

/// 生の unified diff を表示用データへ組み替える。取得 (fetch_diff) と、gh を呼ばずに
/// 同じ画面を描くプレビュー (preview/scene.rs) が共有する
pub fn build_diff_data(raw: String) -> PrDiffData {
    let (raw_lines, truncated) = git::truncate_diff(raw);
    let (lines, hunks, gutter_width, max_width, boundaries) = gitlane::render_commit(&raw_lines);
    PrDiffData {
        lines,
        hunks,
        gutter_width,
        max_width,
        boundaries,
        truncated,
    }
}

/// CI ステータス (`gh pr checks`) も説明と同じくプレーンテキストなので build_detail_lines を使う
pub fn fetch_checks(root: &Path, number: u64) -> (u64, Result<Vec<Line<'static>>, String>) {
    let result = github::pr_checks(root, number).map(|raw| build_detail_lines(&raw));
    (number, result)
}
