mod content;
mod highlight;
mod render;
mod search;
mod selection;
pub mod view;
mod viewport;

pub use content::{Content, Open};
pub use highlight::Highlighter;
pub use render::{HighlightCache, LineSource};
pub use search::SearchState;
pub(crate) use search::{Match, search_matches};
use selection::Point;
pub use selection::Selection;
pub use viewport::Viewport;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ratatui::style::Color;

use crate::git;
use crate::text;

/// これを超えるファイルはハイライトせずプレーン表示する
const MAX_HIGHLIGHT_BYTES: usize = 10 * 1024 * 1024;
/// バイナリ判定で先頭から NUL バイトを探す範囲
const BINARY_SNIFF_BYTES: usize = 8192;
/// 履歴スタックの上限件数。vim の jumplist に倣い、超えたら古い方から捨てる
const HISTORY_LIMIT: usize = 50;

/// syntect が同梱するデフォルトテーマの一覧。設定画面のテーマ切替はこの中を巡回する
pub const THEME_NAMES: [&str; 7] = [
    "base16-ocean.dark",
    "base16-eighties.dark",
    "base16-mocha.dark",
    "base16-ocean.light",
    "InspiredGitHub",
    "Solarized (dark)",
    "Solarized (light)",
];

pub struct Viewer {
    /// シンタックス定義とテーマの置き場。編集 (EditState) も描画時にこれだけを借りる
    pub highlighter: Highlighter,
    /// スクロール・折返し状態。閲覧と編集で同じ実体を共有する
    pub viewport: Viewport,
    /// 開いているファイルの可視範囲だけを組み立てる描画キャッシュ。
    /// 描画時に ui が直接触る (ui→app の書き戻しと同じく、他フィールドと独立に借りるため)
    pub render: HighlightCache,
    // 読み込み済みテキストのキャッシュ。ハイライトは焼き込まれていないので、
    // テーマを変えてもここは捨てなくてよい
    cache: HashMap<PathBuf, Rc<Content>>,
    pub current: Option<Open>,
    // ファイルごとではなく viewer に1つだけ持つ検索状態
    pub search: Option<SearchState>,
    /// 範囲選択 (マウスのドラッグ / v)。検索と同じく viewer に 1 つだけ持ち、
    /// ファイルを開き直した時点で捨てる (別の文書の桁を指したままにしないため)
    pub selection: Option<Selection>,
    // open() の度に更新される root。reload() は path しか受け取らないので、
    // changed_lines の再取得に使う root をここに保持しておく
    root: PathBuf,
    // 開いたファイルの履歴 (jumplist)。history[history_index] が現在位置。
    // 通常の open() は history_index より後ろ (進む方向の履歴) を切り捨てて末尾に積む。
    // history が空の間は history_index は未使用 (0 のまま)
    history: Vec<PathBuf>,
    history_index: usize,
    // ファイルごとの最後の scroll 位置。Ctrl+o/i で履歴を移動した時だけ復元に使う
    // (通常の open では常に先頭から表示する既存挙動を変えないため)
    last_scroll: HashMap<PathBuf, usize>,
}

impl Viewer {
    pub fn new() -> Self {
        Self {
            highlighter: Highlighter::new(),
            viewport: Viewport::new(false),
            render: HighlightCache::new(),
            cache: HashMap::new(),
            current: None,
            search: None,
            selection: None,
            root: PathBuf::new(),
            history: Vec::new(),
            history_index: 0,
            last_scroll: HashMap::new(),
        }
    }

    pub fn background(&self) -> Color {
        self.highlighter.background()
    }

    pub fn theme_name(&self) -> &str {
        self.highlighter.theme_name()
    }

    /// テーマ切替。Content にはハイライトが焼き込まれていないので、捨てるのは
    /// 描画キャッシュ (次の描画で可視範囲だけ組み直される) だけでよい
    pub fn set_theme(&mut self, name: &str) -> bool {
        if !self.highlighter.set_theme(name) {
            return false;
        }
        self.render.invalidate_all();
        true
    }

