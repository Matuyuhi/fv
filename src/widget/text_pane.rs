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
// GIT レーンの diff ペインの行カーソル・行単位選択の帯。char 単位の cursor (REVERSED) と
// 違って行全体を塗るので、diff の赤/緑の前景色を潰さないよう背景だけを差し替える。
// word-level ハイライトや検索マッチが既に付けた背景はそのまま残す (情報を消さない)
const FOCUS_ROW_BG: Color = Color::Rgb(62, 74, 102);
const SELECTED_ROW_BG: Color = Color::Rgb(44, 54, 78);

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
/// 行加工順は mark_changed_gutter → highlight_matches → highlight_selection → tint_row →
/// (hscroll | セル単位 wrap) → cursor overlay で固定。順序を入れ替えると検索マッチ・
/// 選択範囲・カーソルの絶対桁がズレる (CLAUDE.md の桁位置整合インバリアント)。
/// 閲覧は search/selection だけ、編集は cursor だけを Some にする —
/// 両方を同時に使うモードはない。
///
/// 各段は `Vec<Span>` を**受け取って返す**形にしてあり、加工しない span は中身を
/// 複製せず借りたまま (borrowed) / 所有したまま (move) 引き継ぐ。組み立て済みの行は
/// 描画のたびに読むだけなので、段ごとに Line を deep clone すると 1 フレームの確保が
/// 「可視行数 × span 数」に比例して積み上がる (再描画のコストを画面の大きさより上に
/// 持ち上げない、の一環)
pub(crate) struct TextPane<'a> {
    pub window: LineWindow<'a>,
    pub changed_lines: &'a Option<HashSet<usize>>,
    pub search: Option<&'a SearchState>,
    /// VIEW レーンの範囲選択。検索ハイライトの後に重ねるので、重なった桁は選択側が勝つ
    pub selection: Option<&'a Selection>,
    /// ブロックカーソルの (論理行, 表示桁)
    pub cursor: Option<(usize, usize)>,
    /// 行カーソル (diff ペイン)。char 単位の `cursor` とは別物で、行全体を帯にする
    pub focus_row: Option<usize>,
    /// 行単位選択 (両端含む)。focus_row より弱い色で塗り、重なる行は focus_row が勝つ
    pub selected_rows: Option<(usize, usize)>,
    /// 行番号 gutter (span[0]) の char 幅。wrap の続き行 pad と hscroll の除外幅に使う
    pub gutter_width: usize,
}

