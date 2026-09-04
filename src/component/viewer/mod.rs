mod content;
mod highlight;
mod render;
pub(crate) mod rowcursor;
mod search;
mod selection;
pub mod view;
mod viewport;

pub use content::{Content, Open};
pub use highlight::Highlighter;
pub use render::{HighlightCache, LineSource, Touched};
pub use search::SearchState;
pub(crate) use search::{Match, line_matches, search_matches};
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

/// 読み込み済みテキストの cache に残す総量の上限。長く使うほど開いたファイルぶん膨らみ
/// 続けていたので、これを超えたら使っていない順に捨てる (今開いているものは捨てない)
const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;

struct Cached {
    content: Rc<Content>,
    mtime: Option<std::time::SystemTime>,
    size: u64,
    bytes: usize,
}

fn stat(path: &Path) -> (Option<std::time::SystemTime>, u64) {
    match std::fs::metadata(path) {
        Ok(meta) => (meta.modified().ok(), meta.len()),
        Err(_) => (None, 0),
    }
}

pub struct Viewer {
    /// シンタックス定義とテーマの置き場。編集 (EditState) も描画時にこれだけを借りる
    pub highlighter: Highlighter,
    /// スクロール・折返し状態。閲覧と編集で同じ実体を共有する
    pub viewport: Viewport,
    /// 開いているファイルの可視範囲だけを組み立てる描画キャッシュ。
    /// 描画時に ui が直接触る (ui→app の書き戻しと同じく、他フィールドと独立に借りるため)
    pub render: HighlightCache,
    // 読み込み済みテキストのキャッシュ。ハイライトは焼き込まれていないので、
    // テーマを変えてもここは捨てなくてよい。上限 (MAX_CACHE_BYTES) を超えたら古い順に捨て、
    // 使う時は stat で (mtime, size) を照合する — 開いていない間に外から書き換えられた
    // ファイルは watcher の reload (current だけ) が届かないため、ここで見抜くしかない
    cache: HashMap<PathBuf, Cached>,
    /// cache の使用順 (末尾が最新)。数十件なので Vec で足りる
    cache_order: Vec<PathBuf>,
    cache_bytes: usize,
    pub current: Option<Open>,
    // ファイルごとではなく viewer に1つだけ持つ検索状態
    pub search: Option<SearchState>,
    /// 範囲選択 (マウスのドラッグ / v)。検索と同じく viewer に 1 つだけ持ち、
    /// ファイルを開き直した時点で捨てる (別の文書の桁を指したままにしないため)
    pub selection: Option<Selection>,
    /// 行カーソル。閲覧は読むだけのペインだが、`v` の選択開始点・`e` で編集に移った時の
    /// 位置・「今どこを読んでいるか」の全てが「上端に見えている行」という暗黙の基準に
    /// 乗っていたので、明示的な 1 行として持たせる (GIT レーンの diff と同じ考え方)
    cursor: usize,
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
            cache_order: Vec::new(),
            cache_bytes: 0,
            current: None,
            search: None,
            selection: None,
            cursor: 0,
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
        let content = self.cached_or_load(path);
        self.render.reset(path, plain_only(&content));
        self.selection = None;
        self.viewport.scroll = scroll;
        // 履歴で戻った時は記録した scroll の先頭行から読み直す (通常の open は scroll = 0)
        self.cursor = scroll;
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

    // cache にあり (mtime, size) が変わっていなければそれを、無ければ読んで cache に入れたものを返す
    fn cached_or_load(&mut self, path: &Path) -> Rc<Content> {
        let (mtime, size) = stat(path);
        if let Some(cached) = self.cache.get(path)
            && cached.mtime == mtime
            && cached.size == size
        {
            let content = Rc::clone(&cached.content);
            self.touch_order(path);
            return content;
        }
        self.load_into_cache(path, mtime, size)
    }

