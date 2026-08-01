//! side-by-side 表示 (#30)。左右 2 本の Line 列を「対応行が同じ視覚行に来る」よう組み立て、
//! 折返し時の行数合わせ (side_by_side_wrapped) までをここに閉じる。
//! text_pane.rs には side-by-side 専用の分岐を足さないための受け皿。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::text;

use super::render::{
    blank_gutter, blank_row, content_spans, hunk_old_start, hunk_start, max_new_lineno,
    max_old_lineno, number_gutter,
};
use super::word::{run_end, word_diff_ranges};
use super::{ADDED, ADDED_WORD_BG, DELETED, DELETED_WORD_BG, HUNK, Kind, SideDiff};

/// unified diff の body から side-by-side (左 = 旧, 右 = 新) の 2 本の Line 列を組み立てる。
/// 削除行 = 左のみ・追加行 = 右のみ・文脈行と hunk header = 両方。削除→追加が連続する
/// ブロックは同じ視覚行に並べ、行数が合わない側は空行で埋める (issue #30 の要件)
pub(super) fn render_side_by_side(body: &[(Kind, &str)]) -> SideDiff {
    let left_gutter_width = text::gutter_width(max_old_lineno(body));
    let right_gutter_width = text::gutter_width(max_new_lineno(body));
    let word_ranges = word_diff_ranges(body);

    let mut left = Vec::with_capacity(body.len());
    let mut right = Vec::with_capacity(body.len());
    let mut hunks = Vec::new();
    let mut left_max_width = 0usize;
    let mut right_max_width = 0usize;
    let mut old_lineno = 0usize;
    let mut new_lineno = 0usize;

    let mut i = 0;
    while i < body.len() {
        match body[i].0 {
            Kind::Hunk => {
                let raw = body[i].1;
                old_lineno = hunk_old_start(raw).unwrap_or(old_lineno);
                new_lineno = hunk_start(raw).unwrap_or(new_lineno);
                hunks.push(left.len());
                let content = text::normalize(raw);
                left_max_width = left_max_width.max(content.chars().count());
                right_max_width = right_max_width.max(content.chars().count());
                let style = Style::default().fg(HUNK);
                left.push(Line::from(vec![
                    blank_gutter(left_gutter_width),
                    Span::styled(content.clone(), style),
                ]));
                right.push(Line::from(vec![
                    blank_gutter(right_gutter_width),
                    Span::styled(content, style),
                ]));
                i += 1;
            }
            Kind::Note => {
                let content = text::normalize(body[i].1);
                left_max_width = left_max_width.max(content.chars().count());
                right_max_width = right_max_width.max(content.chars().count());
                let style = Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM);
                left.push(Line::from(vec![
                    blank_gutter(left_gutter_width),
                    Span::styled(content.clone(), style),
                ]));
                right.push(Line::from(vec![
                    blank_gutter(right_gutter_width),
                    Span::styled(content, style),
                ]));
                i += 1;
            }
            Kind::Context => {
                let content = text::normalize(body[i].1);
                left_max_width = left_max_width.max(content.chars().count());
                right_max_width = right_max_width.max(content.chars().count());
                let style = Style::default();
                left.push(Line::from(vec![
                    number_gutter(old_lineno, left_gutter_width),
                    Span::styled(content.clone(), style),
                ]));
                right.push(Line::from(vec![
                    number_gutter(new_lineno, right_gutter_width),
                    Span::styled(content, style),
                ]));
                old_lineno += 1;
                new_lineno += 1;
                i += 1;
            }
            Kind::Deleted | Kind::Added => {
                // 連続する削除ブロック→直後の連続する追加ブロックを 1 組にして、
                // 大きい方の行数だけ視覚行を使う (足りない側は空行で埋める)
                let del_start = i;
                let del_end = run_end(body, del_start, |k| matches!(k, Kind::Deleted));
                let add_start = del_end;
                let add_end = run_end(body, add_start, |k| matches!(k, Kind::Added));
                let del_len = del_end - del_start;
                let add_len = add_end - add_start;
                let rows = del_len.max(add_len);
                for r in 0..rows {
                    if r < del_len {
                        let idx = del_start + r;
                        let content = text::normalize(body[idx].1);
                        left_max_width = left_max_width.max(content.chars().count());
                        let style = Style::default().fg(DELETED);
                        let mut spans = vec![number_gutter(old_lineno, left_gutter_width)];
                        spans.extend(content_spans(
                            &content,
                            style,
                            Some(DELETED_WORD_BG),
                            &word_ranges[idx],
                        ));
                        left.push(Line::from(spans));
                        old_lineno += 1;
                    } else {
                        left.push(blank_row(left_gutter_width));
                    }
                    if r < add_len {
                        let idx = add_start + r;
                        let content = text::normalize(body[idx].1);
                        right_max_width = right_max_width.max(content.chars().count());
                        let style = Style::default().fg(ADDED);
                        let mut spans = vec![number_gutter(new_lineno, right_gutter_width)];
                        spans.extend(content_spans(
                            &content,
                            style,
                            Some(ADDED_WORD_BG),
                            &word_ranges[idx],
                        ));
                        right.push(Line::from(spans));
                        new_lineno += 1;
                    } else {
                        right.push(blank_row(right_gutter_width));
                    }
                }
                i = add_end;
            }
        }
    }

    SideDiff {
        left,
        right,
        hunks,
        left_gutter_width,
        right_gutter_width,
        left_max_width,
        right_max_width,
    }
}

