use std::path::Path;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color as SyntectColor, FontStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use crate::text;

/// 編集中はキーストローク毎に全文を再ハイライトするため、閲覧時の
/// MAX_HIGHLIGHT_BYTES より大幅に低い閾値で早めにプレーン表示へ逃がす
const EDIT_HIGHLIGHT_BYTES: usize = 256 * 1024;

/// syntect によるハイライトとテーマ管理。閲覧 (content.rs の load) と
/// 編集 (EditState::rebuild) の両方が同じ実体を使う。cache は持たない —
/// 何をいつキャッシュするかは呼び出し側 (Viewer) の責務
pub struct Highlighter {
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
    theme: Theme,
    theme_name: String,
}

impl Highlighter {
    pub fn new() -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let theme_set = ThemeSet::load_defaults();
        let theme_name = "base16-ocean.dark".to_string();
        let mut theme = theme_set
            .themes
            .get(&theme_name)
            .cloned()
            .expect("base16-ocean.dark is bundled in syntect's default themes");
        tweak_comment_color(&mut theme);
        Self {
            syntax_set,
            theme_set,
            theme,
            theme_name,
        }
    }

    pub fn background(&self) -> Color {
        self.theme
            .settings
            .background
            .map(|c| Color::Rgb(c.r, c.g, c.b))
            .unwrap_or(Color::Reset)
    }

    pub fn theme_name(&self) -> &str {
        &self.theme_name
    }

    /// テーマ差し替え。ハイライト済み Line の無効化 (cache 破棄・再生成) は呼び出し側が行う
    pub fn set_theme(&mut self, name: &str) -> bool {
        let Some(mut theme) = self.theme_set.themes.get(name).cloned() else {
            return false;
        };
        tweak_comment_color(&mut theme);
        self.theme = theme;
        self.theme_name = name.to_string();
        true
    }

    /// 編集バッファの描画行を生成する (editor 用)。cache は経由しない —
    /// 再生成はキー入力起因の 1 回きりで、「再描画毎の再ハイライト禁止」には反しない
    pub fn highlight_text(&self, path: &Path, text: &str) -> Vec<Line<'static>> {
        let gutter_width = text::gutter_width(text.lines().count());
        if text.len() > EDIT_HIGHLIGHT_BYTES {
            plain_lines(text, gutter_width)
        } else {
            self.highlight_lines(path, text, gutter_width)
        }
    }

    pub(super) fn highlight_lines(
        &self,
        path: &Path,
        text: &str,
        gutter_width: usize,
    ) -> Vec<Line<'static>> {
        let syntax = self.find_syntax(path, text);
        let mut highlighter = HighlightLines::new(syntax, &self.theme);
        let mut lines = Vec::new();
        for (i, raw) in LinesWithEndings::from(text).enumerate() {
            let mut spans = vec![gutter_span(i + 1, gutter_width)];
            match highlighter.highlight_line(raw, &self.syntax_set) {
                Ok(ranges) => {
                    for (style, segment) in ranges {
                        let segment = text::normalize(segment);
                        if segment.is_empty() {
                            continue;
                        }
                        spans.push(Span::styled(segment, convert_style(style)));
                    }
                }
                // 文法定義とファイル内容の組み合わせによってはパースが失敗しうる。
                // その行だけ無色で出し、表示自体は継続する
                Err(_) => spans.push(Span::raw(text::normalize(raw))),
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

pub(super) fn plain_lines(text: &str, gutter_width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = text
        .lines()
        .enumerate()
        .map(|(i, raw)| {
            Line::from(vec![
                gutter_span(i + 1, gutter_width),
                Span::raw(text::normalize(raw)),
            ])
        })
        .collect();
    if lines.is_empty() {
        lines.push(Line::from(gutter_span(1, gutter_width)));
    }
    lines
}

// gutter_width は末尾空白込みの全体幅なので、数字の右詰め幅はそこから 1 引いた値
fn gutter_span(number: usize, gutter_width: usize) -> Span<'static> {
    let digits = gutter_width.saturating_sub(1);
    Span::styled(
        format!("{number:>digits$} "),
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

const COMMENT_COLOR_ADJUSTMENT: u8 = 56;

fn tweak_comment_color(theme: &mut Theme) {
    // 背景が明るいテーマ (base16-ocean.light, Solarized (light) 等) で常に明るくすると
    // 白背景に同化して見えなくなるため、背景輝度に応じて明るくする/暗くするを切り替える。
    // background が無いテーマは元々暗背景想定 (base16-ocean.dark 由来) なので明るくする側とする
    let darken = theme
        .settings
        .background
        .is_some_and(|bg| luminance(bg) >= 128);
    for item in &mut theme.scopes {
        // コメント系スコープだけ背景への同化を防ぐ
        if !format!("{:?}", item.scope)
            .to_ascii_lowercase()
            .contains("comment")
        {
            continue;
        }
        let Some(fg) = item.style.foreground else {
            continue;
        };
        item.style.foreground = Some(SyntectColor {
            r: adjust(fg.r, darken),
            g: adjust(fg.g, darken),
            b: adjust(fg.b, darken),
            a: fg.a,
        });
    }
}

fn adjust(c: u8, darken: bool) -> u8 {
    if darken {
        c.saturating_sub(COMMENT_COLOR_ADJUSTMENT)
    } else {
        c.saturating_add(COMMENT_COLOR_ADJUSTMENT)
    }
}

// ITU-R BT.601 の重み付けを整数演算で近似した簡易輝度 (0-255)。
// 255 * 299 (最大項) が u16 に収まらないため u32 で計算する
fn luminance(c: SyntectColor) -> u16 {
    ((c.r as u32 * 299 + c.g as u32 * 587 + c.b as u32 * 114) / 1000) as u16
}