    pub fn open(&mut self, path: &Path, root: &Path) {
        if let Some(open) = &self.current
            && open.path == path
        {
            return;
        }
        // 通常の open (ツリー/ファインダー/クリック経由) は既存挙動どおり常に先頭から表示する。
        // scroll 位置だけは離れる前に記録しておき、後で Ctrl+o/i で戻ってきた時に復元する
        self.record_scroll();
        self.push_history(path);
        self.set_current(path, root, 0);
    }

    /// Ctrl+o: 履歴を1つ戻る。先頭にいる場合は no-op
    pub fn back(&mut self) {
        if self.history_index == 0 {
            return;
        }
        self.record_scroll();
        self.history_index -= 1;
        self.open_from_history();
    }

    /// Ctrl+i: 履歴を1つ進む。末尾にいる場合は no-op
    pub fn forward(&mut self) {
        if self.history.is_empty() || self.history_index + 1 >= self.history.len() {
            return;
        }
        self.record_scroll();
        self.history_index += 1;
        self.open_from_history();
    }

    // 現在開いているファイルの scroll 位置を記録する。ファイルを離れる直前 (open/back/forward) に呼ぶ
    fn record_scroll(&mut self) {
        if let Some(open) = &self.current {
            self.last_scroll
                .insert(open.path.clone(), self.viewport.scroll);
        }
    }

    // 履歴スタックに新規ファイルを積む。ブラウザ履歴と同じく、現在位置より後ろ (進む方向) は
    // 切り捨ててから末尾に追加する。呼び出し元 (open) で「同一ファイルの連続 open」は
    // 早期 return 済みなので、ここでは単純に追加してよい
    fn push_history(&mut self, path: &Path) {
        if !self.history.is_empty() {
            self.history.truncate(self.history_index + 1);
        }
        self.history.push(path.to_path_buf());
        if self.history.len() > HISTORY_LIMIT {
            self.history.remove(0);
        }
        self.history_index = self.history.len() - 1;
    }

    // history[history_index] を、記録済みの scroll 位置を復元しつつ開く
    fn open_from_history(&mut self) {
        let path = self.history[self.history_index].clone();
        let root = self.root.clone();
        let scroll = self.last_scroll.get(&path).copied().unwrap_or(0);
        self.set_current(&path, &root, scroll);
    }

    // open/back/forward 共通の「ファイルを実際に表示状態にする」処理
    fn set_current(&mut self, path: &Path, root: &Path, scroll: usize) {
        self.root = root.to_path_buf();
        let title = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string();
        let content = match self.cache.get(path) {
            Some(cached) => Rc::clone(cached),
            None => {
                let loaded = Rc::new(content::load(path));
                self.cache.insert(path.to_path_buf(), Rc::clone(&loaded));
                loaded
            }
        };
        self.render.reset(path, plain_only(&content));
        self.selection = None;
        self.viewport.scroll = scroll;
        // ファイルを跨ぐたびに水平位置はリセットする (wrap は跨いで維持する設定なのでここでは触らない)
        self.viewport.hscroll = 0;
        self.current = Some(Open {
            title,
            path: path.to_path_buf(),
            content,
            changed_lines: git::changed_lines(root, path),
        });
        self.recompute_search();
    }

