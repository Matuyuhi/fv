//! GIT レーン (Shift+Tab で入る変更レビュー) の表示状態。
//! git CLI の unified diff を TextPane が描ける Line 列に組み替えるところまでを持ち、
//! Viewer の cache・履歴・検索には触らない (EditState が Highlighter と Viewport だけを
//! 借りるのと同じ、依存範囲を広げないための制限)。

use std::path::{Path, PathBuf};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::git::{self, DiffBase};
use crate::text;
use crate::viewer::Viewport;

const ADDED: Color = Color::Green;
const DELETED: Color = Color::Red;
const HUNK: Color = Color::Cyan;

/// diff 行の由来。gutter に新側行番号を出すか、内容をどの色で出すかがこれで決まる
enum Kind {
    Hunk,
    Added,
    Deleted,
    Context,
    /// "\ No newline at end of file" 等の注記行
    Note,
}

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

    let mut lines = Vec::with_capacity(body.len());
    let mut hunks = Vec::new();
    let mut max_width = 0usize;
    let mut lineno = 0usize;
    for (kind, raw) in body {
        let content = text::normalize(raw);
        max_width = max_width.max(content.chars().count());
        let (gutter, style) = match kind {
            Kind::Hunk => {
                hunks.push(lines.len());
                lineno = hunk_start(raw).unwrap_or(lineno);
                (blank_gutter(gutter_width), Style::default().fg(HUNK))
            }
            Kind::Added => {
                let g = number_gutter(lineno, gutter_width);
                lineno += 1;
                (g, Style::default().fg(ADDED))
            }
            Kind::Deleted => (blank_gutter(gutter_width), Style::default().fg(DELETED)),
            Kind::Context => {
                let g = number_gutter(lineno, gutter_width);
                lineno += 1;
                (g, Style::default())
            }
            Kind::Note => (
                blank_gutter(gutter_width),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ),
        };
        lines.push(Line::from(vec![gutter, Span::styled(content, style)]));
    }
    (lines, hunks, gutter_width, max_width)
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
