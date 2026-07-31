use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};

use crate::github::RemoteItem;
use crate::issuesview::IssuesState;

use super::pane_block;
use super::text_pane::TextPane;

// 狭い端末では優先度の低い列から落とす (タイトルが最後まで残る)。issue の要求通り
// 「狭い時はタイトル以外から落とす」を閾値の並びだけで表現する
const AUTHOR_THRESHOLD: u16 = 60;
const DATE_THRESHOLD: u16 = 80;
const LABEL_THRESHOLD: u16 = 100;

pub(super) fn draw_issues_list(
    frame: &mut Frame,
    issues: &mut IssuesState,
    focused: bool,
    area: Rect,
) {
    // Ctrl+d/u の半ページ移動に使う実測値。viewport.height 等と同じ ui→app の書き戻しパターン
    issues.list_area_height = area.height.saturating_sub(2) as usize;
    let title = format!(
        "issues {}/{} [{}]",
        issues.visible_count(),
        issues.total(),
        issues.state_filter.label()
    );
    if issues.list_loading() && !issues.fetched() {
        let paragraph = Paragraph::new("読み込み中…")
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }
    if let Some(err) = issues.list_error() {
        let text = format!("issues の取得に失敗しました:\n{err}\n\n(r で再取得)");
        let paragraph = Paragraph::new(text)
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::Red));
        frame.render_widget(paragraph, area);
        return;
    }
    if issues.visible_count() == 0 {
        let message = if issues.total() == 0 {
            "no issues"
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
    let items: Vec<ListItem> = issues
        .matches
        .iter()
        .map(|m| {
            let spans = issues
                .row(m.row)
                .map(|row| issue_line(row, &m.positions, width))
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
    let selected = (!issues.matches.is_empty()).then_some(issues.selected);
    issues.list_state.select(selected);
    frame.render_stateful_widget(list, area, &mut issues.list_state);
}

// #番号 タイトル(マッチ char をハイライト) [author] [更新日時] [labels]。author/更新日時/labels は
// 幅に応じて右から順に落とす (title_col_width の閾値がそのまま優先度)
fn issue_line(row: &RemoteItem, positions: &[usize], width: u16) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        format!("#{:<5} ", row.number),
        Style::default().fg(Color::DarkGray),
    )];
    spans.extend(highlight_title(row, positions));
    if width >= AUTHOR_THRESHOLD {
        spans.push(Span::styled(
            format!("  @{}", row.author),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if width >= DATE_THRESHOLD {
        spans.push(Span::styled(
            format!("  {}", short_date(&row.updated_at)),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if width >= LABEL_THRESHOLD && !row.labels.is_empty() {
        spans.push(Span::styled(
            format!("  [{}]", row.labels.join(", ")),
            Style::default().fg(Color::Cyan),
        ));
    }
    spans
}

// updatedAt は ISO8601 ("2026-07-30T12:34:56Z")。相対日時ライブラリを足さず日付部分だけ見せる
fn short_date(updated_at: &str) -> &str {
    updated_at.split('T').next().unwrap_or(updated_at)
}

// branch_panel::highlight_name と同じ発想 (positions は char インデックスなので char 単位で分割)。
// closed issue は取り消し線 + 暗い色で「閉じている」ことを一覧上で分かるようにする
fn highlight_title(row: &RemoteItem, positions: &[usize]) -> Vec<Span<'static>> {
    let base_style = if row.state.eq_ignore_ascii_case("open") {
        Style::default()
    } else {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::CROSSED_OUT)
    };
    let match_style = base_style
        .fg(Color::Cyan)
        .add_modifier(Modifier::UNDERLINED);
    let mut spans = Vec::new();
    let mut pos_iter = positions.iter().peekable();
    for (i, ch) in row.title.chars().enumerate() {
        let style = if pos_iter.peek() == Some(&&i) {
            pos_iter.next();
            match_style
        } else {
            base_style
        };
        spans.push(Span::styled(ch.to_string(), style));
    }
    spans
}

// 右ペイン: 選択 issue の詳細 (`gh issue view` のプレーン出力)。TextPane に一本化されている
// 既存の描画パイプラインをそのまま使う (gutter_width は 0、search/cursor は使わない)
pub(super) fn draw_issues_detail(
    frame: &mut Frame,
    issues: &mut IssuesState,
    focused: bool,
    background: Color,
    area: Rect,
) {
    issues.viewport.height = area.height.saturating_sub(2) as usize;
    issues.viewport.width = area.width.saturating_sub(2) as usize;

    let Some(title) = issues.title() else {
        let paragraph = Paragraph::new("Enter / l / クリック: 詳細を開く")
            .block(pane_block("issue".to_string(), focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    };
    if issues.detail_loading_current() {
        let paragraph = Paragraph::new("読み込み中…")
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }
    if let Some(err) = issues.detail_error() {
        let text = format!("取得に失敗しました:\n{err}\n\n(Enter で再試行)");
        let paragraph = Paragraph::new(text)
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::Red));
        frame.render_widget(paragraph, area);
        return;
    }
    if issues.line_count() == 0 {
        let paragraph = Paragraph::new("(empty)")
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }
    let pane = TextPane {
        lines: issues.lines(),
        changed_lines: &None,
        search: None,
        cursor: None,
        gutter_width: 0,
    };
    let visible = pane.visible(&issues.viewport);
    let paragraph = Paragraph::new(visible)
        .block(pane_block(title, focused))
        .style(Style::default().bg(background));
    frame.render_widget(paragraph, area);
}
