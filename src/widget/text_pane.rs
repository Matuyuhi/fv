use std::collections::HashSet;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::component::viewer::{SearchState, Selection, Viewport};
use crate::text;

// 通常マッチ/カレントマッチのハイライト色
const MATCH_BG: Color = Color::Rgb(80, 80, 0);
const CURRENT_MATCH_BG: Color = Color::Rgb(255, 220, 0);
// 範囲選択の色。テーマごとの前景色に依らず必ず読めるよう、前景も一緒に固定する
// (カレントマッチが黄色地に黒文字を固定しているのと同じ理由)
const SELECTION_BG: Color = Color::Rgb(58, 82, 128);
const SELECTION_FG: Color = Color::Rgb(236, 240, 248);

/// TextPane に渡す可視ウィンドウ。文書全体ではなく「vp.scroll から height 論理行」だけを
/// 持つ形にしてあるのは、大きなファイルを丸ごとハイライトせずに済ませる
/// (viewer::HighlightCache が可視範囲だけを組み立てる) ため
pub(crate) struct LineWindow<'a> {
    /// rows[0] が論理行 first にあたる
    pub rows: &'a [Line<'static>],
    pub first: usize,
}

impl<'a> LineWindow<'a> {
    /// 全行を既にメモリに持っている呼び出し側 (diff 系ペイン) 用の切り出し。
    /// wrap 中でも 1 論理行は最低 1 視覚行を占めるので、height 論理行あれば画面は必ず埋まる
    pub(crate) fn slice(lines: &'a [Line<'static>], vp: &Viewport) -> Self {
        let first = vp.scroll.min(lines.len().saturating_sub(1));
        let end = (first + vp.height).min(lines.len());
        Self {
            rows: &lines[first..end],
            first,
        }
    }
}

/// 閲覧 (viewer_pane) と編集 (editor_pane) で共通のテキスト描画パイプライン。
/// 行加工順は mark_changed_line → highlight_matches → highlight_selection →
/// (hscroll | セル単位 wrap) → cursor overlay で固定。順序を入れ替えると検索マッチ・
/// 選択範囲・カーソルの絶対桁がズレる (CLAUDE.md の桁位置整合インバリアント)。
/// 閲覧は search/selection だけ、編集は cursor だけを Some にする —
/// 両方を同時に使うモードはない
pub(crate) struct TextPane<'a> {
    pub window: LineWindow<'a>,
    pub changed_lines: &'a Option<HashSet<usize>>,
    pub search: Option<&'a SearchState>,
    /// VIEW レーンの範囲選択。検索ハイライトの後に重ねるので、重なった桁は選択側が勝つ
    pub selection: Option<&'a Selection>,
    /// ブロックカーソルの (論理行, 表示桁)
    pub cursor: Option<(usize, usize)>,
    /// 行番号 gutter (span[0]) の char 幅。wrap の続き行 pad と hscroll の除外幅に使う
    pub gutter_width: usize,
}

