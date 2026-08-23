//! LOG レーン (Shift+Tab で入るコミット履歴閲覧) の表示状態。
//! 左ペインはツリーではなくコミット一覧に差し替わる (component/log/view.rs)。右ペインは選択
//! コミットの `git show` を gitlane::render_commit で組み替えた複数ファイル diff。
//! GitState と同じく Viewer 本体には触れず、依存範囲を一覧+diff の組み立てだけに絞る。
pub mod view;

use std::path::Path;

use ratatui::text::Line;
use ratatui::widgets::ListState;

use crate::component::gitlane;
use crate::component::viewer::{Viewport, rowcursor};
use crate::git::{self, CommitSummary};
use crate::text;
use crate::widget::text_pane::line_body;

// 初回・追加取得 1 回あたりの件数。ページングは --skip をこの単位で進める
const PAGE_SIZE: usize = 200;

struct CommitDiff {
    lines: Vec<Line<'static>>,
    hunks: Vec<usize>,
    gutter_width: usize,
    max_width: usize,
    /// ファイル境界: 見出し行の index → 表示ラベル (#40 sticky header)。
    /// index 昇順で入っている前提 (render_commit がファイル出現順に push するため)
    boundaries: Vec<(usize, String)>,
}

pub struct LogState {
    commits: Vec<CommitSummary>,
    pub list_state: ListState,
    pub selected: usize,
    // 末尾まで取得しきったかどうか。true になったら load_more を呼ばない
    exhausted: bool,
    /// diff は一覧とは別ドキュメントであり、GIT レーンの diff Viewport とも別 (LOG に入っても
    /// GIT 側の読み位置を壊さない・LOG 内で別コミットに移っても意味を共有しない)
    pub viewport: Viewport,
    current: Option<CommitDiff>,
    // 現在 diff 表示中のコミット index。selected (一覧側のカーソル) と分けて持つのは、
    // j/k では diff を追従させない (Enter/l/クリックでのみ開く) ため
    open_index: Option<usize>,
    /// diff ペインの行カーソル。GIT レーンと同じく「今どの行を見ているか」を明示する
    /// (追従の計算は viewer::rowcursor と共有)
    cursor: usize,
}

impl LogState {
    /// `wrap` に加えて右ペインの実測サイズを引き継ぐ (GitState::new と同じ理由 —
    /// LOG に入った直後の 1 打鍵でカーソル追従が暴れないようにするため)
    pub fn new(root: &Path, wrap: bool, height: usize, width: usize) -> Self {
        let commits = git::log(root, 0, PAGE_SIZE);
        let exhausted = commits.len() < PAGE_SIZE;
        let mut state = Self {
            commits,
            list_state: ListState::default(),
            selected: 0,
            exhausted,
            viewport: {
                let mut vp = Viewport::new(wrap);
                vp.height = height;
                vp.width = width;
                vp
            },
            current: None,
            open_index: None,
            cursor: 0,
        };
        state
            .list_state
            .select((!state.commits.is_empty()).then_some(0));
        state
    }

    pub fn commits(&self) -> &[CommitSummary] {
        &self.commits
    }

    pub fn move_selection(&mut self, root: &Path, delta: isize) {
        if self.commits.is_empty() {
            return;
        }
        let last = self.commits.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
        self.list_state.select(Some(self.selected));
        // 末尾に到達した時だけ追加取得する。held-key の連打で毎回叩かないよう、
        // exhausted は取得件数が要求件数未満だった時点で確定させる (git.rs::log 参照)
        if !self.exhausted && self.selected + 1 == self.commits.len() {
            self.load_more(root);
        }
    }

    fn load_more(&mut self, root: &Path) {
        let more = git::log(root, self.commits.len(), PAGE_SIZE);
        if more.len() < PAGE_SIZE {
            self.exhausted = true;
        }
        self.commits.extend(more);
    }

    pub fn select_top(&mut self) {
        if self.commits.is_empty() {
            return;
        }
        self.selected = 0;
        self.list_state.select(Some(0));
    }

    /// G: 現在読み込み済みの末尾へ。全件を先読みすると巨大な履歴でブロックしうるため、
    /// 追加取得は他のスクロールと同じく 1 ページ分だけに留める
    pub fn select_bottom(&mut self, root: &Path) {
        if self.commits.is_empty() {
            return;
        }
        self.selected = self.commits.len() - 1;
        self.list_state.select(Some(self.selected));
        if !self.exhausted {
            self.load_more(root);
        }
    }

    /// Enter/l/クリック: 選択中コミットの diff を開く。j/k では呼ばない
    /// (GIT レーンのツリー同様、キーリピートで git show を連打しないため)
    pub fn open_selected(&mut self, root: &Path) {
        let Some(commit) = self.commits.get(self.selected) else {
            return;
        };
        let raw = git::show_commit(root, &commit.hash).unwrap_or_default();
        let (lines, hunks, gutter_width, max_width, boundaries) = gitlane::render_commit(&raw);
        self.current = Some(CommitDiff {
            lines,
            hunks,
            gutter_width,
            max_width,
            boundaries,
        });
        self.open_index = Some(self.selected);
        self.viewport.scroll = 0;
        self.cursor = 0;
        self.viewport.hscroll = 0;
    }

