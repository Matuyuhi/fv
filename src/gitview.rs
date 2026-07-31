//! GIT レーン (Shift+Tab で入る変更レビュー) の表示状態。
//! git CLI の unified diff を TextPane が描ける Line 列に組み替えるところまでを持ち、
//! Viewer の cache・履歴・検索には触らない (EditState が Highlighter と Viewport だけを
//! 借りるのと同じ、依存範囲を広げないための制限)。
//!
//! `render_commit` は LOG レーン (logview.rs) の複数ファイル diff (`git show`) 用。
//! 1 行単位の組み立てヘルパー (classify/build_body 等) を GitState の単一ファイル diff と
//! 共有しつつ、ファイル境界ヘッダの挿入だけを上乗せする形にしてある。

use std::path::{Path, PathBuf};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::editor::diff::{self, CharRanges};
use crate::git::{self, DiffBase};
use crate::text;
use crate::viewer::Viewport;

const ADDED: Color = Color::Green;
const DELETED: Color = Color::Red;
const HUNK: Color = Color::Cyan;
// word-level ハイライト (#29) の背景色。前景の赤/緑はそのまま、背景だけ濃くして
// 変更範囲を示す (検索ハイライトの MATCH_BG と同じく端末テーマに依存しない固定色)
const ADDED_WORD_BG: Color = Color::Rgb(20, 90, 20);
const DELETED_WORD_BG: Color = Color::Rgb(110, 25, 25);
// 1 hunk あたりの word-level 対応ペア数の上限。大きな hunk で char diff を大量に
// 計算して描画が重くなるのを避けるための打ち切り (超えた分は従来の全行ハイライト)
const MAX_WORD_DIFF_PAIRS_PER_HUNK: usize = 200;

/// diff 行の由来。gutter に新側行番号を出すか、内容をどの色で出すかがこれで決まる
enum Kind {
    Hunk,
    Added,
    Deleted,
    Context,
    /// "\ No newline at end of file" 等の注記行
    Note,
}

/// render_commit の戻り値: (行, hunk header index 一覧, gutter 幅, 最長行幅, ファイル境界)。
/// clippy::type_complexity 回避のための alias で、意味は呼び出し側 (logview.rs) のタプル
/// destructure と 1:1 対応させたまま
type CommitRender = (
    Vec<Line<'static>>,
    Vec<usize>,
    usize,
    usize,
    Vec<(usize, String)>,
);

struct GitDiff {
    title: String,
    lines: Vec<Line<'static>>,
    /// lines 上の hunk header の index (n/N ジャンプ用)
    hunks: Vec<usize>,
    gutter_width: usize,
    /// gutter を除いた最長行の char 数。水平スクロールのクランプ上限に使う
    max_width: usize,
    path: PathBuf,
}

pub struct GitState {
    /// diff は閲覧中のファイルとは別ドキュメントなので、Viewer と EditState が共有する
    /// Viewport とは別に自前で持つ (GIT に入っても VIEW 側の読み位置を壊さないため)
    pub viewport: Viewport,
    /// 現在の diff 基準 (HEAD/staged/unstaged)。`w` (折返し) と同じく GIT レーン内だけの
    /// 一時状態で config には保存しない
    base: DiffBase,
    current: Option<GitDiff>,
}

impl GitState {
    pub fn new(wrap: bool) -> Self {
        Self {
            viewport: Viewport::new(wrap),
            base: DiffBase::Head,
            current: None,
        }
    }

    /// 指定ファイルの diff を読み込んで表示対象にする。差分が無い・取得失敗の場合も
    /// title 付きの空 diff として保持し、ペイン側で "no changes" を出す
    pub fn open(&mut self, root: &Path, path: &Path) {
        let title = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string();
        let raw = git::file_diff(root, path, self.base).unwrap_or_default();
        let (lines, hunks, gutter_width, max_width) = render(&raw);
        self.current = Some(GitDiff {
            title,
            lines,
            hunks,
            gutter_width,
            max_width,
            path: path.to_path_buf(),
        });
        self.viewport.scroll = 0;
        self.viewport.hscroll = 0;
    }

    /// t: diff 基準を HEAD → staged → unstaged → HEAD と循環し、表示中ファイルを取り直す。
    /// スクロール位置は refresh と同じく新しい行数にクランプして維持する
    pub fn cycle_base(&mut self, root: &Path) {
        self.base = self.base.next();
        self.refresh(root);
    }

