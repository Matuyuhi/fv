use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::FontStyle;
use syntect::parsing::SyntaxReference;
use syntect::util::LinesWithEndings;

use super::Viewer;

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

// 編集中はキーストローク毎に全文を再ハイライトするため、閲覧時の
// MAX_HIGHLIGHT_BYTES より大幅に低い閾値で早めにプレーン表示へ逃がす
const EDIT_HIGHLIGHT_BYTES: usize = 256 * 1024;

impl Viewer {
    /// 編集バッファの描画行を生成する (editor 用)。cache は経由しない —
    /// 再生成はキー入力起因の 1 回きりで、「再描画毎の再ハイライト禁止」には反しない
    pub fn highlight_text(&self, path: &Path, text: &str) -> Vec<Line<'static>> {
        let gutter_width = text.lines().count().max(1).to_string().len();
        if text.len() > EDIT_HIGHLIGHT_BYTES {
            plain_lines(text, gutter_width)
        } else {
            self.highlight_lines(path, text, gutter_width)
        }
    }

    pub(super) fn load(&self, path: &Path) -> Content {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => return Content::Error(format!("failed to read: {e}")),
        };
        let sniff = &bytes[..bytes.len().min(super::BINARY_SNIFF_BYTES)];
        if sniff.contains(&0) {
            return Content::Binary;
        }
        let text = String::from_utf8_lossy(&bytes);
        let gutter_width = text.lines().count().max(1).to_string().len();
        let lines = if bytes.len() > super::MAX_HIGHLIGHT_BYTES {
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
        if let Some(ext) = path.extension().and_then(|e| e.to_str())
            && let Some(syntax) = self.syntax_set.find_syntax_by_extension(ext)
        {
            return syntax;
        }
        // Makefile 等、拡張子なしのファイル名そのものが文法定義に登録されている
        if let Some(file_name) = path.file_name().and_then(|n| n.to_str())
            && let Some(syntax) = self.syntax_set.find_syntax_by_extension(file_name)
        {
            return syntax;
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
        .map(|(i, raw)| {
            Line::from(vec![
                gutter_span(i + 1, gutter_width),
                Span::raw(normalize(raw)),
            ])
        })
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
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| line.replace('\t', "    "))
        .collect();
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
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