impl<'a> TextPane<'a> {
    /// viewport に収まる分の描画行を組み立てる。Paragraph::scroll / Paragraph::wrap は
    /// 使わない (u16 上限と、折返し位置が外から計算できない問題をどちらも避ける)
    pub fn visible(&self, vp: &Viewport) -> Vec<Line<'a>> {
        if vp.wrap {
            return self.wrapped(vp);
        }
        (0..self.window.rows.len())
            .take(vp.height)
            .map(|offset| {
                let spans = self.marked_and_highlighted(offset);
                let spans = hscroll_spans(spans, vp.hscroll);
                match self.cursor {
                    Some((cursor_line, col)) if cursor_line == self.window.first + offset => {
                        overlay_cursor(spans, col.saturating_sub(vp.hscroll))
                    }
                    _ => Line::from(spans),
                }
            })
            .collect()
    }

    // wrap 時の描画: 論理行を width セルずつに自前分割する。折返し位置を
    // カーソル追従 (editor の ensure_visible) とクリック座標 (click_at) の視覚行数
    // 計算と一致させるため、規則は text::WrapCursor 1 つに寄せ、単語境界 wrap は使わない
    fn wrapped(&self, vp: &Viewport) -> Vec<Line<'a>> {
        let width = vp.width.saturating_sub(self.gutter_width).max(1);
        let mut rows: Vec<Line<'a>> = Vec::new();
        for offset in 0..self.window.rows.len() {
            if rows.len() >= vp.height {
                break;
            }
            // マーカーは先頭の視覚行の gutter にだけ付く (続き行は pad で置き換わる)
            let spans = self.marked_and_highlighted(offset);
            let mut chunks = wrap_line(&spans, width, self.gutter_width);
            if let Some((cursor_line, col)) = self.cursor
                && cursor_line == self.window.first + offset
            {
                // 折返し位置は wrap_line と同じ規則 (text::WrapCursor) で引き直す。
                // 全角を含む行では「表示桁 / 幅」がその規則と一致しない
                let content: String = spans.iter().skip(1).map(|s| s.content.as_ref()).collect();
                let (row, offset_in_row) = text::wrap_position(&content, col, width);
                // 折返し境界ちょうど (行末で行が埋まっている) に立った場合は空の続き行に置く
                while chunks.len() <= row {
                    chunks.push(Line::from(vec![pad_span(
                        self.gutter_width,
                        self.row_band(self.window.first + offset),
                    )]));
                }
                let taken = std::mem::take(&mut chunks[row]).spans;
                chunks[row] = overlay_cursor(taken, offset_in_row);
            }
            rows.extend(chunks);
        }
        rows.truncate(vp.height);
        rows
    }

    // offset はウィンドウ内の位置。変更行マーク・検索マッチは文書全体での論理行
    // (first + offset) で引く。キャッシュ済みの行は借りたまま Vec に写し取り、
    // 実際に手を入れる段だけが中身を作り直す
    fn marked_and_highlighted(&self, offset: usize) -> Vec<Span<'a>> {
        let i = self.window.first + offset;
        let mut spans: Vec<Span<'a>> = self.window.rows[offset]
            .spans
            .iter()
            .map(borrowed)
            .collect();
        if self
            .changed_lines
            .as_ref()
            .is_some_and(|lines| lines.contains(&(i + 1)))
        {
            mark_changed_gutter(&mut spans);
        }
        if let Some(search) = self.search {
            spans = highlight_matches(spans, i, search);
        }
        if let Some((start, end)) = self.selection.and_then(|sel| sel.columns_at(i)) {
            spans = highlight_selection(spans, start, end);
        }
        // 帯は最後に重ねる。wrap 中も marked_and_highlighted の後で行を切るので、
        // 折返した続き行にも同じ背景がそのまま乗る
        if let Some(bg) = self.row_band(i) {
            tint_row(&mut spans, bg);
        }
        spans
    }

    // その論理行に敷く帯の色。カーソル行が選択範囲の中にあるときはカーソル側を優先する
    fn row_band(&self, line: usize) -> Option<Color> {
        if self.focus_row == Some(line) {
            return Some(FOCUS_ROW_BG);
        }
        match self.selected_rows {
            Some((from, to)) if (from..=to).contains(&line) => Some(SELECTED_ROW_BG),
            _ => None,
        }
    }
}