/// side-by-side + wrap 描画専用。text_pane.rs の非公開 wrap_line と同じ char 単位分割を
/// カラムごとの幅で行った上で、行ごとに「左右の視覚行数の大きい方」に空行を足して
/// 総行数を揃える (揃えないと折返しで左右の対応行がズレる)。ここで作った行は非 wrap の
/// TextPane にそのまま渡す想定で、side-by-side 専用の分岐を text_pane.rs には増やさない。
/// hunks は揃えた後の行 index に付け替えて返す (n/N が正しい行へジャンプできるように)
pub fn side_by_side_wrapped(
    left: &[Line<'static>],
    right: &[Line<'static>],
    hunks: &[usize],
    left_gutter_width: usize,
    right_gutter_width: usize,
    column_width: usize,
) -> (Vec<Line<'static>>, Vec<Line<'static>>, Vec<usize>) {
    let lw = column_width.saturating_sub(left_gutter_width).max(1);
    let rw = column_width.saturating_sub(right_gutter_width).max(1);
    let mut out_left = Vec::with_capacity(left.len());
    let mut out_right = Vec::with_capacity(right.len());
    let mut out_hunks = Vec::with_capacity(hunks.len());
    let mut hunk_iter = hunks.iter().peekable();
    for (i, (l, r)) in left.iter().zip(right.iter()).enumerate() {
        if hunk_iter.peek() == Some(&&i) {
            out_hunks.push(out_left.len());
            hunk_iter.next();
        }
        let mut lchunks = wrap_split(l, lw, left_gutter_width);
        let mut rchunks = wrap_split(r, rw, right_gutter_width);
        while lchunks.len() < rchunks.len() {
            lchunks.push(blank_row(left_gutter_width));
        }
        while rchunks.len() < lchunks.len() {
            rchunks.push(blank_row(right_gutter_width));
        }
        out_left.extend(lchunks);
        out_right.extend(rchunks);
    }
    (out_left, out_right, out_hunks)
}

// text_pane.rs の wrap_line と同じ char 単位分割 (span の style は境界を跨いで保持する)。
// side-by-side はカラムごとに幅・gutter 幅が違う独立した 2 本のドキュメントを同時に扱うため
// text_pane 側の (1 本の Line 列前提の) wrap をそのまま呼べず、ここに複製している
fn wrap_split(line: &Line<'static>, width: usize, gutter_width: usize) -> Vec<Line<'static>> {
    let mut rows: Vec<Line> = Vec::new();
    let mut spans: Vec<Span> = vec![
        line.spans
            .first()
            .cloned()
            .unwrap_or_else(|| blank_gutter(gutter_width)),
    ];
    let mut used = 0usize;
    for span in line.spans.iter().skip(1) {
        let chars: Vec<char> = span.content.chars().collect();
        let mut idx = 0;
        while idx < chars.len() {
            let take = (width - used).min(chars.len() - idx);
            if take == 0 {
                rows.push(Line::from(std::mem::replace(
                    &mut spans,
                    vec![blank_gutter(gutter_width)],
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