    pub fn open_index(&self) -> Option<usize> {
        self.open_index
    }

    pub fn title(&self) -> Option<String> {
        let idx = self.open_index?;
        let commit = self.commits.get(idx)?;
        Some(format!("{}  {}", commit.short, commit.subject))
    }

    pub fn lines(&self) -> &[Line<'static>] {
        match &self.current {
            Some(diff) => &diff.lines,
            None => &[],
        }
    }

    pub fn gutter_width(&self) -> usize {
        self.current.as_ref().map_or(0, |d| d.gutter_width)
    }

    pub fn line_count(&self) -> usize {
        self.lines().len()
    }

    /// #40: sticky header 用のファイル境界一覧。空ならコミットに複数ファイル diff が無い
    /// (diff 未オープン・0 ファイルの空コミットなど)
    pub fn boundaries(&self) -> &[(usize, String)] {
        self.current.as_ref().map_or(&[], |d| &d.boundaries)
    }

    /// sticky 行 1 行分の描画領域を確保すべきか。scroll に依らずコミット単位で固定なので、
    /// ここを scroll 依存にすると Ctrl+d/Ctrl+u のページ送り量が位置によってズレる
    pub fn has_file_boundary(&self) -> bool {
        !self.boundaries().is_empty()
    }

    /// scroll がまだ最初のファイルに届いていない (コミットメッセージ部分) 場合は None。
    /// 探索ロジックは GIT レーンのまとめ diff (GitState::sticky_label) と共有する
    /// (gitlane::sticky_label、#31 で複数ファイル diff の sticky header を GIT にも広げた際に切り出した)
    pub fn sticky_label(&self) -> Option<&str> {
        gitlane::sticky_label(self.boundaries(), self.viewport.scroll)
    }

    /// ホイール等の「画面を動かす」操作。カーソルは画面内へ引き戻して連れて動かす
    pub fn scroll_by(&mut self, delta: isize) {
        let last = self.line_count().saturating_sub(1);
        self.viewport.scroll_by(delta, last);
        let (count, wrapped, width) = self.cursor_metrics();
        self.cursor = rowcursor::clamp_cursor(&self.viewport, self.cursor, count, wrapped, |i| {
            self.rows_at(i, width)
        });
    }

    /// j/k・Ctrl+d/u: カーソルを動かし、画面はそれに追従させる
    pub fn move_cursor(&mut self, delta: isize) {
        let last = self.line_count().saturating_sub(1);
        self.cursor = (self.cursor as isize + delta).clamp(0, last as isize) as usize;
        self.ensure_cursor_visible();
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// クリックしたペイン内 row → カーソル (sticky header の 1 行は呼び出し側が差し引く)
    pub fn click_row(&mut self, row: usize) {
        let (count, wrapped, width) = self.cursor_metrics();
        let line = rowcursor::line_at_row(&self.viewport, row, count, wrapped, |i| {
            self.rows_at(i, width)
        });
        self.cursor = line;
        self.ensure_cursor_visible();
    }

    fn ensure_cursor_visible(&mut self) {
        let (count, wrapped, width) = self.cursor_metrics();
        let scroll = rowcursor::scroll_for(&self.viewport, self.cursor, count, wrapped, |i| {
            self.rows_at(i, width)
        });
        self.viewport.scroll = scroll;
    }

    fn cursor_metrics(&self) -> (usize, bool, usize) {
        let width = self
            .viewport
            .width
            .saturating_sub(self.gutter_width())
            .max(1);
        (self.line_count(), self.viewport.wrap, width)
    }

    fn rows_at(&self, i: usize, width: usize) -> usize {
        match self.lines().get(i) {
            Some(line) => text::wrap_rows(&line_body(line), width),
            None => 1,
        }
    }

    pub fn jump_to_top(&mut self) {
        self.cursor = 0;
        self.viewport.scroll = 0;
    }

    pub fn jump_to_bottom(&mut self) {
        self.cursor = self.line_count().saturating_sub(1);
        self.ensure_cursor_visible();
    }

    pub fn hscroll_by(&mut self, delta: isize) {
        let max = self
            .current
            .as_ref()
            .map_or(0, |d| d.max_width.saturating_sub(self.viewport.width / 2));
        self.viewport.hscroll_by(delta, max);
    }

    pub fn hscroll_reset(&mut self) {
        self.viewport.hscroll = 0;
    }

    /// n: 現在位置より後ろの最初の hunk header へ
    pub fn next_hunk(&mut self) {
        let Some(diff) = &self.current else {
            return;
        };
        if let Some(&target) = diff.hunks.iter().find(|&&i| i > self.cursor) {
            self.cursor = target;
            self.viewport.scroll = target;
        }
    }

    /// N: 現在位置より前の最後の hunk header へ
    pub fn prev_hunk(&mut self) {
        let Some(diff) = &self.current else {
            return;
        };
        if let Some(&target) = diff.hunks.iter().rev().find(|&&i| i < self.cursor) {
            self.cursor = target;
            self.viewport.scroll = target;
        }
    }
}