/// 帯の色が付いた行をペイン幅まで伸ばして、行末まで続く 1 本の帯に見せる
/// (widget/diff_boundary.rs::widen_boundary_bands と同じ、描画側だけの後加工)。
/// 折返しの続き行は gutter を素の空白 (pad_span) で埋めるため帯が途切れる —
/// 帯色を持つ span が 1 つでもあれば、その行の未着色の span もここで塗り直す
pub(crate) fn widen_row_bands(rows: &mut [Line<'_>], width: usize) {
    for row in rows.iter_mut() {
        let Some(bg) = row.spans.iter().find_map(|s| {
            s.style
                .bg
                .filter(|c| matches!(*c, FOCUS_ROW_BG | SELECTED_ROW_BG))
        }) else {
            continue;
        };
        for span in row.spans.iter_mut() {
            if span.style.bg.is_none() {
                span.style = span.style.bg(bg);
            }
        }
        // 埋める量は char 数ではなく**セル幅**で測る (CLAUDE.md の桁インバリアント)。
        // 全角を 1 桁と数えると余計に詰めて行がペイン幅を超え、ZWJ 絵文字のように
        // char 数が描画幅より多い列では逆に足りず、帯が右端まで届かない。
        // Line::width は span ごとに text::cells と同じ測り方をするので描画と一致する
        let used = row.width();
        if used < width {
            row.spans.push(Span::styled(
                " ".repeat(width - used),
                Style::default().bg(bg),
            ));
        }
    }
}

/// gutter (span[0]) を除いた本文。「span[1..] を連結すると本文に戻る」という桁インバリアント
/// (CLAUDE.md) の読み出し側で、diff ペインの検索・折返し行数・クリック座標が共有する。
/// 不変条件を持っているのがこのファイルなので、取り出し口もここに 1 つだけ置く
pub(crate) fn line_body(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .skip(1)
        .map(|s| s.content.as_ref())
        .collect()
}

/// キャッシュ済みの span を、中身を複製せずに借りたまま写し取る。組み立て済みの行は
/// 描画のたびに読むだけなので、ここで String を作り直すと確保が可視行数 × span 数に比例する
fn borrowed<'a>(span: &'a Span<'_>) -> Span<'a> {
    Span::styled(span.content.as_ref(), span.style)
}

// 行全体に帯の背景を敷く。既に背景を持つ span (word-level 差分・検索マッチ) は
// そのまま残す — 帯で塗り潰すと「どの文字が変わったのか」が読めなくなるため。
// 変えるのは style だけなので本文には触れない
fn tint_row(spans: &mut [Span<'_>], bg: Color) {
    for span in spans.iter_mut().filter(|s| s.style.bg.is_none()) {
        span.style = span.style.bg(bg);
    }
}

// gutter span (span[0]) の末尾1文字 (常に半角空白) を変更行マーカーに置き換える。
// span 数・各 span の文字数はどちらも変わらないため、highlight_matches の列計算
// (span[0] を除外して col=0 から数える) には影響しない
fn mark_changed_gutter(spans: &mut [Span<'_>]) {
    let Some(gutter) = spans.first_mut() else {
        return;
    };
    let mut text = gutter.content.to_string();
    text.pop();
    text.push('▎');
    *gutter = Span::styled(text, Style::default().fg(Color::Cyan));
}

// gutter (span[0]) は固定したまま、コンテンツ部分だけ hscroll 文字分左にシフトする。
// highlight_matches 適用後に呼ぶことで、シフトで画面外に落ちる文字ごとその bg スタイルも
// 一緒に捨てられ、残った文字のハイライトは桁がずれず正しく残る。
// 切れない span は作り直さずそのまま持ち越す
fn hscroll_spans<'a>(spans: Vec<Span<'a>>, hscroll: usize) -> Vec<Span<'a>> {
    if hscroll == 0 {
        return spans;
    }
    let mut out = Vec::with_capacity(spans.len());
    let mut iter = spans.into_iter();
    if let Some(gutter) = iter.next() {
        out.push(gutter);
    }
    let mut col = 0usize;
    for span in iter {
        let len = span.content.chars().count();
        let span_end = col + len;
        if span_end <= hscroll {
            // span 全体が切り捨て範囲に収まる場合は丸ごと捨てる
            col = span_end;
            continue;
        }
        if col >= hscroll {
            // 切り捨て範囲より右にある span は丸ごと残る
            out.push(span);
            col = span_end;
            continue;
        }
        let segment: String = span.content.chars().skip(hscroll - col).collect();
        out.push(Span::styled(segment, span.style));
        col = span_end;
    }
    out
}

// span 列に検索マッチの背景色を重ねる。マッチと交差しない span は切る必要がないので
// そのまま持ち越す (span 数を無駄に増やさず、本文も作り直さない)
fn highlight_matches<'a>(
    spans: Vec<Span<'a>>,
    line_idx: usize,
    search: &SearchState,
) -> Vec<Span<'a>> {
    let ranges: Vec<(usize, usize, bool)> = search
        .matches
        .iter()
        .enumerate()
        .filter(|(_, m)| m.line == line_idx)
        .map(|(i, m)| (m.start_col, m.end_col, Some(i) == search.current))
        .collect();
    if ranges.is_empty() {
        return spans;
    }

    let mut out = Vec::with_capacity(spans.len());
    let mut iter = spans.into_iter();
    // span[0] は行番号 gutter なのでハイライト対象から除外し、そのまま引き継ぐ
    if let Some(gutter) = iter.next() {
        out.push(gutter);
    }

    let mut col = 0usize;
    for span in iter {
        let len = span.content.chars().count();
        let span_end = col + len;
        // どのマッチとも交差しない span は切らずにそのまま渡す
        if !ranges.iter().any(|(s, e, _)| *s < span_end && col < *e) {
            out.push(span);
            col = span_end;
            continue;
        }
        let chars: Vec<char> = span.content.chars().collect();
        let mut idx = 0usize;
        while idx < chars.len() {
            let global = col + idx;
            match ranges.iter().find(|(s, e, _)| *s <= global && global < *e) {
                Some(&(_, end, current)) => {
                    let seg_end = (end - col).min(chars.len());
                    let bg = if current { CURRENT_MATCH_BG } else { MATCH_BG };
                    let mut style = span.style.bg(bg);
                    if current {
                        style = style.fg(Color::Black);
                    }
                    push_segment(&mut out, &chars[idx..seg_end], style);
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
                    push_segment(&mut out, &chars[idx..seg_end], span.style);
                    idx = seg_end;
                }
            }
        }
        col = span_end;
    }
    out
}

