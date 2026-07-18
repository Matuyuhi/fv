use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use crate::git;

/// これを超えるファイルはハイライトせずプレーン表示する
const MAX_HIGHLIGHT_BYTES: usize = 10 * 1024 * 1024;
/// バイナリ判定で先頭から NUL バイトを探す範囲
const BINARY_SNIFF_BYTES: usize = 8192;
/// 履歴スタックの上限件数。vim の jumplist に倣い、超えたら古い方から捨てる
const HISTORY_LIMIT: usize = 50;

pub enum Content {
    // plain は normalize 済み (タブ展開後) の行文字列。lines の span と桁位置が一致するので、
    // 検索マッチの char 列インデックスをそのままハイライト適用に使い回せる
    Text {
        lines: Vec<Line<'static>>,
        plain: Vec<String>,
    },
    Binary,
    Error(String),
}

pub struct Open {
    pub title: String,
    pub path: PathBuf,
    pub content: Rc<Content>,
    // 変更行番号 (1-origin)。git 情報が取れない場合は None のままガター表示を素通しする
    pub changed_lines: Option<HashSet<usize>>,
}

/// 1件のマッチ位置。列は plain の char 単位インデックス (gutter は含まない)
pub struct Match {
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
}

pub struct SearchState {
    pub query: String,
    pub matches: Vec<Match>,
    // Enter で確定した後にだけ Some。n/N で動かす現在位置
    pub current: Option<usize>,
}

pub struct Viewer {
    syntax_set: SyntaxSet,
    theme: Theme,
    // ハイライト済み行のキャッシュ。ファイルを開き直しても再計算しない
    cache: HashMap<PathBuf, Rc<Content>>,
    pub current: Option<Open>,
    pub scroll: usize,
    // 描画時に ui 側が実測値を書き戻す。Ctrl+d/u の半ページ量の算出用
    pub viewport_height: usize,
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
        let theme = ThemeSet::load_defaults()
            .themes
            .remove("base16-ocean.dark")
            .expect("base16-ocean.dark is bundled in syntect's default themes");
        Self {
            syntax_set,
            theme,
            cache: HashMap::new(),
            current: None,
            scroll: 0,
            viewport_height: 0,
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
        if let Some(open) = &self.current {
            if open.path == path {
                return;
            }
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
        let title = path.strip_prefix(root).unwrap_or(path).display().to_string();
        let content = match self.cache.get(path) {
            Some(cached) => Rc::clone(cached),
            None => {
                let loaded = Rc::new(self.load(path));
                self.cache.insert(path.to_path_buf(), Rc::clone(&loaded));
                loaded
            }
        };
        self.scroll = scroll;
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
        self.recompute_search();
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let last = self.line_count().saturating_sub(1) as isize;
        self.scroll = (self.scroll as isize + delta).clamp(0, last) as usize;
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

    /// Search 入力中のライブプレビュー。マッチを再計算するだけでジャンプはしない
    pub fn update_search(&mut self, query: &str) {
        if query.is_empty() {
            self.search = None;
            return;
        }
        let matches = self.compute_matches(query);
        self.search = Some(SearchState {
            query: query.to_string(),
            matches,
            current: None,
        });
    }

    /// Enter で確定。現在のスクロール位置以降の最初のマッチへジャンプ (なければ先頭へ wrap)
    pub fn confirm_search(&mut self) {
        let Some(search) = &self.search else {
            return;
        };
        if search.matches.is_empty() {
            return;
        }
        let scroll = self.scroll;
        let idx = search
            .matches
            .iter()
            .position(|m| m.line >= scroll)
            .unwrap_or(0);
        let line = search.matches[idx].line;
        if let Some(search) = &mut self.search {
            search.current = Some(idx);
        }
        self.center_on(line);
    }

    pub fn cancel_search(&mut self) {
        self.search = None;
    }

    pub fn next_match(&mut self) {
        self.step_match(1);
    }

    pub fn prev_match(&mut self) {
        self.step_match(-1);
    }

    fn step_match(&mut self, delta: isize) {
        let Some(search) = &self.search else {
            return;
        };
        if search.matches.is_empty() {
            return;
        }
        let Some(current) = search.current else {
            return;
        };
        let len = search.matches.len() as isize;
        let next = (current as isize + delta).rem_euclid(len) as usize;
        let line = search.matches[next].line;
        if let Some(search) = &mut self.search {
            search.current = Some(next);
        }
        self.center_on(line);
    }

    // マッチ行が viewport の中央付近に来るようスクロールする
    fn center_on(&mut self, line: usize) {
        let last = self.line_count().saturating_sub(1);
        let half = self.viewport_height / 2;
        self.scroll = line.saturating_sub(half).min(last);
    }

    fn compute_matches(&self, query: &str) -> Vec<Match> {
        let Some(open) = &self.current else {
            return Vec::new();
        };
        let Content::Text { plain, .. } = open.content.as_ref() else {
            return Vec::new();
        };
        search_matches(plain, query)
    }

    // ファイルを開き直した/reload した際、同じクエリでマッチを再計算する。
    // 確定済みだった場合は現在位置を新しいマッチ数に合わせてクランプする
    fn recompute_search(&mut self) {
        let Some(query) = self.search.as_ref().map(|s| s.query.clone()) else {
            return;
        };
        let matches = self.compute_matches(&query);
        if let Some(search) = &mut self.search {
            let current = search
                .current
                .map(|idx| idx.min(matches.len().saturating_sub(1)));
            search.current = if matches.is_empty() { None } else { current };
            search.matches = matches;
        }
    }

    fn load(&self, path: &Path) -> Content {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => return Content::Error(format!("failed to read: {e}")),
        };
        let sniff = &bytes[..bytes.len().min(BINARY_SNIFF_BYTES)];
        if sniff.contains(&0) {
            return Content::Binary;
        }
        let text = String::from_utf8_lossy(&bytes);
        let gutter_width = text.lines().count().max(1).to_string().len();
        let lines = if bytes.len() > MAX_HIGHLIGHT_BYTES {
            plain_lines(&text, gutter_width)
        } else {
            self.highlight_lines(path, &text, gutter_width)
        };
        let plain = plain_text_lines(&text);
        Content::Text { lines, plain }
    }

    fn highlight_lines(&self, path: &Path, text: &str, gutter_width: usize) -> Vec<Line<'static>> {
        let syntax = self.find_syntax(path, text);
        let mut highlighter = HighlightLines::new(syntax, &self.theme);
        let mut lines = Vec::new();
        for (i, raw) in LinesWithEndings::from(text).enumerate() {
            let mut spans = vec![gutter_span(i + 1, gutter_width)];
            match highlighter.highlight_line(raw, &self.syntax_set) {
                Ok(ranges) => {
                    for (style, segment) in ranges {
                        let segment = normalize(segment);
                        if segment.is_empty() {
                            continue;
                        }
                        spans.push(Span::styled(segment, convert_style(style)));
                    }
                }
                // 文法定義とファイル内容の組み合わせによってはパースが失敗しうる。
                // その行だけ無色で出し、表示自体は継続する
                Err(_) => spans.push(Span::raw(normalize(raw))),
            }
            lines.push(Line::from(spans));
        }
        if lines.is_empty() {
            lines.push(Line::from(gutter_span(1, gutter_width)));
        }
        lines
    }

