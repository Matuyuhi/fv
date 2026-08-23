//! 生の unified diff を TextPane が描ける Line 列へ組み替える (inline 表示)。
//! 単一ファイル (render_inline) と複数ファイル (render_commit) で入口は分かれるが、
//! 1 行単位の組み立て (build_body) と分類 (classify) は共有する。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::component::editor::diff::CharRanges;
use crate::text;

use super::word::word_diff_ranges;
use super::{ADDED, ADDED_WORD_BG, CommitRender, DELETED, DELETED_WORD_BG, HUNK, InlineDiff, Kind};

pub(super) fn render_inline(body: &[(Kind, &str)]) -> InlineDiff {
    // gutter 幅は行番号の最大桁で決まるので、行番号を振る前に一度だけ走査して求める
    let max_lineno = max_new_lineno(body);
    let gutter_width = text::gutter_width(max_lineno);
    let (lines, hunks, max_width) = build_body(body, gutter_width);
    InlineDiff {
        lines,
        hunks,
        gutter_width,
        max_width,
    }
}

/// LOG レーンの複数ファイル diff (`git show`) 用。単一ファイルの `render_inline` とはコミット
/// メッセージ・ファイル境界ヘッダの組み立てが増える分だけ別ルートにするが、1 行単位の
/// 組み立て (classify・number_gutter 等) はそのまま共有する。gutter 幅は全ファイル共通の
/// 1 つに揃える (TextPane の wrap 幅計算はパネル単位で単一の gutter_width を前提にしており、
/// ファイルごとに違う幅を使うと折返し位置がずれるため)
///
/// 戻り値の最後の `Vec<(usize, String)>` はファイル境界 (#40 sticky header 用):
/// ファイル見出し行の index → 表示ラベル。既存の 4 要素の意味・生成ロジックはそのまま
/// (呼び出し側で追加的に使うだけの情報なので、行の組み立て自体には手を入れない)
pub fn render_commit(raw: &[String]) -> CommitRender {
    let diff_start = raw
        .iter()
        .position(|l| l.starts_with("diff --git "))
        .unwrap_or(raw.len());
    let header = &raw[..diff_start];
    let segments = split_segments(&raw[diff_start..]);

    let bodies: Vec<Vec<(Kind, &str)>> = segments.iter().map(|seg| classify_body(seg)).collect();
    let max_lineno = bodies.iter().map(|b| max_new_lineno(b)).max().unwrap_or(0);
    let gutter_width = text::gutter_width(max_lineno);

    let mut lines = Vec::new();
    let mut hunks = Vec::new();
    let mut max_width = 0usize;
    // ファイル境界: 見出し行を push する直前の index がその行番号 (#40)
    let mut boundaries = Vec::new();

    for raw_line in header {
        let content = text::normalize(raw_line);
        max_width = max_width.max(content.chars().count());
        lines.push(Line::from(vec![
            blank_gutter(gutter_width),
            Span::styled(content, Style::default().fg(Color::Gray)),
        ]));
    }
    if !header.is_empty() && !segments.is_empty() {
        lines.push(Line::from(vec![blank_gutter(gutter_width)]));
    }

    for (segment, body) in segments.iter().zip(bodies) {
        let label = segment_label(segment);
        boundaries.push((lines.len(), label.clone()));
        max_width = max_width.max(label.chars().count() + 3);
        lines.push(Line::from(vec![
            blank_gutter(gutter_width),
            Span::styled(
                format!("── {label} "),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        let (body_lines, body_hunks, body_max_width) = build_body(&body, gutter_width);
        let offset = lines.len();
        hunks.extend(body_hunks.into_iter().map(|h| h + offset));
        max_width = max_width.max(body_max_width);
        lines.extend(body_lines);
    }

    (lines, hunks, gutter_width, max_width, boundaries)
}

// classify 済みの行を Line 列へ組み立てる。gutter_width は呼び出し側が (単一ファイルなら
// その diff だけの、複数ファイルなら全体で揃えた) 幅を渡す。lineno は呼び出しごとにリセット
// される (ファイルをまたいで行番号を通し番号にする意味がないため)
fn build_body(
    body: &[(Kind, &str)],
    gutter_width: usize,
) -> (Vec<Line<'static>>, Vec<usize>, usize) {
    // word-level の対応付けは hunk 内で閉じているので、単一ファイル (render_inline) でも
    // ファイル単位に分割済みの複数ファイル diff (render_commit) でも同じ扱いで済む
    let word_ranges = word_diff_ranges(body);

    let mut lines = Vec::with_capacity(body.len());
    let mut hunks = Vec::new();
    let mut max_width = 0usize;
    let mut lineno = 0usize;
    for (i, (kind, raw)) in body.iter().enumerate() {
        let content = text::normalize(raw);
        max_width = max_width.max(content.chars().count());
        // word_bg は Added/Deleted のみ Some。word-level 差分が取れた行だけ実際に分割する
        let (gutter, style, word_bg) = match kind {
            Kind::Hunk => {
                hunks.push(lines.len());
                lineno = hunk_start(raw).unwrap_or(lineno);
                (blank_gutter(gutter_width), Style::default().fg(HUNK), None)
            }
            Kind::Added => {
                let g = number_gutter(lineno, gutter_width);
                lineno += 1;
                (g, Style::default().fg(ADDED), Some(ADDED_WORD_BG))
            }
            Kind::Deleted => (
                blank_gutter(gutter_width),
                Style::default().fg(DELETED),
                Some(DELETED_WORD_BG),
            ),
            Kind::Context => {
                let g = number_gutter(lineno, gutter_width);
                lineno += 1;
                (g, Style::default(), None)
            }
            Kind::Note => (
                blank_gutter(gutter_width),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
                None,
            ),
        };
        let mut spans = Vec::with_capacity(2);
        spans.push(gutter);
        spans.extend(content_spans(&content, style, word_bg, &word_ranges[i]));
        lines.push(Line::from(spans));
    }
    (lines, hunks, max_width)
}

// "diff --git " 行を境界に生行をファイルごとの区間へ分割する (先頭行は必ず "diff --git ")
fn split_segments(raw: &[String]) -> Vec<&[String]> {
    let mut segments = Vec::new();
    let mut start = 0usize;
    for (i, line) in raw.iter().enumerate() {
        if i > start && line.starts_with("diff --git ") {
            segments.push(&raw[start..i]);
            start = i;
        }
    }
    if start < raw.len() {
        segments.push(&raw[start..]);
    }
    segments
}

// diff --git ヘッダ群からファイル境界の見出し文字列を作る。rename は "old → new"、
// 新規/削除ファイルは git の一般的な表記に合わせて "(new)" / "(deleted)" を添える
fn segment_label(segment: &[String]) -> String {
    let rename_from = segment.iter().find_map(|l| l.strip_prefix("rename from "));
    let rename_to = segment.iter().find_map(|l| l.strip_prefix("rename to "));
    if let (Some(from), Some(to)) = (rename_from, rename_to) {
        return format!("{from} → {to}");
    }
    let is_new = segment.iter().any(|l| l.starts_with("new file mode "));
    let is_deleted = segment.iter().any(|l| l.starts_with("deleted file mode "));
    let path = segment
        .iter()
        .find_map(|l| l.strip_prefix("+++ b/"))
        .or_else(|| segment.iter().find_map(|l| l.strip_prefix("--- a/")))
        .or_else(|| {
            segment
                .first()
                .and_then(|l| l.strip_prefix("diff --git a/"))
                .and_then(|s| s.split(" b/").nth(1))
        })
        .unwrap_or("?")
        .to_string();
    if is_new {
        format!("{path} (new)")
    } else if is_deleted {
        format!("{path} (deleted)")
    } else {
        path
    }
}

// content (gutter を除いた本文) に word-level 差分の色を重ねる。word_bg/word_range の
// どちらかが無ければ (word-level が取れなかった・Context/Hunk/Note 行) 単一 span のまま返す。
// 単一ファイルの inline (build_body) と side-by-side (render_side_by_side) の両方で使う
pub(super) fn content_spans(
    content: &str,
    style: Style,
    word_bg: Option<Color>,
    word_range: &Option<CharRanges>,
) -> Vec<Span<'static>> {
    match (word_bg, word_range) {
        (Some(bg), Some(range)) => split_with_emphasis(content, style, bg, range),
        _ => vec![Span::styled(content.to_string(), style)],
    }
}

// content (gutter を除いた本文) を word-level 差分の char range で複数 span に割る。
// 範囲内は前景色そのまま背景だけ濃くする。range 同士・前後の隙間は必ず埋めるので、
// span を連結すれば content に戻る (「span[1..] を連結すると本文」の前提を保つ)
fn split_with_emphasis(
    content: &str,
    style: Style,
    emphasis_bg: Color,
    ranges: &[(usize, usize)],
) -> Vec<Span<'static>> {
    if ranges.is_empty() {
        return vec![Span::styled(content.to_string(), style)];
    }
    let chars: Vec<char> = content.chars().collect();
    let mut spans = Vec::new();
    let mut pos = 0usize;
    for &(start, end) in ranges {
        let start = start.min(chars.len());
        let end = end.min(chars.len());
        if start > pos {
            spans.push(Span::styled(
                chars[pos..start].iter().collect::<String>(),
                style,
            ));
        }
        if end > start {
            spans.push(Span::styled(
                chars[start..end].iter().collect::<String>(),
                style.bg(emphasis_bg),
            ));
        }
        pos = pos.max(end);
    }
    if pos < chars.len() {
        spans.push(Span::styled(chars[pos..].iter().collect::<String>(), style));
    }
    spans
}

/// 1 ファイル分の生 diff を表示用に分類する。`in_hunk` の管理をここに閉じ、呼び出し側
/// (GitState::open / render_commit) が同じ規則を 2 度書かないようにする
pub(super) fn classify_body(raw: &[String]) -> Vec<(Kind, &str)> {
    classify_indexed(raw).into_iter().map(|(_, e)| e).collect()
}

/// classify_body に「元の raw の index」を添えたもの。行単位ステージ (Enter) が表示行から
/// 生 diff の行へ戻るのに使う (GitState::raw_index)。同じ 1 つの走査から作ることで、
/// 表示と index のズレを構造的に防ぐ
pub(super) fn classify_indexed(raw: &[String]) -> Vec<(usize, (Kind, &str))> {
    let mut in_hunk = false;
    let mut out = Vec::with_capacity(raw.len());
    for (i, line) in raw.iter().enumerate() {
        let Some(entry) = classify(line, in_hunk) else {
            continue;
        };
        if matches!(entry.0, Kind::Hunk) {
            in_hunk = true;
        }
        out.push((i, entry));
    }
    out
}

// diff --git / index / --- / +++ 等のヘッダは落とす。
// **`--- ` / `+++ ` を落とすのは最初の `@@` より前だけ** — hunk の中では、`-- ` で始まる行の
// 削除 (SQL/Haskell のコメント、markdown の `---` 等) が diff 上で `--- ` として現れる。
// 位置を見ずに落とすとその行が diff から丸ごと消え、表示にも出ず Enter の対象にもできない。
// 他のヘッダ (diff --git / index / mode / rename) は hunk 行が必ず ' '/'+'/'-'/'\' で始まる以上
// 本文と衝突しようがないので、位置に関わらず落として構わない
pub(super) fn classify(line: &str, in_hunk: bool) -> Option<(Kind, &str)> {
    if line.starts_with("@@") {
        return Some((Kind::Hunk, line));
    }
    if !in_hunk && (line.starts_with("--- ") || line.starts_with("+++ ")) {
        return None;
    }
    if line.starts_with("diff --git")
        || line.starts_with("index ")
        || line.starts_with("old mode ")
        || line.starts_with("new mode ")
        || line.starts_with("new file mode ")
        || line.starts_with("deleted file mode ")
        || line.starts_with("similarity index ")
        || line.starts_with("rename from ")
        || line.starts_with("rename to ")
        || line.starts_with("copy from ")
        || line.starts_with("copy to ")
    {
        return None;
    }
    if line.starts_with('\\') {
        return Some((Kind::Note, line));
    }
    match line.as_bytes().first() {
        Some(b'+') => Some((Kind::Added, line)),
        Some(b'-') => Some((Kind::Deleted, line)),
        // 文脈行は先頭が空白。空行の文脈は git が空文字列で出すのでこちらも文脈扱いにする
        Some(b' ') | None => Some((Kind::Context, line)),
        // "Binary files ... differ" 等はそのまま注記として見せる
        _ => Some((Kind::Note, line)),
    }
}

// hunk header の +側開始行 + 行数から、この diff に出てくる新側行番号の最大値を求める
pub(super) fn max_new_lineno(body: &[(Kind, &str)]) -> usize {
    let mut max = 0usize;
    let mut lineno = 0usize;
    for (kind, raw) in body {
        match kind {
            Kind::Hunk => lineno = hunk_start(raw).unwrap_or(lineno),
            Kind::Added | Kind::Context => {
                max = max.max(lineno);
                lineno += 1;
            }
            _ => {}
        }
    }
    max
}

// hunk header の -側開始行 + 行数から、この diff に出てくる旧側行番号の最大値を求める
// (max_new_lineno と対になる side-by-side 左カラム用)
pub(super) fn max_old_lineno(body: &[(Kind, &str)]) -> usize {
    let mut max = 0usize;
    let mut lineno = 0usize;
    for (kind, raw) in body {
        match kind {
            Kind::Hunk => lineno = hunk_old_start(raw).unwrap_or(lineno),
            Kind::Deleted | Kind::Context => {
                max = max.max(lineno);
                lineno += 1;
            }
            _ => {}
        }
    }
    max
}

// "@@ -a,b +c,d @@ ..." の c を取る (git.rs の parse_hunk_header と同じ形式)
pub(super) fn hunk_start(line: &str) -> Option<usize> {
    let new_range = line.split_whitespace().nth(2)?.strip_prefix('+')?;
    new_range.split(',').next()?.parse().ok()
}

// "@@ -a,b +c,d @@ ..." の a を取る (hunk_start と対になる旧側の開始行)
pub(super) fn hunk_old_start(line: &str) -> Option<usize> {
    let old_range = line.split_whitespace().nth(1)?.strip_prefix('-')?;
    old_range.split(',').next()?.parse().ok()
}

// gutter は span[0] 固定という TextPane の前提を守るため、番号なしの行でも幅を埋める
pub(super) fn blank_gutter(gutter_width: usize) -> Span<'static> {
    Span::raw(" ".repeat(gutter_width))
}

// side-by-side で片側が空の視覚行を埋めるための、gutter のみの行 (content span 無し)。
// span[1..] を連結すると空文字列になるだけなので hscroll/highlight には何の影響も無い
pub(super) fn blank_row(gutter_width: usize) -> Line<'static> {
    Line::from(vec![blank_gutter(gutter_width)])
}

