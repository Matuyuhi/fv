use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

/// これを超えるファイルはハイライトせずプレーン表示する
const MAX_HIGHLIGHT_BYTES: usize = 10 * 1024 * 1024;
/// バイナリ判定で先頭から NUL バイトを探す範囲
const BINARY_SNIFF_BYTES: usize = 8192;

pub enum Content {
    Text { lines: Vec<Line<'static>> },
    Binary,
    Error(String),
}

pub struct Open {
    pub title: String,
    pub path: PathBuf,
    pub content: Rc<Content>,
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
        let title = path.strip_prefix(root).unwrap_or(path).display().to_string();
        let content = match self.cache.get(path) {
            Some(cached) => Rc::clone(cached),
            None => {
                let loaded = Rc::new(self.load(path));
                self.cache.insert(path.to_path_buf(), Rc::clone(&loaded));
                loaded
            }
        };
        self.scroll = 0;
        self.current = Some(Open {
            title,
            path: path.to_path_buf(),
            content,
        });
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
        if let Some(open) = &mut self.current {
            open.content = loaded;
        }
        let last = self.line_count().saturating_sub(1);
        self.scroll = self.scroll.min(last);
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let last = self.line_count().saturating_sub(1) as isize;
        self.scroll = (self.scroll as isize + delta).clamp(0, last) as usize;
    }

    pub fn line_count(&self) -> usize {
        match &self.current {
            Some(open) => match open.content.as_ref() {
                Content::Text { lines } => lines.len(),
                _ => 0,
            },
            None => 0,
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
        Content::Text { lines }
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