    /// 外部変更を検知したファイルを読み直す。current が同じファイルなら
    /// 差し替え、スクロール位置は維持しつつ新しい行数にクランプする。
    pub fn reload(&mut self, path: &Path) {
        self.cache.remove(path);
        let is_current = self.current.as_ref().is_some_and(|open| open.path == path);
        if !is_current {
            return;
        }
        let loaded = Rc::new(content::load(path));
        self.cache.insert(path.to_path_buf(), Rc::clone(&loaded));
        self.render.reset(path, plain_only(&loaded));
        // 行が入れ替わった後の桁を指したままにしない (外部から書き換えられたファイル)
        self.selection = None;
        let changed_lines = git::changed_lines(&self.root, path);
        if let Some(open) = &mut self.current {
            open.content = loaded;
            open.changed_lines = changed_lines;
        }
        let last = self.line_count().saturating_sub(1);
        self.viewport.scroll = self.viewport.scroll.min(last);
        self.viewport.hscroll = 0;
        self.recompute_search();
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let last = self.line_count().saturating_sub(1);
        self.viewport.scroll_by(delta, last);
    }

    /// h/l 等の水平スクロール。クランプ上限だけ Content から算出して Viewport に渡す
    pub fn hscroll_by(&mut self, delta: isize) {
        let max = self.max_hscroll();
        self.viewport.hscroll_by(delta, max);
    }

    /// 0: 水平スクロールを先頭に戻す
    pub fn hscroll_reset(&mut self) {
        self.viewport.hscroll = 0;
    }

