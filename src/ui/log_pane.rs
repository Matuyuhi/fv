use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph};

use crate::logview::LogState;

use super::pane_block;
use super::text_pane::TextPane;

// sticky header・全幅バンド化した通常のファイル境界行の固定色 (#40)。端末テーマに
// 依存させないのは word-level ハイライトの ADDED_WORD_BG 等と同じ方針
const BOUNDARY_BG: Color = Color::Cyan;
const BOUNDARY_FG: Color = Color::Black;

// 左ペイン: コミット一覧。ツリーではなく LogState が持つ commits を直接描く
pub(super) fn draw_log_list(frame: &mut Frame, log: &mut LogState, focused: bool, area: Rect) {
    let title = format!("log ({})", log.commits().len());
    if log.commits().is_empty() {
        let paragraph = Paragraph::new("no commits")
            .block(pane_block(title, focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }
    let open_index = log.open_index();
    let items: Vec<ListItem> = log
        .commits()
        .iter()
        .enumerate()
        .map(|(i, commit)| {
            // diff を開いている行だけ印を付ける (selected と別概念: j/k では動かない)
            let marker = if Some(i) == open_index { "▶ " } else { "  " };
            let label = format!(
                "{marker}{}  {:<15}  {:<12}  {}",
                commit.short, commit.relative_time, commit.author, commit.subject
            );
            ListItem::new(label)
        })
        .collect();
    let list = List::new(items)
        .block(pane_block(title, focused))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, area, &mut log.list_state);
}

// 右ペイン: 選択コミットの diff。GIT レーンの diff ペインと基本構造は同じだが、
// 基準 (base) の概念が無いのでタイトルにコミット情報を出すだけで良い
pub(super) fn draw_log_diff(
    frame: &mut Frame,
    log: &mut LogState,
    focused: bool,
    background: Color,
    area: Rect,
) {
    let inner_width = area.width.saturating_sub(2) as usize;
    // sticky header に 1 行使う分だけ TextPane へ渡す高さを削る。scroll 位置ではなく
    // 「このコミットの diff にファイル境界があるか」だけで決めるのが要点: scroll 依存にすると
    // コミットメッセージ部分とファイル本文とで高さが変わり、Ctrl+d/Ctrl+u のページ送り量が
    // スクロール中に狂う (キー処理側は書き戻し後の viewport.height をそのまま読む)
    let sticky_reserved = usize::from(log.has_file_boundary());
    log.viewport.height = (area.height.saturating_sub(2) as usize).saturating_sub(sticky_reserved);
    log.viewport.width = inner_width;

    let Some(title) = log.title() else {
        let paragraph = Paragraph::new("Enter/l: open diff")
            .block(pane_block("diff".to_string(), focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    };
    let title = if !log.viewport.wrap && log.viewport.hscroll > 0 {
        format!("{title}  →{}", log.viewport.hscroll)
    } else {
        title
    };
    let block = pane_block(title, focused);

    if log.line_count() == 0 {
        let paragraph = Paragraph::new("no changes")
            .block(block)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    }
    let pane = TextPane {
        lines: log.lines(),
        changed_lines: &None,
        search: None,
        cursor: None,
        gutter_width: log.gutter_width(),
    };
    let mut rows = pane.visible(&log.viewport);
    widen_boundary_bands(&mut rows, inner_width);
    if let Some(label) = log.sticky_label() {
        rows.insert(0, sticky_line(label, inner_width));
    }
    let paragraph = Paragraph::new(rows)
        .block(block)
        .style(Style::default().bg(background));
    frame.render_widget(paragraph, area);
}

// sticky 行 (常にペイン上端に固定するファイル名バー) を組み立てる。gutter は持たせず
// (diff 本文ではなくメタ情報のため) 全幅を同じ背景色で埋めて「本文ではない」ことを示す
fn sticky_line(label: &str, width: usize) -> Line<'static> {
    let style = Style::default()
        .fg(BOUNDARY_FG)
        .bg(BOUNDARY_BG)
        .add_modifier(Modifier::BOLD);
    let text = truncate_label(label, width.max(1));
    let pad = width.saturating_sub(text.chars().count());
    Span::styled(format!("{text}{}", " ".repeat(pad)), style).into()
}

// 流れる側 (スクロールで消えていく通常のファイル境界行) も見た目を強化する。
// render_commit がヘッダ行に付けた固定背景色を目印に、右側をペイン幅まで同じ背景で
// 埋めて全幅の帯にする。gitview 側の行組み立てには触れず、描画側だけの加工に留める
// (#40: 複数ファイル diff のレンダラ自体は #23/#30 と衝突を避けるため変更しない方針)
fn widen_boundary_bands(rows: &mut [Line<'static>], width: usize) {
    for row in rows.iter_mut() {
        let Some(style) = row
            .spans
            .iter()
            .find(|s| s.style.bg == Some(BOUNDARY_BG))
            .map(|s| s.style)
        else {
            continue;
        };
        let used: usize = row.spans.iter().map(|s| s.content.chars().count()).sum();
        if used < width {
            row.spans
                .push(Span::styled(" ".repeat(width - used), style));
        }
    }
}

// 長いパスは先頭を省略する。末尾のファイル名が最も情報量が多いため、区切り文字境界で
// 前方のディレクトリ階層から落としていき、それでも収まらなければファイル名自体を
// 末尾優先で char 単位に切る
fn truncate_label(label: &str, max_width: usize) -> String {
    if label.chars().count() <= max_width {
        return label.to_string();
    }
    let mut parts: Vec<&str> = label.split('/').collect();
    while parts.len() > 1 {
        parts.remove(0);
        let candidate = format!("…/{}", parts.join("/"));
        if candidate.chars().count() <= max_width {
            return candidate;
        }
    }
    let budget = max_width.saturating_sub(1);
    let mut tail: Vec<char> = label.chars().rev().take(budget).collect();
    tail.reverse();
    let tail: String = tail.into_iter().collect();
    format!("…{tail}")
}