    pub fn base_label(&self) -> &'static str {
        self.base.label()
    }

    /// 表示中ファイルの diff を取り直す (rescan / 外部変更の取り込み後)。
    /// スクロール位置は新しい行数にクランプして維持する
    pub fn refresh(&mut self, root: &Path) {
        let Some(path) = self.current.as_ref().map(|d| d.path.clone()) else {
            return;
        };
        let scroll = self.viewport.scroll;
        self.open(root, &path);
        self.viewport.scroll = scroll.min(self.line_count().saturating_sub(1));
    }

    pub fn title(&self) -> Option<&str> {
        self.current.as_ref().map(|d| d.title.as_str())
    }

    pub fn lines(&self) -> &[Line<'static>] {
        match &self.current {
            Some(diff) => &diff.lines,
            None => &[],
        }
    }

    pub fn gutter_width(&self) -> usize {
        self.current.as_ref().map_or(0, |d| d.gutter_width)
    }

    pub fn line_count(&self) -> usize {
        self.lines().len()
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let last = self.line_count().saturating_sub(1);
        self.viewport.scroll_by(delta, last);
    }

    pub fn jump_to_top(&mut self) {
        self.viewport.scroll = 0;
    }

    pub fn jump_to_bottom(&mut self) {
        let total = self.line_count();
        let last = total.saturating_sub(1);
        self.viewport.scroll = total.saturating_sub(self.viewport.height).min(last);
    }

    pub fn hscroll_by(&mut self, delta: isize) {
        let max = self
            .current
            .as_ref()
            .map_or(0, |d| d.max_width.saturating_sub(self.viewport.width / 2));
        self.viewport.hscroll_by(delta, max);
    }

    pub fn hscroll_reset(&mut self) {
        self.viewport.hscroll = 0;
    }

    /// n: 現在位置より後ろの最初の hunk header へ。無ければ位置を変えない
    pub fn next_hunk(&mut self) {
        let Some(diff) = &self.current else {
            return;
        };
        if let Some(&target) = diff.hunks.iter().find(|&&i| i > self.viewport.scroll) {
            self.viewport.scroll = target;
        }
    }

    /// N: 現在位置より前の最後の hunk header へ
    pub fn prev_hunk(&mut self) {
        let Some(diff) = &self.current else {
            return;
        };
        if let Some(&target) = diff.hunks.iter().rev().find(|&&i| i < self.viewport.scroll) {
            self.viewport.scroll = target;
        }
    }
}

/// unified diff の生行から (描画行, hunk 位置, gutter 幅, 最長行幅) を組み立てる。
/// gutter には新側の行番号を入れ、削除行は空欄にする
fn render(raw: &[String]) -> (Vec<Line<'static>>, Vec<usize>, usize, usize) {
    let body: Vec<(Kind, &str)> = raw.iter().filter_map(|line| classify(line)).collect();
    // gutter 幅は行番号の最大桁で決まるので、行番号を振る前に一度だけ走査して求める
    let max_lineno = max_new_lineno(&body);
    let gutter_width = text::gutter_width(max_lineno);
    let (lines, hunks, max_width) = build_body(body, gutter_width);
    (lines, hunks, gutter_width, max_width)
}