    fn find_syntax(&self, path: &Path, text: &str) -> &SyntaxReference {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if let Some(syntax) = self.syntax_set.find_syntax_by_extension(ext) {
                return syntax;
            }
        }
        // Makefile 等、拡張子なしのファイル名そのものが文法定義に登録されている
        if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
            if let Some(syntax) = self.syntax_set.find_syntax_by_extension(file_name) {
                return syntax;
            }
        }
        let first_line = text.lines().next().unwrap_or("");
        self.syntax_set
            .find_syntax_by_first_line(first_line)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text())
    }
}

fn plain_lines(text: &str, gutter_width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = text
        .lines()
        .enumerate()
        .map(|(i, raw)| Line::from(vec![gutter_span(i + 1, gutter_width), Span::raw(normalize(raw))]))
        .collect();
    if lines.is_empty() {
        lines.push(Line::from(gutter_span(1, gutter_width)));
    }
    lines
}

// 改行を落とし、端末で幅が不定になるタブをスペースに展開する
fn normalize(segment: &str) -> String {
    segment.trim_end_matches(['\n', '\r']).replace('\t', "    ")
}

// lines/plain_lines/highlight_lines と同じ行分割・タブ展開を行い、桁位置を一致させる
fn plain_text_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.lines().map(|line| line.replace('\t', "    ")).collect();
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

// smart-case (クエリが全て小文字なら大小無視、大文字を含めば区別) の部分一致検索。
// 大小無視の比較は ASCII の範囲だけ行う (to_ascii_lowercase は char 数を変えないため、
// plain の char 列インデックスと桁位置が確実に一致する)
fn search_matches(plain: &[String], query: &str) -> Vec<Match> {
    if query.is_empty() {
        return Vec::new();
    }
    let ignore_case = !query.chars().any(|c| c.is_uppercase());
    let needle: Vec<char> = fold_case(query, ignore_case).collect();
    let mut matches = Vec::new();
    for (line, text) in plain.iter().enumerate() {
        let haystack: Vec<char> = fold_case(text, ignore_case).collect();
        if haystack.len() < needle.len() {
            continue;
        }
        for start in 0..=(haystack.len() - needle.len()) {
            if haystack[start..start + needle.len()] == needle[..] {
                matches.push(Match {
                    line,
                    start_col: start,
                    end_col: start + needle.len(),
                });
            }
        }
    }
    matches
}

fn fold_case(s: &str, ignore_case: bool) -> impl Iterator<Item = char> + '_ {
    s.chars()
        .map(move |c| if ignore_case { c.to_ascii_lowercase() } else { c })
}

fn gutter_span(number: usize, width: usize) -> Span<'static> {
    Span::styled(
        format!("{number:>width$} "),
        Style::default().fg(Color::DarkGray),
    )
}

fn convert_style(style: syntect::highlighting::Style) -> Style {
    let fg = style.foreground;
    let mut converted = Style::default().fg(Color::Rgb(fg.r, fg.g, fg.b));
    if style.font_style.contains(FontStyle::BOLD) {
        converted = converted.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        converted = converted.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        converted = converted.add_modifier(Modifier::UNDERLINED);
    }
    converted
}