// 選択範囲 (絶対桁 [start, end)、end は行末までなら usize::MAX) に色を重ねる。
// span[0] の gutter を対象外にするのは highlight_matches と同じ。選択が行末より右まで
// 伸びていても足りない桁は描かない (行の実際の長さより先に文字は無いため、右端は行なりに揃う)
fn highlight_selection<'a>(spans: Vec<Span<'a>>, start: usize, end: usize) -> Vec<Span<'a>> {
    let mut out: Vec<Span<'a>> = Vec::with_capacity(spans.len() + 2);
    let mut iter = spans.into_iter();
    if let Some(gutter) = iter.next() {
        out.push(gutter);
    }
    let selected = Style::default().bg(SELECTION_BG).fg(SELECTION_FG);
    let mut col = 0usize;
    for span in iter {
        // 交差判定に要るのは長さだけなので、ここではまだ Vec<char> を作らない。
        // 選択中の可視行ぶん毎フレーム通る経路なので、切らない span で確保しない
        // (再描画のコストを画面の大きさより上に持ち上げない、の一環)
        let len = span.content.chars().count();
        let span_end = col + len;
        // 選択と交差しない span はそのまま引き継ぐ (span 数を無駄に増やさない)
        if span_end <= start || col >= end {
            out.push(span);
            col = span_end;
            continue;
        }
        let chars: Vec<char> = span.content.chars().collect();
        let from = start.saturating_sub(col);
        let to = (end - col).min(len);
        push_segment(&mut out, &chars[..from], span.style);
        push_segment(&mut out, &chars[from..to], selected);
        push_segment(&mut out, &chars[to..], span.style);
        col = span_end;
    }
    out
}

fn push_segment(spans: &mut Vec<Span<'_>>, chars: &[char], style: Style) {
    if chars.is_empty() {
        return;
    }
    spans.push(Span::styled(chars.iter().collect::<String>(), style));
}

// 論理行 1 本を width セルごとの視覚行に切る。span の style は切れ目を跨いで保存する。
// 切る単位が char 数ではなくセル数なのは、端末 (ratatui の LineTruncator) が全角を
// 2 セル送るため — char 数で詰めると行が幅を超え、はみ出した文字が次の視覚行にも
// 現れないまま消える。走査も char ではなく grapheme 単位にするのは、ZWJ 絵文字の
// ように「char ごとの幅の合計と実際の描画幅が食い違う」列を割らないため。
// span の内容は normalize 済み (タブ展開済み) なのでタブの手当ては要らない
fn wrap_line<'a>(line: &[Span<'a>], width: usize, gutter_width: usize) -> Vec<Line<'a>> {
    let mut rows: Vec<Line<'a>> = Vec::new();
    let mut spans: Vec<Span<'a>> = vec![line.first().cloned().unwrap_or_default()];
    // 続き行の gutter pad にも元の gutter の背景を引き継ぐ。帯 (focus_row/selected_rows) は
    // tint_row が gutter にも塗っているので、これで折返しても帯が途切れず、widen_row_bands が
    // 「この視覚行は帯付き」を gutter だけで判定できる (本文の span が全て word-level 差分や
    // 検索マッチの背景を持つ続き行でも取りこぼさない)
    let pad = pad_span(gutter_width, line.first().and_then(|g| g.style.bg));
    let mut wrap = text::WrapCursor::new(width);
    for span in line.iter().skip(1) {
        let mut segment = String::new();
        // ratatui の描画と同じ単位で送るため grapheme で辿る (ZWJ 絵文字を割らない)
        for grapheme in span.styled_graphemes(Style::default()) {
            if wrap.push(grapheme.symbol) {
                if !segment.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut segment), span.style));
                }
                rows.push(Line::from(std::mem::replace(&mut spans, vec![pad.clone()])));
            }
            segment.push_str(grapheme.symbol);
        }
        if !segment.is_empty() {
            spans.push(Span::styled(segment, span.style));
        }
    }
    rows.push(Line::from(spans));
    rows
}

// 続き行の gutter 部分を空白で埋めて桁を揃える。bg は元の gutter から引き継ぐ (帯の連続用)
fn pad_span(gutter_width: usize, bg: Option<Color>) -> Span<'static> {
    let text = " ".repeat(gutter_width);
    match bg {
        Some(bg) => Span::styled(text, Style::default().bg(bg)),
        None => Span::raw(text),
    }
}

