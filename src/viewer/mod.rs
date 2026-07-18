mod content;
mod search;

pub use content::{Content, Open};
pub use search::SearchState;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ratatui::style::Color;
use syntect::highlighting::{Color as SyntectColor, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

use crate::git;

/// これを超えるファイルはハイライトせずプレーン表示する
const MAX_HIGHLIGHT_BYTES: usize = 10 * 1024 * 1024;
/// バイナリ判定で先頭から NUL バイトを探す範囲
const BINARY_SNIFF_BYTES: usize = 8192;
/// 履歴スタックの上限件数。vim の jumplist に倣い、超えたら古い方から捨てる
const HISTORY_LIMIT: usize = 50;

pub struct Viewer {
    syntax_set: SyntaxSet,
    theme: Theme,
    // ハイライト済み行のキャッシュ。ファイルを開き直しても再計算しない
    cache: HashMap<PathBuf, Rc<Content>>,
    pub current: Option<Open>,
    pub scroll: usize,
    // 描画時に ui 側が実測値を書き戻す。Ctrl+d/u の半ページ量の算出用
    pub viewport_height: usize,
    // 描画時に ui 側が実測値を書き戻す。hscroll の緩いクランプ算出用
    pub viewport_width: usize,
    // ファイルを跨いで維持する表示設定。true の間は draw_viewer が Paragraph::wrap を付ける
    pub wrap: bool,
    // wrap off 時のみ有効な水平スクロール量 (char 単位)。wrap on の間は 0 に固定する
    pub hscroll: usize,
    // ファイルごとではなく viewer に1つだけ持つ検索状態
    pub search: Option<SearchState>,
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
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let mut theme = ThemeSet::load_defaults()
            .themes
            .remove("base16-ocean.dark")
            .expect("base16-ocean.dark is bundled in syntect's default themes");
        tweak_comment_color(&mut theme);
        Self {
            syntax_set,
            theme,
            cache: HashMap::new(),
            current: None,
            scroll: 0,
            viewport_height: 0,
            viewport_width: 0,
            wrap: false,
            hscroll: 0,
            search: None,
            root: PathBuf::new(),
            history: Vec::new(),
            history_index: 0,
            last_scroll: HashMap::new(),
        }
    }

    pub fn background(&self) -> Color {
        self.theme
            .settings
            .background
            .map(|c| Color::Rgb(c.r, c.g, c.b))
            .unwrap_or(Color::Reset)
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
            self.last_scroll.insert(open.path.clone(), self.scroll);
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
                let loaded = Rc::new(self.load(path));
                self.cache.insert(path.to_path_buf(), Rc::clone(&loaded));
                loaded
            }
        };
        self.scroll = scroll;
        // ファイルを跨ぐたびに水平位置はリセットする (wrap は跨いで維持する設定なのでここでは触らない)
        self.hscroll = 0;
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
        let loaded = Rc::new(self.load(path));
        self.cache.insert(path.to_path_buf(), Rc::clone(&loaded));
        let changed_lines = git::changed_lines(&self.root, path);
        if let Some(open) = &mut self.current {
            open.content = loaded;
            open.changed_lines = changed_lines;
        }
        let last = self.line_count().saturating_sub(1);
        self.scroll = self.scroll.min(last);
        self.hscroll = 0;
        self.recompute_search();
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let last = self.line_count().saturating_sub(1) as isize;
        self.scroll = (self.scroll as isize + delta).clamp(0, last) as usize;
    }

    /// w: 折返しトグル。有効化した瞬間は水平スクロール位置の意味が失われるので 0 に戻す
    pub fn toggle_wrap(&mut self) {
        self.wrap = !self.wrap;
        if self.wrap {
            self.hscroll = 0;
        }
    }

    /// h/l 等の水平スクロール。wrap 中は no-op (呼び出し側の条件分岐と二重に守る)
    pub fn hscroll_by(&mut self, delta: isize) {
        if self.wrap {
            return;
        }
        let max = self.max_hscroll() as isize;
        self.hscroll = (self.hscroll as isize + delta).clamp(0, max) as usize;
    }

    /// 0: 水平スクロールを先頭に戻す
    pub fn hscroll_reset(&mut self) {
        self.hscroll = 0;
    }

    // 現在 viewport に見えている行の最大 char 幅から表示幅の半分を引いた値を上限にする、
    // 無限に右へ流れていかない程度の緩いクランプ (gutter 幅や罫線は考慮しない概算でよい)
    fn max_hscroll(&self) -> usize {
        let Some(open) = &self.current else {
            return 0;
        };
        let Content::Text { plain, .. } = open.content.as_ref() else {
            return 0;
        };
        let start = self.scroll.min(plain.len());
        let end = (self.scroll + self.viewport_height.max(1)).min(plain.len());
        let max_width = plain[start..end]
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0);
        max_width.saturating_sub(self.viewport_width / 2)
    }

    /// gg: ファイル先頭へ
    pub fn jump_to_top(&mut self) {
        self.scroll = 0;
    }

    /// G: 最終行が viewport の下端に来る位置へ。ファイルが viewport より短ければ先頭のまま
    pub fn jump_to_bottom(&mut self) {
        let total = self.line_count();
        let last = total.saturating_sub(1);
        let bottom = total.saturating_sub(self.viewport_height);
        self.scroll = bottom.min(last);
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
                Content::Text { lines, .. } => lines.len(),
                _ => 0,
            },
            None => 0,
        }
    }

    pub fn is_text(&self) -> bool {
        matches!(
            self.current.as_ref().map(|open| open.content.as_ref()),
            Some(Content::Text { .. })
        )
    }
}

fn tweak_comment_color(theme: &mut Theme) {
    for item in &mut theme.scopes {
        // コメント系スコープだけ少し明るくして背景への同化を防ぐ
        if !format!("{:?}", item.scope)
            .to_ascii_lowercase()
            .contains("comment")
        {
            continue;
        }
        let Some(fg) = item.style.foreground else {
            continue;
        };
        const ADJUSTMENT: u8 = 56;
        item.style.foreground = Some(SyntectColor {
            r: fg.r.saturating_add(ADJUSTMENT),
            g: fg.g.saturating_add(ADJUSTMENT),
            b: fg.b.saturating_add(ADJUSTMENT),
            a: fg.a,
        });
    }
}
