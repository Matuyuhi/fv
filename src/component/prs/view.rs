//! pull requests タブ (#34) の描画。左ペイン (一覧) は issues タブと同じ
//! remote_list_pane::draw_remote_list を再利用し、右ペインは表示切替 (説明/diff/CI) を
//! ここで振り分ける。diff だけ GIT/LOG レーンと同じ sticky header・hunk・wrap の描画が
//! 要るので、gitlane::render_commit の結果 (PrsState 経由) と widget/diff_boundary.rs の
//! 部品をそのまま使う (行の組み立てそのものは複製しない)

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use crate::component::prs::{DetailView, PrsState};
use crate::github::PrRow;

use crate::component::issues::view::{highlight_span, short_date};
use crate::component::remotelist::view::{draw_remote_list, draw_text_detail};
use crate::widget::diff_boundary::{sticky_line, widen_boundary_bands};
use crate::widget::pane_block;
use crate::widget::text_pane::{LineWindow, TextPane, widen_row_bands};

const AUTHOR_THRESHOLD: u16 = 60;
const BRANCH_THRESHOLD: u16 = 80;
const DATE_THRESHOLD: u16 = 100;

pub(crate) fn draw_pr_list(frame: &mut Frame, prs: &mut PrsState, focused: bool, area: Rect) {
    prs.list_area_height = area.height.saturating_sub(2) as usize;
    let title = format!(
        "pull requests {}/{} [{}]",
        prs.visible_count(),
        prs.total(),
        prs.state_filter.label()
    );
    // list_error() は Option<&str> (prs を借りたまま) を返すため、&mut prs.list_state と
    // 同じ呼び出しには渡せない (issues_pane::draw_issues_list と同じ理由で String に複製する)
    let error = prs.list_error().map(str::to_string);
    draw_remote_list(
        frame,
        title,
        prs.list_loading() && !prs.fetched(),
        error.as_deref(),
        prs.total(),
        "no pull requests",
        &prs.matches,
        &prs.rows,
        pr_line,
        prs.selected,
        &mut prs.list_state,
        focused,
        area,
    );
}

// #番号 [draft] タイトル(マッチハイライト) [author] [ブランチ] [更新日時]。issues の
// issue_line と同じ「狭い端末では右側の列から落とす」方針だが、draft バッジ・ブランチ名が
// PR 固有の付随情報 (RemoteItem を汚さないための github::PrRow) として増える
fn pr_line(row: &PrRow, positions: &[usize], width: u16) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        format!("#{:<5} ", row.item.number),
        Style::default().fg(Color::DarkGray),
    )];
    if row.is_draft {
        spans.push(Span::styled(
            "draft ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.extend(highlight_span(
        &row.item.title,
        positions,
        state_style(&row.item.state),
    ));
    if width >= AUTHOR_THRESHOLD {
        spans.push(Span::styled(
            format!("  @{}", row.item.author),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if width >= BRANCH_THRESHOLD {
        spans.push(Span::styled(
            format!("  {}", row.head_ref),
            Style::default().fg(Color::Magenta),
        ));
    }
    if width >= DATE_THRESHOLD {
        spans.push(Span::styled(
            format!("  {}", short_date(&row.item.updated_at)),
            Style::default().fg(Color::DarkGray),
        ));
    }
    spans
}

// merged は closed とは違う色 (紫) で見分けられるようにする。issues は open/closed の 2 値だが
// PR は state に "MERGED" も乗るため issue_line の 2 値判定をそのまま使えない
fn state_style(state: &str) -> Style {
    if state.eq_ignore_ascii_case("open") {
        Style::default()
    } else if state.eq_ignore_ascii_case("merged") {
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::CROSSED_OUT)
    } else {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::CROSSED_OUT)
    }
}

// 右ペイン: 説明 (既定) / diff (d) / CI ステータス (S) の切替。診断・スクロール状態は
// PrsState 側 (current_viewport 等) が view ごとに振り分け済みなので、ここは描き分けるだけ
pub(crate) fn draw_pr_detail(
    frame: &mut Frame,
    prs: &mut PrsState,
    focused: bool,
    background: Color,
    area: Rect,
) {
    if prs.view == DetailView::Diff {
        draw_pr_diff(frame, prs, focused, background, area);
        return;
    }
    prs.text_viewport.height = area.height.saturating_sub(2) as usize;
    prs.text_viewport.width = area.width.saturating_sub(2) as usize;
    let title = prs.title();
    draw_text_detail(
        frame,
        title,
        "Enter / l / クリック: 説明を開く (d: diff  S: CI)",
        prs.loading_current(),
        prs.error_current(),
        prs.lines(),
        &prs.text_viewport,
        focused,
        background,
        area,
    );
}

// diff 表示。GIT レーン (git_pane.rs) の単一ファイル diff・LOG レーン (log_pane.rs) の
// 複数ファイル diff と同じ見え方 (行番号 gutter・色・hunk・sticky header) にするため、
// gitlane::render_commit の結果をそのまま同じ組み立て順 (widen_boundary_bands → sticky_line
// を先頭に挿す) で描く。行の組み立てそのものは PrsState::fetch 側 (gitlane::render_commit) に
// 任せ、ここでは描画だけを行う
fn draw_pr_diff(
    frame: &mut Frame,
    prs: &mut PrsState,
    focused: bool,
    background: Color,
    area: Rect,
) {
    let inner_width = area.width.saturating_sub(2) as usize;
    let sticky_reserved = usize::from(prs.has_file_boundary());
    prs.diff_viewport.height =
        (area.height.saturating_sub(2) as usize).saturating_sub(sticky_reserved);
    prs.diff_viewport.width = inner_width;

    let Some(mut title) = prs.title() else {
        let paragraph = Paragraph::new("Enter/l: open  d: diff")
            .block(pane_block("diff".to_string(), focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    };
    if prs.truncated_current() {
        title.push_str("  (打ち切り)");
    }
    if !prs.diff_viewport.wrap && prs.diff_viewport.hscroll > 0 {
        title = format!("{title}  →{}", prs.diff_viewport.hscroll);
    }

    if prs.loading_current() {
        let paragraph = Paragraph::new("読み込み中…")
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }
    if let Some(err) = prs.error_current() {
        let text = format!("取得に失敗しました:\n{err}\n\n(d で再試行)");
        let paragraph = Paragraph::new(text)
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::Red));
        frame.render_widget(paragraph, area);
        return;
    }
    if prs.line_count() == 0 {
        let paragraph = Paragraph::new("no changes")
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }
    let pane = TextPane {
        window: LineWindow::slice(prs.lines(), &prs.diff_viewport),
        changed_lines: &None,
        search: None,
        selection: None,
        cursor: None,
        // 帯を出すのはこのペインにフォーカスがある間だけ (draw_git / draw_log_diff と同じ)
        focus_row: focused.then(|| prs.cursor()).flatten(),
        selected_rows: None,
        gutter_width: prs.gutter_width(),
    };
    let mut rows = pane.visible(&prs.diff_viewport);
    widen_row_bands(&mut rows, inner_width);
    widen_boundary_bands(&mut rows, inner_width);
    if let Some(label) = prs.sticky_label() {
        rows.insert(0, sticky_line(label, inner_width));
    }
    let paragraph = Paragraph::new(rows)
        .block(pane_block(title, focused))
        .style(Style::default().bg(background));
    frame.render_widget(paragraph, area);
}