/// LOG レーンの複数ファイル diff (`git show`) 用。単一ファイルの `render` とはコミット
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

    let bodies: Vec<Vec<(Kind, &str)>> = segments
        .iter()
        .map(|seg| seg.iter().filter_map(|l| classify(l)).collect())
        .collect();
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
        let (body_lines, body_hunks, body_max_width) = build_body(body, gutter_width);
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
    body: Vec<(Kind, &str)>,
    gutter_width: usize,
) -> (Vec<Line<'static>>, Vec<usize>, usize) {
    // word-level の対応付けは hunk 内で閉じているので、単一ファイル (render) でも
    // ファイル単位に分割済みの複数ファイル diff (render_commit) でも同じ扱いで済む
    let word_ranges = word_diff_ranges(&body);

    let mut lines = Vec::with_capacity(body.len());
    let mut hunks = Vec::new();
    let mut max_width = 0usize;
    let mut lineno = 0usize;
    for (i, (kind, raw)) in body.into_iter().enumerate() {
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
        match (word_bg, &word_ranges[i]) {
            (Some(bg), Some(word_range)) => {
                spans.extend(split_with_emphasis(&content, style, bg, word_range))
            }
            _ => spans.push(Span::styled(content, style)),
        }
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

// hunk 内で連続する削除ブロック→追加ブロックのペアを検出し、行数が一致する時だけ
// 1 対 1 で char 単位の差分 (editor::diff::word_diff) を計算する。行数が合わない・
// 打ち切り上限を超えるペアは None のままにし、呼び出し側を従来の全行ハイライトに倒す
fn word_diff_ranges(body: &[(Kind, &str)]) -> Vec<Option<CharRanges>> {
    let mut ranges: Vec<Option<CharRanges>> = vec![None; body.len()];
    let mut hunk_pairs = 0usize;
    let mut i = 0;
    while i < body.len() {
        match body[i].0 {
            Kind::Hunk => {
                hunk_pairs = 0;
                i += 1;
            }
            Kind::Deleted => {
                let del_start = i;
                let del_end = run_end(body, del_start, |k| matches!(k, Kind::Deleted));
                let add_start = del_end;
                let add_end = run_end(body, add_start, |k| matches!(k, Kind::Added));
                let del_len = del_end - del_start;
                let add_len = add_end - add_start;
                if del_len == add_len && hunk_pairs + del_len <= MAX_WORD_DIFF_PAIRS_PER_HUNK {
                    hunk_pairs += del_len;
                    for offset in 0..del_len {
                        pair_word_diff(body, del_start + offset, add_start + offset, &mut ranges);
                    }
                } else {
                    hunk_pairs += del_len;
                }
                i = add_end;
            }
            _ => i += 1,
        }
    }
    ranges
}

fn run_end(body: &[(Kind, &str)], start: usize, matches_kind: impl Fn(&Kind) -> bool) -> usize {
    let mut j = start;
    while j < body.len() && matches_kind(&body[j].0) {
        j += 1;
    }
    j
}

// 削除行・追加行 1 組の char diff を計算し、結果を該当 index の ranges に入れる。
// 先頭 1 文字は diff の +/- マーカーなので比較対象から外し、range だけマーカー分 (1 char)
// 戻して content 上の座標に合わせる
fn pair_word_diff(
    body: &[(Kind, &str)],
    del_idx: usize,
    add_idx: usize,
    ranges: &mut [Option<CharRanges>],
) {
    let del_body = text::normalize(&body[del_idx].1[1..]);
    let add_body = text::normalize(&body[add_idx].1[1..]);
    let Some((del_ranges, add_ranges)) = diff::word_diff(&del_body, &add_body) else {
        return;
    };
    ranges[del_idx] = Some(shift_ranges(del_ranges));
    ranges[add_idx] = Some(shift_ranges(add_ranges));
}

fn shift_ranges(ranges: CharRanges) -> CharRanges {
    // マーカー1 文字分だけ後ろにずらす (word_diff は marker を含まない文字列で計算している)
    ranges.into_iter().map(|(s, e)| (s + 1, e + 1)).collect()
}

// diff 本文として描く行だけを残す。ファイル名は右ペインのタイトルに出るので、
// diff --git / index / --- / +++ 等のヘッダは落とす
fn classify(line: &str) -> Option<(Kind, &str)> {
    if line.starts_with("@@") {
        return Some((Kind::Hunk, line));
    }
    if line.starts_with("diff --git")
        || line.starts_with("index ")
        || line.starts_with("--- ")
        || line.starts_with("+++ ")
        || line.starts_with("old mode ")
        || line.starts_with("new mode ")
        || line.starts_with("new file mode ")
        || line.starts_with("deleted file mode ")
        || line.starts_with("similarity index ")
        || line.starts_with("rename from ")
        || line.starts_with("rename to ")
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
fn max_new_lineno(body: &[(Kind, &str)]) -> usize {
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

// "@@ -a,b +c,d @@ ..." の c を取る (git.rs の parse_hunk_header と同じ形式)
fn hunk_start(line: &str) -> Option<usize> {
    let new_range = line.split_whitespace().nth(2)?.strip_prefix('+')?;
    new_range.split(',').next()?.parse().ok()
}

// gutter は span[0] 固定という TextPane の前提を守るため、番号なしの行でも幅を埋める
fn blank_gutter(gutter_width: usize) -> Span<'static> {
    Span::raw(" ".repeat(gutter_width))
}

fn number_gutter(number: usize, gutter_width: usize) -> Span<'static> {
    let digits = gutter_width.saturating_sub(1);
    Span::styled(
        format!("{number:>digits$} "),
        Style::default().fg(Color::DarkGray),
    )
}
