use std::path::Path;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use syntect::highlighting::{
    Color as SyntectColor, FontStyle, HighlightIterator, HighlightState,
    Highlighter as ThemeHighlighter, Style as SyntectStyle, Theme, ThemeSet,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

use crate::text;

/// syntect のシンタックス定義とテーマの置き場。ハイライト結果も行の状態も持たない —
/// 何をどこまで計算するかは呼び出し側 (render.rs の HighlightCache) の責務
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

    /// テーマ差し替え。組み立て済みの行・パーサ状態の破棄は呼び出し側が行う
    pub fn set_theme(&mut self, name: &str) -> bool {
        let Some(mut theme) = self.theme_set.themes.get(name).cloned() else {
            return false;
        };
        tweak_comment_color(&mut theme);
        self.theme = theme;
        self.theme_name = name.to_string();
        true
    }

    /// 行単位で再開できるハイライトの実行単位を作る。テーマ側の Highlighter (セレクタの
    /// 展開) はここで 1 回だけ組み立て、行ごとの状態は LineState として呼び出し側が持ち回る
    pub(super) fn session<'a>(&'a self, path: &Path, first_line: &str) -> Session<'a> {
        Session {
            syntax_set: &self.syntax_set,
            theme: ThemeHighlighter::new(&self.theme),
            syntax: find_syntax(&self.syntax_set, path, first_line),
        }
    }
}

fn find_syntax<'a>(
    syntax_set: &'a SyntaxSet,
    path: &Path,
    first_line: &str,
) -> &'a SyntaxReference {
    if let Some(ext) = path.extension().and_then(|e| e.to_str())
        && let Some(syntax) = syntax_set.find_syntax_by_extension(ext)
    {
        return syntax;
    }
    // Makefile 等、拡張子なしのファイル名そのものが文法定義に登録されている
    if let Some(file_name) = path.file_name().and_then(|n| n.to_str())
        && let Some(syntax) = syntax_set.find_syntax_by_extension(file_name)
    {
        return syntax;
    }
    syntax_set
        .find_syntax_by_first_line(first_line)
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text())
}

/// 1 ファイル分のハイライト実行単位。行を 1 本ずつ食わせて LineState を進める
pub(super) struct Session<'a> {
    syntax_set: &'a SyntaxSet,
    theme: ThemeHighlighter<'a>,
    syntax: &'a SyntaxReference,
}

/// ある行を解析する直前のパーサ状態。Clone して途中経過を保存できるので、
/// 文書の先頭からやり直さずに任意の行からハイライトを再開できる
/// (これが「ファイル全体を毎回ハイライトしない」ことの土台)
#[derive(Clone)]
pub(super) struct LineState {
    parse: ParseState,
    highlight: HighlightState,
}

impl Session<'_> {
    pub(super) fn start(&self) -> LineState {
        LineState {
            parse: ParseState::new(self.syntax),
            highlight: HighlightState::new(&self.theme, ScopeStack::new()),
        }
    }

    /// raw (行末の改行を含む) を 1 行ハイライトし、描画用の span を spans へ積む
    pub(super) fn line(&self, raw: &str, state: &mut LineState, spans: &mut Vec<Span<'static>>) {
        self.scan(raw, state, |style, segment| {
            let segment = text::normalize(segment);
            if segment.is_empty() {
                return;
            }
            spans.push(match style {
                Some(style) => Span::styled(segment, convert_style(style)),
                None => Span::raw(segment),
            });
        });
    }

    /// 画面に映らない行を状態だけ進める (可視範囲より手前の助走)。span を組み立てない分だけ
    /// 文字列の確保が丸ごと省ける
    pub(super) fn skip(&self, raw: &str, state: &mut LineState) {
        self.scan(raw, state, |_, _| {});
    }

    // 文法定義とファイル内容の組み合わせによってはパースが失敗しうる。その行だけ
    // style なし (無色) の 1 セグメントとして流し、表示自体は継続する
    fn scan(
        &self,
        raw: &str,
        state: &mut LineState,
        mut emit: impl FnMut(Option<SyntectStyle>, &str),
    ) {
        match state.parse.parse_line(raw, self.syntax_set) {
            Ok(ops) => {
                let iter = HighlightIterator::new(&mut state.highlight, &ops, raw, &self.theme);
                for (style, segment) in iter {
                    emit(Some(style), segment);
                }
            }
            Err(_) => emit(None, raw),
        }
    }
}

// gutter_width は末尾空白込みの全体幅なので、数字の右詰め幅はそこから 1 引いた値
pub(super) fn gutter_span(number: usize, gutter_width: usize) -> Span<'static> {
    let digits = gutter_width.saturating_sub(1);
    Span::styled(
        format!("{number:>digits$} "),
        Style::default().fg(Color::DarkGray),
    )
}

fn convert_style(style: SyntectStyle) -> Style {
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
