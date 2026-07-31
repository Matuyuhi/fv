//! issues (#33) / pull requests (#34) タブが共有する描画部品。「読み込み中/エラー/0 件/一覧」
//! の分岐と List ウィジェットの組み立て、右ペインのプレーンテキスト表示 (issues の詳細・PR の
//! 説明/CI ステータス) はどちらも同じ形なので、行の型ごとの違いはクロージャに追い出して 1 度だけ
//! 実装する (#34 の受け入れ条件: 一覧の描画を 2 回書かない)。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::remotelist::ListMatch;
use crate::viewer::Viewport;

use super::pane_block;
use super::text_pane::TextPane;

/// 左ペイン: 一覧。row_line が行の型ごとの表示テキスト組み立てを担う
/// (issue_line/pr_line。「一覧行の表示テキストを組み立てる関数を型ごとに差し替える」形)
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_remote_list<R>(
    frame: &mut Frame,
    title: String,
    loading_initial: bool,
    error: Option<&str>,
    total: usize,
    empty_label: &str,
    matches: &[ListMatch],
    rows: &[R],
    row_line: impl Fn(&R, &[usize], u16) -> Vec<Span<'static>>,
    selected: usize,
    list_state: &mut ListState,
    focused: bool,
    area: Rect,
) {
    if loading_initial {
        let paragraph = Paragraph::new("読み込み中…")
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }
    if let Some(err) = error {
        let text = format!("取得に失敗しました:\n{err}\n\n(r で再取得)");
        let paragraph = Paragraph::new(text)
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::Red));
        frame.render_widget(paragraph, area);
        return;
    }
    if matches.is_empty() {
        let message = if total == 0 {
            empty_label
        } else {
            "no matches"
        };
        let paragraph = Paragraph::new(message)
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }

    let width = area.width;
    let items: Vec<ListItem> = matches
        .iter()
        .map(|m| {
            let spans = rows
                .get(m.row)
                .map(|row| row_line(row, &m.positions, width))
                .unwrap_or_default();
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items)
        .block(pane_block(title, focused))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    list_state.select((!matches.is_empty()).then_some(selected));
    frame.render_stateful_widget(list, area, list_state);
}

/// 右ペイン: プレーンテキストの詳細 (issues の詳細、PR の説明/CI ステータス)。
/// TextPane に一本化されている既存の描画パイプラインをそのまま使う
/// (gutter_width は 0、search/cursor は使わない)。viewport の height/width の書き戻しは
/// 呼び出し側が事前に済ませておく (`&mut` を要求すると、同じ呼び出しで必要な他の借用
/// (lines 等、状態から借りたデータ) と同時に借りられなくなるため、ここは `&Viewport` で読むだけ)
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_text_detail(
    frame: &mut Frame,
    title: Option<String>,
    placeholder: &str,
    loading: bool,
    error: Option<&str>,
    lines: &[Line<'static>],
    viewport: &Viewport,
    focused: bool,
    background: Color,
    area: Rect,
) {
    let Some(title) = title else {
        let paragraph = Paragraph::new(placeholder.to_string())
            .block(pane_block("detail".to_string(), focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    };
    if loading {
        let paragraph = Paragraph::new("読み込み中…")
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }
    if let Some(err) = error {
        let text = format!("取得に失敗しました:\n{err}\n\n(再試行で開き直せます)");
        let paragraph = Paragraph::new(text)
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::Red));
        frame.render_widget(paragraph, area);
        return;
    }
    if lines.is_empty() {
        let paragraph = Paragraph::new("(empty)")
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }
    let pane = TextPane {
        lines,
        changed_lines: &None,
        search: None,
        cursor: None,
        gutter_width: 0,
    };
    let visible = pane.visible(viewport);
    let paragraph = Paragraph::new(visible)
        .block(pane_block(title, focused))
        .style(Style::default().bg(background));
    frame.render_widget(paragraph, area);
}
