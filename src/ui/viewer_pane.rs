use std::collections::HashSet;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::app::{App, Focus};
use crate::viewer::{Content, SearchState};

use super::pane_block;

// 通常マッチ/カレントマッチのハイライト色
const MATCH_BG: Color = Color::Rgb(80, 80, 0);
const CURRENT_MATCH_BG: Color = Color::Rgb(255, 220, 0);

pub(super) fn draw_viewer(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Viewer;
    let inner_height = area.height.saturating_sub(2) as usize;
    app.viewer.viewport_height = inner_height;
    // 罫線分のみを引いた概算値。hscroll の緩いクランプにしか使わないので gutter 幅までは引かない
    app.viewer.viewport_width = area.width.saturating_sub(2) as usize;

    let Some(open) = &app.viewer.current else {
        let paragraph = Paragraph::new("no file selected")
            .block(pane_block(String::from("viewer"), focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    };
    // hscroll > 0 の間はステータスバーではなくタイトル側に現在オフセットを出す
    let title = if !app.viewer.wrap && app.viewer.hscroll > 0 {
        format!("{}  →{}", open.title, app.viewer.hscroll)
    } else {
        open.title.clone()
    };
    let block = pane_block(title, focused);
    match open.content.as_ref() {
        Content::Text { lines, .. } => {
            // Paragraph::scroll は u16 上限で巨大ファイルに届かないため、
            // 表示範囲を自前でスライスして先頭から描画する
            let start = app.viewer.scroll.min(lines.len().saturating_sub(1));
            let end = (start + inner_height).min(lines.len());
            let wrap = app.viewer.wrap;
            let hscroll = app.viewer.hscroll;
            let visible: Vec<Line> = (start..end)
                .map(|i| {
                    let line = mark_changed_line(&lines[i], i, &open.changed_lines);
                    let line = highlight_matches(&line, i, &app.viewer.search);
                    // シフトは最後に適用する。検索ハイライトの bg 計算は元の桁位置基準なので、
                    // 先にシフトすると global col がずれてマッチと違う文字に色が付いてしまう
                    if wrap {
                        line
                    } else {
                        hscroll_line(&line, hscroll)
                    }
                })
                .collect();
            let mut paragraph = Paragraph::new(visible)
                .block(block)
                .style(Style::default().bg(app.viewer.background()));
            if wrap {
                paragraph = paragraph.wrap(Wrap { trim: false });
            }
            frame.render_widget(paragraph, area);
        }
        Content::Binary => {
            let paragraph = Paragraph::new("binary file")
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(paragraph, area);
        }
        Content::Error(message) => {
            let paragraph = Paragraph::new(message.as_str())
                .block(block)
                .style(Style::default().fg(Color::Red));
            frame.render_widget(paragraph, area);
        }
    }
}

// gutter span (span[0]) の末尾1文字 (常に半角空白) を変更行マーカーに置き換えた
// 新しい Line を返す。キャッシュ済みの Line 自体は変更しない。span 数・各 span の
// 文字数はどちらも変わらないため、highlight_matches の列計算 (span[0] を除外して
// col=0 から数える) には影響しない
pub(super) fn mark_changed_line(
    line: &Line<'static>,
    line_idx: usize,
    changed: &Option<HashSet<usize>>,
) -> Line<'static> {
    let is_changed = changed
        .as_ref()
        .is_some_and(|lines| lines.contains(&(line_idx + 1)));
    if !is_changed {
        return line.clone();
    }
    let Some(gutter) = line.spans.first() else {
        return line.clone();
    };
    let mut text = gutter.content.to_string();
    text.pop();
    text.push('▎');

    let mut spans = Vec::with_capacity(line.spans.len());
    spans.push(Span::styled(text, Style::default().fg(Color::Cyan)));
    spans.extend(line.spans.iter().skip(1).cloned());
    Line::from(spans)
}

// gutter (span[0]) は固定したまま、コンテンツ部分だけ hscroll 文字分左にシフトした
// 新しい Line を返す。highlight_matches 適用後に呼ぶことで、シフトで画面外に落ちる文字ごと
// その bg スタイルも一緒に捨てられ、残った文字のハイライトは桁がずれず正しく残る
pub(super) fn hscroll_line(line: &Line<'static>, hscroll: usize) -> Line<'static> {
    if hscroll == 0 {
        return line.clone();
    }
    let mut spans = Vec::with_capacity(line.spans.len());
    if let Some(gutter) = line.spans.first() {
        spans.push(gutter.clone());
    }
    let mut col = 0usize;
    for span in line.spans.iter().skip(1) {
        let chars: Vec<char> = span.content.chars().collect();
        let span_end = col + chars.len();
        if span_end <= hscroll {
            // span 全体が切り捨て範囲に収まる場合は丸ごと捨てる
            col = span_end;
            continue;
        }
        let skip = hscroll.saturating_sub(col);
        let segment: String = chars[skip..].iter().collect();
        spans.push(Span::styled(segment, span.style));
        col = span_end;
    }
    Line::from(spans)
}

// キャッシュ済み span 列に背景色を重ねた新しい Line を組み立てる (キャッシュ自体は変更しない)
fn highlight_matches(
    line: &Line<'static>,
    line_idx: usize,
    search: &Option<SearchState>,
) -> Line<'static> {
    let Some(search) = search else {
        return line.clone();
    };
    let ranges: Vec<(usize, usize, bool)> = search
        .matches
        .iter()
        .enumerate()
        .filter(|(_, m)| m.line == line_idx)
        .map(|(i, m)| (m.start_col, m.end_col, Some(i) == search.current))
        .collect();
    if ranges.is_empty() {
        return line.clone();
    }

    // span[0] は行番号 gutter なのでハイライト対象から除外し、そのまま引き継ぐ
    let mut spans = Vec::with_capacity(line.spans.len());
    if let Some(gutter) = line.spans.first() {
        spans.push(gutter.clone());
    }

    let mut col = 0usize;
    for span in line.spans.iter().skip(1) {
        let chars: Vec<char> = span.content.chars().collect();
        let mut idx = 0usize;
        while idx < chars.len() {
            let global = col + idx;
            match ranges.iter().find(|(s, e, _)| *s <= global && global < *e) {
                Some(&(_, end, current)) => {
                    let seg_end = (end - col).min(chars.len());
                    let segment: String = chars[idx..seg_end].iter().collect();
                    let bg = if current { CURRENT_MATCH_BG } else { MATCH_BG };
                    let mut style = span.style.bg(bg);
                    if current {
                        style = style.fg(Color::Black);
                    }
                    spans.push(Span::styled(segment, style));
                    idx = seg_end;
                }
                None => {
                    let next_start = ranges
                        .iter()
                        .map(|(s, _, _)| *s)
                        .filter(|&s| s > global)
                        .min();
                    let seg_end = match next_start {
                        Some(s) => (s - col).min(chars.len()),
                        None => chars.len(),
                    };
                    let segment: String = chars[idx..seg_end].iter().collect();
                    spans.push(Span::styled(segment, span.style));
                    idx = seg_end;
                }
            }
        }
        col += chars.len();
    }
    Line::from(spans)
}