pub(super) fn number_gutter(number: usize, gutter_width: usize) -> Span<'static> {
    let digits = gutter_width.saturating_sub(1);
    Span::styled(
        format!("{number:>digits$} "),
        Style::default().fg(Color::DarkGray),
    )
}

#[cfg(test)]
mod tests {
    use super::{Kind, classify_indexed};

    fn raw(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| s.to_string()).collect()
    }

    // `-- ` で始まる行の削除は diff 上で `--- ` になる (SQL/Haskell のコメント、markdown の
    // `---` 等)。位置を見ずにヘッダとして落とすと、その行が表示からも Enter の対象からも消える
    #[test]
    fn a_deleted_line_starting_with_two_dashes_is_not_mistaken_for_a_header() {
        let raw = raw(&[
            "diff --git a/q.sql b/q.sql",
            "index 1111111..2222222 100644",
            "--- a/q.sql",
            "+++ b/q.sql",
            "@@ -1,3 +1,3 @@",
            " SELECT 1;",
            "--- 古いコメント",
            " SELECT 2;",
            "+++ added",
        ]);
        let body = classify_indexed(&raw);
        // ヘッダ 4 行は落ち、hunk header + 4 行が残る
        let kinds: Vec<_> = body.iter().map(|(_, (k, _))| *k).collect();
        assert!(matches!(kinds[0], Kind::Hunk));
        assert!(matches!(kinds[2], Kind::Deleted), "{kinds:?}");
        assert!(matches!(kinds[4], Kind::Added), "{kinds:?}");
        // 表示行 → raw の index が 1:1 で戻せる (行単位ステージが依存する対応)
        let indices: Vec<usize> = body.iter().map(|(i, _)| *i).collect();
        assert_eq!(indices, vec![4, 5, 6, 7, 8]);
    }

    // 最初の @@ より前の `--- a/...` / `+++ b/...` は今までどおりヘッダとして落とす
    #[test]
    fn file_headers_before_the_first_hunk_are_still_dropped() {
        let raw = raw(&["--- a/a.txt", "+++ b/a.txt", "@@ -1,1 +1,1 @@", "-a", "+b"]);
        let indices: Vec<usize> = classify_indexed(&raw).iter().map(|(i, _)| *i).collect();
        assert_eq!(indices, vec![2, 3, 4]);
    }
}