// コンテンツ部 (span[0] の gutter を除く) の col 文字目に REVERSED を重ねた
// ブロックカーソルを見せる。行末より先なら REVERSED 空白を足す。
// 端末カーソルでなく文字スタイルにするのは、全角・タブの画面幅計算を避けるため
fn overlay_cursor<'a>(line: Vec<Span<'a>>, col: usize) -> Line<'a> {
    let mut spans: Vec<Span<'a>> = Vec::with_capacity(line.len() + 2);
    let mut iter = line.into_iter();
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
        let content = span.content;
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
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};

    fn pane_rows(
        spans: Vec<Span<'static>>,
        width: usize,
        gutter_width: usize,
    ) -> Vec<Line<'static>> {
        banded_pane_rows(spans, width, gutter_width, None)
    }

    fn banded_pane_rows(
        spans: Vec<Span<'static>>,
        width: usize,
        gutter_width: usize,
        focus_row: Option<usize>,
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
            focus_row,
            selected_rows: None,
            gutter_width,
        };
        // visible は window から借りたまま返すので、rows より長生きさせるためここで所有に移す
        pane.visible(&vp)
            .into_iter()
            .map(|row| {
                Line::from(
                    row.spans
                        .into_iter()
                        .map(|span| Span::styled(span.content.into_owned(), span.style))
                        .collect::<Vec<_>>(),
                )
            })
            .collect()
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
            assert!(row.width() <= 12, "視覚行が幅を超えている: {row:?}");
        }
        assert_eq!(content(&rows, 2), text);
    }

    // ZWJ 絵文字は char ごとの幅の合計 (4) と描画幅 (2) が食い違う。char で数えると
    // 幅 4 の行に収まらないと誤判定して列の途中で割れ、絵文字が 2 つに分かれて見える
    #[test]
    fn wrapping_keeps_a_zwj_sequence_whole() {
        let text = "👩\u{200d}💻ab";
        let rows = pane_rows(vec![Span::raw("1 "), Span::raw(text)], 6, 2);
        assert_eq!(rows.len(), 1);
        assert_eq!(content(&rows, 2), text);
        assert!(
            rows[0].width() <= 6,
            "視覚行が幅を超えている: {:?}",
            rows[0]
        );
    }

    // 帯をペイン幅まで伸ばす量は char 数ではなくセル幅で測る。全角を 1 桁と数えると
    // 詰めすぎて行が幅を超え、ZWJ 絵文字 (char 4 個で描画幅 2) では足りずに右端が空く
    #[test]
    fn widening_a_band_measures_cells_not_chars() {
        for (body, label) in [("あいう", "全角"), ("👩\u{200d}💻ab", "ZWJ")] {
            let rows = banded_pane_rows(vec![Span::raw("1 "), Span::raw(body)], 20, 2, Some(0));
            let mut rows = rows;
            super::widen_row_bands(&mut rows, 20);
            assert_eq!(rows[0].width(), 20, "{label}: 帯の幅がペイン幅と一致しない");
        }
    }

    // 折返しの続き行は gutter が pad に差し替わる。本文の span が全て背景を持っている
    // (word-level 差分・検索マッチで埋まっている) 行だと、pad が無色のままでは
    // widen_row_bands が「この視覚行は帯付き」を判定できず、帯が続き行だけ消える
    #[test]
    fn a_wrapped_focus_band_survives_on_continuation_rows() {
        let body = Style::default().bg(Color::Rgb(20, 90, 20));
        let rows = banded_pane_rows(
            vec![Span::raw("1 "), Span::styled("abcdefghij", body)],
            6, // gutter 2 + 折返し幅 4 → 3 視覚行
            2,
            Some(0),
        );
        assert!(rows.len() > 1, "折返していない: {rows:?}");
        let mut rows = rows;
        super::widen_row_bands(&mut rows, 6);
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(
                row.spans[0].style.bg,
                Some(super::FOCUS_ROW_BG),
                "視覚行 {i} の gutter に帯が乗っていない: {row:?}"
            );
            assert_eq!(row.width(), 6, "帯がペイン幅まで伸びていない: {row:?}");
        }
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
            assert!(row.width() <= 6, "視覚行が幅を超えている: {row:?}");
        }
    }
}
