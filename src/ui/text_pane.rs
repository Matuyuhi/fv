use std::collections::HashSet;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::viewer::{SearchState, Viewport};

// 通常マッチ/カレントマッチのハイライト色
const MATCH_BG: Color = Color::Rgb(80, 80, 0);
const CURRENT_MATCH_BG: Color = Color::Rgb(255, 220, 0);

/// 閲覧 (viewer_pane) と編集 (editor_pane) で共通のテキスト描画パイプライン。
/// 行加工順は mark_changed_line → highlight_matches → (hscroll | char 単位 wrap) →
/// cursor overlay で固定。順序を入れ替えると検索マッチ・カーソルの絶対桁がズレる
/// (CLAUDE.md の桁位置整合インバリアント)。
/// 閲覧は search だけ、編集は cursor だけを Some にする — 両方を同時に使うモードはない
pub(super) struct TextPane<'a> {
    pub lines: &'a [Line<'static>],
    pub changed_lines: &'a Option<HashSet<usize>>,
    pub search: Option<&'a SearchState>,
    /// ブロックカーソルの (論理行, 表示桁)
    pub cursor: Option<(usize, usize)>,
    /// 行番号 gutter (span[0]) の char 幅。wrap の続き行 pad と hscroll の除外幅に使う
    pub gutter_width: usize,
}

impl TextPane<'_> {
    /// viewport に収まる分の描画行を組み立てる。Paragraph::scroll / Paragraph::wrap は
    /// 使わない (u16 上限と、折返し位置が外から計算できない問題をどちらも避ける)
    pub fn visible(&self, vp: &Viewport) -> Vec<Line<'static>> {
        let start = vp.scroll.min(self.lines.len().saturating_sub(1));
        if vp.wrap {
            return self.wrapped(start, vp);
        }
        let end = (start + vp.height).min(self.lines.len());
        (start..end)
            .map(|i| {
                let line = self.marked_and_highlighted(i);
                let line = hscroll_line(&line, vp.hscroll);
                match self.cursor {
                    Some((cursor_line, col)) if cursor_line == i => {
                        overlay_cursor(line, col.saturating_sub(vp.hscroll))
                    }
                    _ => line,
                }
            })
            .collect()
    }

    // wrap 時の描画: 論理行を width で char 単位に自前分割する。折返し位置を
    // カーソル追従 (editor の ensure_visible) とクリック座標 (click_at) の視覚行数
    // 計算 (text::wrap_rows) と一致させるため、単語境界 wrap は使わない
    fn wrapped(&self, start: usize, vp: &Viewport) -> Vec<Line<'static>> {
        let width = vp.width.saturating_sub(self.gutter_width).max(1);
        let mut rows: Vec<Line> = Vec::new();
        let mut i = start;
        while rows.len() < vp.height && i < self.lines.len() {
            // マーカーは先頭の視覚行の gutter にだけ付く (続き行は pad で置き換わる)
            let line = self.marked_and_highlighted(i);
            let mut chunks = wrap_line(&line, width, self.gutter_width);
            if let Some((cursor_line, col)) = self.cursor
                && cursor_line == i
            {
                let row = col / width;
                // 折返し境界ちょうど (行末が width の倍数) に立った場合は空の続き行に置く
                while chunks.len() <= row {
                    chunks.push(Line::from(vec![pad_span(self.gutter_width)]));
                }
                chunks[row] = overlay_cursor(std::mem::take(&mut chunks[row]), col % width);
            }
            rows.extend(chunks);
            i += 1;
        }
        rows.truncate(vp.height);
        rows
    }

    fn marked_and_highlighted(&self, i: usize) -> Line<'static> {
        let line = mark_changed_line(&self.lines[i], i, self.changed_lines);
        match self.search {
            Some(search) => highlight_matches(&line, i, search),
            None => line,
        }
    }
}

// gutter span (span[0]) の末尾1文字 (常に半角空白) を変更行マーカーに置き換えた
// 新しい Line を返す。キャッシュ済みの Line 自体は変更しない。span 数・各 span の
// 文字数はどちらも変わらないため、highlight_matches の列計算 (span[0] を除外して
// col=0 から数える) には影響しない
fn mark_changed_line(
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
fn hscroll_line(line: &Line<'static>, hscroll: usize) -> Line<'static> {
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
fn highlight_matches(line: &Line<'static>, line_idx: usize, search: &SearchState) -> Line<'static> {
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

// 続き行の gutter 部分を空白で埋めて桁を揃える
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