    // 現在 viewport に見えている行の最大 char 幅から表示幅の半分を引いた値を上限にする、
    // 無限に右へ流れていかない程度の緩いクランプ (gutter 幅や罫線は考慮しない概算でよい)
    fn max_hscroll(&self) -> usize {
        let Some(open) = &self.current else {
            return 0;
        };
        let Content::Text(doc) = open.content.as_ref() else {
            return 0;
        };
        let plain = &doc.plain;
        let start = self.viewport.scroll.min(plain.len());
        let end = (self.viewport.scroll + self.viewport.height.max(1)).min(plain.len());
        let max_width = plain[start..end]
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0);
        max_width.saturating_sub(self.viewport.width / 2)
    }

    /// gg: ファイル先頭へ
    pub fn jump_to_top(&mut self) {
        self.viewport.scroll = 0;
    }

    /// G: 最終行が viewport の下端に来る位置へ。ファイルが viewport より短ければ先頭のまま
    pub fn jump_to_bottom(&mut self) {
        let total = self.line_count();
        let last = total.saturating_sub(1);
        let bottom = total.saturating_sub(self.viewport.height);
        self.viewport.scroll = bottom.min(last);
    }

    /// :N の行ジャンプ。1-origin。範囲外は最終行にクランプ。0 は no-op (呼び出し側でも弾いているが念のため)
    pub fn goto_line(&mut self, line_no: usize) {
        if line_no == 0 {
            return;
        }
        let last = self.line_count().saturating_sub(1);
        let target = (line_no - 1).min(last);
        self.center_on(target);
    }

    pub fn line_count(&self) -> usize {
        match &self.current {
            Some(open) => match open.content.as_ref() {
                Content::Text(doc) => doc.line_count(),
                _ => 0,
            },
            None => 0,
        }
    }

    /// 開いているファイルを閉じて右ペインを空にする。ブランチ切替で開いていたファイルが
    /// 切替先に存在しなくなった場合に使う (cache・履歴には触れず current だけ落とす)
    pub fn close(&mut self) {
        self.current = None;
        self.selection = None;
    }

    // 表示中のテキスト文書。バイナリ・エラー・未選択では None
    fn text_doc(&self) -> Option<&content::TextDoc> {
        match self.current.as_ref()?.content.as_ref() {
            Content::Text(doc) => Some(doc),
            _ => None,
        }
    }

    // ペイン内側の (row, col) を plain 座標へ。行末より右・最終行より下を指した場合も
    // そのまま返す (テキストを取り出す側が行の長さでクランプする)
    fn point_at(&self, row: usize, col: usize) -> Option<Point> {
        let doc = self.text_doc()?;
        let gutter = text::gutter_width(doc.line_count());
        // plain はタブ展開済みなので、表示桁 = char 数がそのまま成り立つ
        let (line, display) = self
            .viewport
            .locate(row, col, gutter, doc.line_count(), |i| {
                doc.plain[i].chars().count()
            });
        Some(Point { line, col: display })
    }

    /// マウスの押下: その位置を掴み直す (前の選択は捨てる)。押しただけの空選択は
    /// 何もハイライトせず、`y` も「選択が無い」として扱われる
    pub fn begin_drag(&mut self, row: usize, col: usize) {
        self.selection = self
            .point_at(row, col)
            .map(|at| Selection::new(at, false, true));
    }

    /// ドラッグ中の伸縮。押下を見ていない (dragging でない) 間は何もしない
    pub fn drag_to(&mut self, row: usize, col: usize) {
        if !self.dragging() {
            return;
        }
        if let Some(at) = self.point_at(row, col)
            && let Some(sel) = &mut self.selection
        {
            sel.set_head(at);
        }
    }

    pub fn end_drag(&mut self) {
        if let Some(sel) = &mut self.selection {
            sel.dragging = false;
        }
    }

    pub fn dragging(&self) -> bool {
        self.selection.as_ref().is_some_and(|s| s.dragging)
    }

    /// v: 画面上端の行から行単位選択を始める。既に行単位選択中なら解除 (トグル)
    pub fn toggle_line_selection(&mut self) {
        if self.line_selecting() {
            self.selection = None;
            return;
        }
        if self.text_doc().is_none() {
            return;
        }
        let line = self
            .viewport
            .scroll
            .min(self.line_count().saturating_sub(1));
        self.selection = Some(Selection::new(Point { line, col: 0 }, true, false));
    }

    /// 行単位選択 (v) の最中か。この間だけ j/k 等が選択の伸縮に化ける
    pub fn line_selecting(&self) -> bool {
        self.selection.as_ref().is_some_and(|s| s.linewise)
    }

    /// 行単位選択を delta 行ぶん伸縮し、伸ばした先が見えるまでスクロールする
    pub fn extend_line_selection(&mut self, delta: isize) {
        let last = self.line_count().saturating_sub(1);
        let Some(sel) = &mut self.selection else {
            return;
        };
        let line = (sel.head_line() as isize + delta).clamp(0, last as isize) as usize;
        sel.set_head(Point { line, col: 0 });
        self.viewport.ensure_row_visible(line);
    }

    /// 行単位選択の伸ばす側を指定行へ飛ばす (gg / G)
    pub fn move_line_selection_to(&mut self, line: usize) {
        let last = self.line_count().saturating_sub(1);
        let line = line.min(last);
        if let Some(sel) = &mut self.selection {
            sel.set_head(Point { line, col: 0 });
        }
        self.viewport.ensure_row_visible(line);
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// ステータスバー表示用の選択行数。空選択・選択なしでは None
    pub fn selected_line_count(&self) -> Option<usize> {
        let sel = self.selection.as_ref()?;
        (!sel.is_empty()).then(|| sel.line_count())
    }

    /// 選択範囲のテキスト。空選択・非テキストでは None
    pub fn selection_text(&self) -> Option<String> {
        let sel = self.selection.as_ref()?;
        if sel.is_empty() {
            return None;
        }
        Some(sel.text(self.text_doc()?.raw()))
    }

    /// 開いているファイル全体のテキスト (末尾改行の有無も元ファイルどおり)
    pub fn all_text(&self) -> Option<String> {
        let doc = self.text_doc()?;
        let mut out = doc.raw().join("\n");
        if doc.has_trailing_newline() {
            out.push('\n');
        }
        Some(out)
    }

    pub fn is_text(&self) -> bool {
        matches!(
            self.current.as_ref().map(|open| open.content.as_ref()),
            Some(Content::Text(_))
        )
    }
}

// 巨大ファイルは syntect を通さない。判定自体は load が済ませているので、
// ここは HighlightCache へ渡すための取り出しだけ
fn plain_only(content: &Content) -> bool {
    match content {
        Content::Text(doc) => doc.plain_only,
        _ => false,
    }
}