    fn load_into_cache(
        &mut self,
        path: &Path,
        mtime: Option<std::time::SystemTime>,
        size: u64,
    ) -> Rc<Content> {
        self.forget(path);
        let content = Rc::new(content::load(path));
        let bytes = content.approx_bytes();
        self.cache.insert(
            path.to_path_buf(),
            Cached {
                content: Rc::clone(&content),
                mtime,
                size,
                bytes,
            },
        );
        self.cache_bytes += bytes;
        self.cache_order.push(path.to_path_buf());
        self.evict(path);
        content
    }

    fn touch_order(&mut self, path: &Path) {
        if let Some(i) = self.cache_order.iter().position(|p| p == path) {
            let p = self.cache_order.remove(i);
            self.cache_order.push(p);
        }
    }

    // 上限を超えたぶんを古い順に捨てる。keep (今開こうとしているもの) だけは残す
    fn evict(&mut self, keep: &Path) {
        while self.cache_bytes > MAX_CACHE_BYTES {
            let Some(i) = self.cache_order.iter().position(|p| p != keep) else {
                return;
            };
            let victim = self.cache_order.remove(i);
            self.forget(&victim);
        }
    }

    /// cache から落とす (今開いているものには触れない)。開いていないファイルの外部変更で呼び、
    /// 変わったファイルの古い内容を持ち続けない。呼ばれなくても次に開く時の stat で見抜ける
    pub fn forget(&mut self, path: &Path) {
        if let Some(old) = self.cache.remove(path) {
            self.cache_bytes -= old.bytes;
            self.cache_order.retain(|p| p != path);
        }
    }

    /// 外部変更を検知したファイルを読み直す。current が同じファイルなら
    /// 差し替え、スクロール位置は維持しつつ新しい行数にクランプする。
    pub fn reload(&mut self, path: &Path) {
        let is_current = self.current.as_ref().is_some_and(|open| open.path == path);
        if !is_current {
            self.forget(path);
            return;
        }
        let (mtime, size) = stat(path);
        let loaded = self.load_into_cache(path, mtime, size);
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
        self.cursor = self.cursor.min(last);
        self.viewport.hscroll = 0;
        self.recompute_search();
    }