impl TextPane<'_> {
    /// viewport に収まる分の描画行を組み立てる。Paragraph::scroll / Paragraph::wrap は
    /// 使わない (u16 上限と、折返し位置が外から計算できない問題をどちらも避ける)
    pub fn visible(&self, vp: &Viewport) -> Vec<Line<'static>> {
        if vp.wrap {
            return self.wrapped(vp);
        }
        (0..self.window.rows.len())
            .take(vp.height)
            .map(|offset| {
                let line = self.marked_and_highlighted(offset);
                let line = hscroll_line(&line, vp.hscroll);
                match self.cursor {
                    Some((cursor_line, col)) if cursor_line == self.window.first + offset => {
                        overlay_cursor(line, col.saturating_sub(vp.hscroll))
                    }
                    _ => line,
                }
            })
            .collect()
    }

    // wrap 時の描画: 論理行を width セルずつに自前分割する。折返し位置を
    // カーソル追従 (editor の ensure_visible) とクリック座標 (click_at) の視覚行数
    // 計算と一致させるため、規則は text::WrapCursor 1 つに寄せ、単語境界 wrap は使わない
    fn wrapped(&self, vp: &Viewport) -> Vec<Line<'static>> {
        let width = vp.width.saturating_sub(self.gutter_width).max(1);
        let mut rows: Vec<Line> = Vec::new();
        for offset in 0..self.window.rows.len() {
            if rows.len() >= vp.height {
                break;
            }
            // マーカーは先頭の視覚行の gutter にだけ付く (続き行は pad で置き換わる)
            let line = self.marked_and_highlighted(offset);
            let mut chunks = wrap_line(&line, width, self.gutter_width);
            if let Some((cursor_line, col)) = self.cursor
                && cursor_line == self.window.first + offset
            {
                // 折返し位置は wrap_line と同じ規則 (text::WrapCursor) で引き直す。
                // 全角を含む行では「表示桁 / 幅」がその規則と一致しない
                let content: String = line
                    .spans
                    .iter()
                    .skip(1)
                    .map(|s| s.content.as_ref())
                    .collect();
                let (row, offset_in_row) = text::wrap_position(&content, col, width);
                // 折返し境界ちょうど (行末で行が埋まっている) に立った場合は空の続き行に置く
                while chunks.len() <= row {
                    chunks.push(Line::from(vec![pad_span(self.gutter_width)]));
                }
                chunks[row] = overlay_cursor(std::mem::take(&mut chunks[row]), offset_in_row);
            }
            rows.extend(chunks);
        }
        rows.truncate(vp.height);
        rows
    }

    // offset はウィンドウ内の位置。変更行マーク・検索マッチは文書全体での論理行
    // (first + offset) で引く
    fn marked_and_highlighted(&self, offset: usize) -> Line<'static> {
        let i = self.window.first + offset;
        let line = mark_changed_line(&self.window.rows[offset], i, self.changed_lines);
        let line = match self.search {
            Some(search) => highlight_matches(&line, i, search),
            None => line,
        };
        match self.selection.and_then(|sel| sel.columns_at(i)) {
            Some((start, end)) => highlight_selection(&line, start, end),
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

// 選択範囲 (絶対桁 [start, end)、end は行末までなら usize::MAX) に色を重ねた新しい Line を
// 返す。span[0] の gutter を対象外にするのは highlight_matches と同じ。選択が行末より右まで
// 伸びていても足りない桁は描かない (行の実際の長さより先に文字は無いため、右端は行なりに揃う)
fn highlight_selection(line: &Line<'static>, start: usize, end: usize) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 2);
    if let Some(gutter) = line.spans.first() {
        spans.push(gutter.clone());
    }
    let selected = Style::default().bg(SELECTION_BG).fg(SELECTION_FG);
    let mut col = 0usize;
    for span in line.spans.iter().skip(1) {
        // 交差判定に要るのは長さだけなので、ここではまだ Vec<char> を作らない。
        // 選択中の可視行ぶん毎フレーム通る経路なので、切らない span で確保しない
        // (再描画のコストを画面の大きさより上に持ち上げない、の一環)
        let len = span.content.chars().count();
        let span_end = col + len;
        // 選択と交差しない span はそのまま引き継ぐ (span 数を無駄に増やさない)
        if span_end <= start || col >= end {
            spans.push(span.clone());
            col = span_end;
            continue;
        }
        let chars: Vec<char> = span.content.chars().collect();
        let from = start.saturating_sub(col);
        let to = (end - col).min(len);
        push_segment(&mut spans, &chars[..from], span.style);
        push_segment(&mut spans, &chars[from..to], selected);
        push_segment(&mut spans, &chars[to..], span.style);
        col = span_end;
    }
    Line::from(spans)
}

fn push_segment(spans: &mut Vec<Span<'static>>, chars: &[char], style: Style) {
    if chars.is_empty() {
        return;
    }
    spans.push(Span::styled(chars.iter().collect::<String>(), style));
}

// 論理行 1 本を width セルごとの視覚行に切る。span の style は切れ目を跨いで保存する。
// 切る単位が char 数ではなくセル数なのは、端末 (ratatui の LineTruncator) が全角を
// 2 セル送るため — char 数で詰めると行が幅を超え、はみ出した文字が次の視覚行にも
// 現れないまま消える。span の内容は normalize 済み (タブ展開済み) なので、
// ここでは text::WrapCursor に char をそのまま食わせてよい
fn wrap_line(line: &Line<'static>, width: usize, gutter_width: usize) -> Vec<Line<'static>> {
    let mut rows: Vec<Line> = Vec::new();
    let mut spans: Vec<Span> = vec![line.spans.first().cloned().unwrap_or_default()];
    let mut wrap = text::WrapCursor::new(width);
    for span in line.spans.iter().skip(1) {
        let mut segment = String::new();
        for c in span.content.chars() {
            if wrap.push(c) {
                if !segment.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut segment), span.style));
                }
                rows.push(Line::from(std::mem::replace(
                    &mut spans,
                    vec![pad_span(gutter_width)],
                )));
            }
            segment.push(c);
        }
        if !segment.is_empty() {
            spans.push(Span::styled(segment, span.style));
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

#[cfg(test)]
mod tests {
    use super::{LineWindow, TextPane};
    use crate::component::viewer::Viewport;
    use crate::text;
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};

    fn pane_rows(
        spans: Vec<Span<'static>>,
        width: usize,
        gutter_width: usize,
    ) -> Vec<Line<'static>> {
        let rows = vec![Line::from(spans)];
        let mut vp = Viewport::new(true);
        vp.width = width;
        vp.height = 20;
        let pane = TextPane {
            window: LineWindow {
                rows: &rows,
                first: 0,
            },
            changed_lines: &None,
            search: None,
            selection: None,
            cursor: None,
            gutter_width,
        };
        pane.visible(&vp)
    }

    fn cells(line: &Line<'static>) -> usize {
        line.spans
            .iter()
            .flat_map(|s| s.content.chars())
            .map(text::char_cells)
            .sum()
    }

    fn content(rows: &[Line<'static>], gutter_width: usize) -> String {
        rows.iter()
            .map(|row| {
                let body: String = row
                    .spans
                    .iter()
                    .skip(1)
                    .map(|s| s.content.as_ref())
                    .collect();
                // 続き行の gutter は空白詰めなので、gutter を落とせば本文だけが残る
                debug_assert_eq!(row.spans[0].content.chars().count(), gutter_width);
                body
            })
            .collect()
    }

    // 全角文字は 2 セルを占めるので、char 数で詰めると視覚行が幅を超え、
    // はみ出した文字が次の視覚行にも現れないまま消える (#w の折返しバグ)
    #[test]
    fn wrapping_full_width_text_keeps_every_char() {
        let text = "あいうえおかきくけこ漢字テスト";
        let rows = pane_rows(
            vec![Span::raw("1 "), Span::raw(text)],
            12, // gutter 2 + 折返し幅 10 = 全角 5 文字ぶん
            2,
        );
        assert_eq!(rows.len(), 3);
        for row in &rows {
            assert!(cells(row) <= 12, "視覚行が幅を超えている: {row:?}");
        }
        assert_eq!(content(&rows, 2), text);
    }

    // 折返しは span の切れ目と無関係に起きるので、style を跨いでも本文は落ちない
    #[test]
    fn wrapping_splits_inside_a_span_and_keeps_styles() {
        let rows = pane_rows(
            vec![
                Span::raw("1 "),
                Span::styled("あa", Style::default().fg(Color::Red)),
                Span::styled("bいc", Style::default().fg(Color::Blue)),
            ],
            6, // 折返し幅 4
            2,
        );
        assert_eq!(content(&rows, 2), "あabいc");
        for row in &rows {
            assert!(cells(row) <= 6, "視覚行が幅を超えている: {row:?}");
        }
    }
}
