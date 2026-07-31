use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::github::RemoteItem;
use crate::issuesview::IssuesState;

use super::remote_list_pane::{draw_remote_list, draw_text_detail};

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
    // list_error() は Option<&str> (issues を借りたまま) を返すため、&mut issues.list_state と
    // 同じ呼び出しには渡せない (String に複製して借用を切ってから渡す。エラー時のみのコストなので安い)
    let error = issues.list_error().map(str::to_string);
    draw_remote_list(
        frame,
        title,
        issues.list_loading() && !issues.fetched(),
        error.as_deref(),
        issues.total(),
        "no issues",
        &issues.matches,
        &issues.rows,
        issue_line,
        issues.selected,
        &mut issues.list_state,
        focused,
        area,
    );
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
pub(super) fn short_date(updated_at: &str) -> &str {
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
    highlight_span(&row.title, positions, base_style)
}

// PR タブ (ui/pr_pane.rs) のタイトルハイライトとも共有する (positions は char インデックス)
pub(super) fn highlight_span(
    text: &str,
    positions: &[usize],
    base_style: Style,
) -> Vec<Span<'static>> {
    let match_style = base_style
        .fg(Color::Cyan)
        .add_modifier(Modifier::UNDERLINED);
    let mut spans = Vec::new();
    let mut pos_iter = positions.iter().peekable();
    for (i, ch) in text.chars().enumerate() {
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
    // TextPane に渡す viewport の実測値書き戻し (ui→app の既存パターン)。draw_text_detail は
    // &Viewport しか要求しないため、書き戻しは呼び出し側 (ここ) で先に済ませておく
    issues.viewport.height = area.height.saturating_sub(2) as usize;
    issues.viewport.width = area.width.saturating_sub(2) as usize;
    let title = issues.title();
    draw_text_detail(
        frame,
        title,
        "Enter / l / クリック: 詳細を開く",
        issues.detail_loading_current(),
        issues.detail_error(),
        issues.lines(),
        &issues.viewport,
        focused,
        background,
        area,
    );
}