    /// ホイール等の「画面を動かす」操作。カーソルは画面内へ引き戻して連れて動かす
    /// (置き去りにすると `v` や `e` の起点が画面外に消える)
    pub fn scroll_by(&mut self, delta: isize) {
        let last = self.line_count().saturating_sub(1);
        self.viewport.scroll_by(delta, last);
        self.clamp_cursor_into_view();
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

    /// クリックした行へカーソルを移す (範囲選択の起点と兼用)
    pub(super) fn set_cursor(&mut self, line: usize) {
        self.cursor = line.min(self.line_count().saturating_sub(1));
        self.ensure_cursor_visible();
    }

    // 行カーソルの追従。折返しを跨ぐ計算は rowcursor に寄せてある (GIT/LOG/PR と共有)
    pub(super) fn ensure_cursor_visible(&mut self) {
        let (count, wrapped, width) = self.cursor_metrics();
        let scroll = rowcursor::scroll_for(&self.viewport, self.cursor, count, wrapped, |i| {
            self.rows_at(i, width)
        });
        self.viewport.scroll = scroll;
    }

    fn clamp_cursor_into_view(&mut self) {
        let (count, wrapped, width) = self.cursor_metrics();
        let cursor = rowcursor::clamp_cursor(&self.viewport, self.cursor, count, wrapped, |i| {
            self.rows_at(i, width)
        });
        self.cursor = cursor;
    }

    fn cursor_metrics(&self) -> (usize, bool, usize) {
        let count = self.line_count();
        let gutter = text::gutter_width(count);
        let width = self.viewport.width.saturating_sub(gutter).max(1);
        (count, self.viewport.wrap, width)
    }

    // 論理行 i が占める視覚行数。plain はタブ展開済みなので描画と同じ文字列で数えられる
    fn rows_at(&self, i: usize, width: usize) -> usize {
        match self.text_doc().and_then(|doc| doc.plain.get(i)) {
            Some(line) => text::wrap_rows(line, width),
            None => 1,
        }
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
        self.cursor = 0;
        self.viewport.scroll = 0;
    }

    /// G: 最終行へ。カーソルが下端に来るまでスクロールする
    pub fn jump_to_bottom(&mut self) {
        self.cursor = self.line_count().saturating_sub(1);
        self.ensure_cursor_visible();
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
        // plain はタブ展開済みなので、返る表示桁がそのまま plain の char 座標になる
        let (line, display) = self
            .viewport
            .locate(row, col, gutter, doc.line_count(), |i| {
                doc.plain[i].as_str()
            });
        Some(Point { line, col: display })
    }

    /// マウスの押下: その位置を掴み直す (前の選択は捨てる)。押しただけの空選択は
    /// 何もハイライトせず、`y` も「選択が無い」として扱われる
    pub fn begin_drag(&mut self, row: usize, col: usize) {
        let at = self.point_at(row, col);
        // 押しただけ (ドラッグしない) でもカーソルは移す — クリックが「ここを見ている」の
        // 表明になるようにするため
        if let Some(at) = at {
            self.set_cursor(at.line);
        }
        self.selection = at.map(|at| Selection::new(at, false, true));
    }

    /// ドラッグ中の伸縮。押下を見ていない (dragging でない) 間は何もしない
    pub fn drag_to(&mut self, row: usize, col: usize) {
        if !self.dragging() {
            return;
        }
        if let Some(at) = self.point_at(row, col) {
            self.cursor = at.line.min(self.line_count().saturating_sub(1));
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
        let line = self.cursor.min(self.line_count().saturating_sub(1));
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
        // 伸ばしている先がカーソル (vim の visual mode と同じく head 側が動く)
        self.cursor = line;
        self.ensure_cursor_visible();
    }

    /// 行単位選択の伸ばす側を指定行へ飛ばす (gg / G)
    pub fn move_line_selection_to(&mut self, line: usize) {
        let last = self.line_count().saturating_sub(1);
        let line = line.min(last);
        if let Some(sel) = &mut self.selection {
            sel.set_head(Point { line, col: 0 });
        }
        self.cursor = line;
        self.ensure_cursor_visible();
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

#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::fs;

    fn dir(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("fv-viewer-cache-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    // 開いていない間に外から書き換えられたファイルは、watcher の reload (current だけ) が
    // 届かない。開き直した時に cache の古い内容を出さないこと
    #[test]
    fn reopening_a_file_changed_while_another_was_open_shows_the_new_content() {
        let root = dir("stale");
        let a = root.join("a.txt");
        let b = root.join("b.txt");
        fs::write(&a, "old\n").unwrap();
        fs::write(&b, "b\n").unwrap();
        let mut viewer = Viewer::new();
        viewer.open(&a, &root);
        viewer.open(&b, &root);
        // mtime の粒度に依らず size を変える
        fs::write(&a, "new content\n").unwrap();
        viewer.open(&a, &root);
        let Content::Text(doc) = viewer.current.as_ref().unwrap().content.as_ref() else {
            panic!("text");
        };
        assert_eq!(doc.plain, ["new content"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cache_evicts_oldest_files_beyond_the_byte_limit_but_keeps_the_open_one() {
        let root = dir("evict");
        let big = "x".repeat(MAX_CACHE_BYTES / 4);
        let paths: Vec<PathBuf> = (0..4).map(|i| root.join(format!("{i}.txt"))).collect();
        for p in &paths {
            fs::write(p, &big).unwrap();
        }
        let mut viewer = Viewer::new();
        for p in &paths {
            viewer.open(p, &root);
        }
        assert!(viewer.cache_bytes <= MAX_CACHE_BYTES);
        assert!(viewer.cache.contains_key(&paths[3]));
        assert!(!viewer.cache.contains_key(&paths[0]));
        // 外部変更の通知で開いていないものは落ち、開いているものは残る
        viewer.forget(&paths[2]);
        assert!(!viewer.cache.contains_key(&paths[2]));
        assert_eq!(
            viewer.cache_bytes,
            viewer.cache.values().map(|c| c.bytes).sum::<usize>()
        );
        let _ = fs::remove_dir_all(root);
    }
}
