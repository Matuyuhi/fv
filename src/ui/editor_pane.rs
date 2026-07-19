use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Mode};
use crate::editor::{EditState, display_col};

use super::pane_block;
use super::viewer_pane::{hscroll_line, mark_changed_line};

// 編集中の右ペイン。viewer_pane とは独立した描画パスにして、検索ハイライト・
// 変更行マーク・wrap の桁整合パイプラインと相互作用しないようにする
pub(super) fn draw_editor(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner_height = area.height.saturating_sub(2) as usize;
    app.viewer.viewport_height = inner_height;
    app.viewer.viewport_width = area.width.saturating_sub(2) as usize;

    let Mode::Edit(state) = &app.mode else {
        return;
    };
    let name = app
        .viewer
        .current
        .as_ref()
        .map(|open| open.title.clone())
        .unwrap_or_else(|| state.path.display().to_string());
    let dirty = if state.buffer.dirty() { "*" } else { "" };
    // 編集はモーダルなのでフォーカスは常にこのペイン扱い
    let block = pane_block(format!("{name}{dirty} [EDIT]"), true);

    let start = app.viewer.scroll.min(state.lines.len().saturating_sub(1));
    let (cursor_line, cursor_col) = state.cursor;
    let cursor_display = display_col(state.buffer.line(cursor_line), cursor_col);
    let visible: Vec<Line> = if app.viewer.wrap {
        let width = app
            .viewer
            .viewport_width
            .saturating_sub(state.gutter_width)
            .max(1);
        wrapped_visible(state, start, inner_height, width, cursor_display)
    } else {
        let end = (start + inner_height).min(state.lines.len());
        let hscroll = app.viewer.hscroll;
        (start..end)
            .map(|i| {
                // マークは gutter の char 数を変えないため、後段の hscroll・カーソル重ねに影響しない
                let line = mark_changed_line(&state.lines[i], i, &state.changed_lines);
                let line = hscroll_line(&line, hscroll);
                if i == cursor_line {
                    overlay_cursor(line, cursor_display.saturating_sub(hscroll))
                } else {
                    line
                }
            })
            .collect()
    };
    let paragraph = Paragraph::new(visible)
        .block(block)
        .style(Style::default().bg(app.viewer.background()));
    frame.render_widget(paragraph, area);
}

// wrap 時の描画: 論理行を width で char 単位に自前分割する。Paragraph::wrap は
// 単語境界 wrap で折返し位置が外から計算できず、editor/mod.rs のカーソル追従
// (ensure_visible) とクリック座標 (click_at) の視覚行数と一致させられないため使わない。
// 続き行の gutter は空白で埋めて桁を揃える
fn wrapped_visible(
    state: &EditState,
    scroll: usize,
    height: usize,
    width: usize,
    cursor_display: usize,
) -> Vec<Line<'static>> {
    let mut rows: Vec<Line> = Vec::new();
    let mut i = scroll;
    while rows.len() < height && i < state.lines.len() {
        // マーカーは先頭の視覚行の gutter にだけ付く (続き行は pad で置き換わる)
        let marked = mark_changed_line(&state.lines[i], i, &state.changed_lines);
        let mut chunks = wrap_line(&marked, width, state.gutter_width);
        if i == state.cursor.0 {
            let row = cursor_display / width;
            // 折返し境界ちょうど (行末が width の倍数) に立った場合は空の続き行に置く
            while chunks.len() <= row {
                chunks.push(Line::from(vec![pad_span(state.gutter_width)]));
            }
            chunks[row] = overlay_cursor(std::mem::take(&mut chunks[row]), cursor_display % width);
        }
        rows.extend(chunks);
        i += 1;
    }
    rows.truncate(height);
    rows
}

// 論理行 1 本を width ごとの視覚行に切る。span の style は切れ目を跨いで保存する
fn wrap_line(line: &Line<'static>, width: usize, gutter_width: usize) -> Vec<Line<'static>> {
    let mut rows: Vec<Line> = Vec::new();
    let mut spans: Vec<Span> = vec![line.spans.first().cloned().unwrap_or_default()];
    let mut used = 0usize;
    for span in line.spans.iter().skip(1) {
        let chars: Vec<char> = span.content.chars().collect();
        let mut idx = 0;
        while idx < chars.len() {
            let take = (width - used).min(chars.len() - idx);
            if take == 0 {
                rows.push(Line::from(std::mem::replace(
                    &mut spans,
                    vec![pad_span(gutter_width)],
                )));
                used = 0;
                continue;
            }
            let segment: String = chars[idx..idx + take].iter().collect();
            spans.push(Span::styled(segment, span.style));
            used += take;
            idx += take;
        }
    }
    rows.push(Line::from(spans));
    rows
}

fn pad_span(gutter_width: usize) -> Span<'static> {
    Span::raw(" ".repeat(gutter_width))
}

// コンテンツ部 (span[0] の gutter を除く) の col 文字目に REVERSED を重ねた
// ブロックカーソルを見せる。行末より先なら REVERSED 空白を足す。
// 端末カーソルでなく文字スタイルにするのは、全角・タブの画面幅計算を避けるため
fn overlay_cursor(line: Line<'static>, col: usize) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 2);
    let mut iter = line.spans.into_iter();
    if let Some(gutter) = iter.next() {
        spans.push(gutter);
    }
    let mut remaining = col;
    let mut placed = false;
    for span in iter {
        if placed {
            spans.push(span);
            continue;
        }
        let count = span.content.chars().count();
        if remaining >= count {
            remaining -= count;
            spans.push(span);
            continue;
        }
        let content = span.content.into_owned();
        let before: String = content.chars().take(remaining).collect();
        let cursor: String = content.chars().skip(remaining).take(1).collect();
        let after: String = content.chars().skip(remaining + 1).collect();
        if !before.is_empty() {
            spans.push(Span::styled(before, span.style));
        }
        spans.push(Span::styled(
            cursor,
            span.style.add_modifier(Modifier::REVERSED),
        ));
        if !after.is_empty() {
            spans.push(Span::styled(after, span.style));
        }
        placed = true;
    }
    if !placed {
        spans.push(Span::styled(
            " ",
            Style::default().add_modifier(Modifier::REVERSED),
        ));
    }
    Line::from(spans)
}
