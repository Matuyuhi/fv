//! GIT レーン (Shift+Tab で入る変更レビュー) の表示状態。
//! git CLI の unified diff を TextPane が描ける Line 列に組み替えるところまでを持ち、
//! Viewer の cache・履歴・検索には触らない (EditState が Highlighter と Viewport だけを
//! 借りるのと同じ、依存範囲を広げないための制限)。
//!
//! `render_commit` は LOG レーン (component/log/mod.rs) の複数ファイル diff (`git show`) 用。
//! 1 行単位の組み立てヘルパー (classify/build_body 等) を GitState の単一ファイル diff と
//! 共有しつつ、ファイル境界ヘッダの挿入だけを上乗せする形にしてある。
//! GIT レーンの「全ファイルまとめ diff」(`A`、#31) もこの `render_commit` をそのまま呼び、
//! 複数ファイル diff のレンダラを 2 箇所に複製しない。
//!
//! side-by-side (#30) は GIT レーンの単一ファイル diff のみ対応。LOG レーンの複数ファイル
//! diff (render_commit) は既存のまま inline 専用で残した (issue #30 が GIT レーンだけでも可、
//! としているスコープに合わせた)。「全ファイルまとめ diff」も同じ理由で inline 専用にする
//! (showing_all 中は side_by_side_active が常に false を返す)。
//!
//! 分割方針: このファイルは「今どの diff をどう見ているか」という状態 (GitState) だけを持ち、
//! 生の unified diff を Line 列へ組み替えるレンダラは用途ごとのサブモジュールへ分ける。
//! 定数・Kind・各 *Diff 構造体をここに残すのは、レンダラ 3 種が同じ形を組み立てるため。
pub mod view;

mod patch;
mod render;
mod side;
mod word;

pub use render::render_commit;
pub use side::side_by_side_wrapped;

use patch::build_line_patch;
use render::{classify, render_inline};
use side::render_side_by_side;

use std::path::{Path, PathBuf};

use ratatui::style::Color;
use ratatui::text::Line;

use crate::component::viewer::{Match, SearchState, Viewport, search_matches};
use crate::git::{self, DiffBase};
use crate::text;

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
// side-by-side の 1 カラムの最小幅 (gutter 込み)。これを切ったら自動で inline に戻す
// (ペイン幅のドラッグリサイズで縮めていっても文字が潰れて読めなくなるのを避けるため)
const MIN_SIDE_BY_SIDE_COLUMN: usize = 40;

/// diff 行の由来。gutter に新側行番号を出すか、内容をどの色で出すかがこれで決まる
#[derive(Clone, Copy)]
enum Kind {
    Hunk,
    Added,
    Deleted,
    Context,
    /// "\ No newline at end of file" 等の注記行
    Note,
}

/// render_commit の戻り値: (行, hunk header index 一覧, gutter 幅, 最長行幅, ファイル境界)。
/// clippy::type_complexity 回避のための alias で、意味は呼び出し側 (component/log/mod.rs) のタプル
/// destructure と 1:1 対応させたまま
type CommitRender = (
    Vec<Line<'static>>,
    Vec<usize>,
    usize,
    usize,
    Vec<(usize, String)>,
);

struct InlineDiff {
    lines: Vec<Line<'static>>,
    /// lines 上の hunk header の index (n/N ジャンプ用)
    hunks: Vec<usize>,
    gutter_width: usize,
    /// gutter を除いた最長行の char 数。水平スクロールのクランプ上限に使う
    max_width: usize,
}

/// side-by-side (左 = 旧, 右 = 新) の 2 本の Line 列。削除/追加が連続するブロックは
/// 同じ視覚行に並べ、行数が合わない側は空行で埋めてあるので left.len() == right.len() が
/// 常に成り立つ (hunks もこの揃えた後の行 index を指す)
struct SideDiff {
    left: Vec<Line<'static>>,
    right: Vec<Line<'static>>,
    hunks: Vec<usize>,
    left_gutter_width: usize,
    right_gutter_width: usize,
    left_max_width: usize,
    right_max_width: usize,
}

struct GitDiff {
    title: String,
    inline: InlineDiff,
    side: SideDiff,
    path: PathBuf,
    /// hunk 単位 stage/unstage 用の生 unified diff。表示用の Line 列からは復元できない
    /// (classify がファイルヘッダを落とし、text::normalize がタブを空白へ展開するため)
    /// ので、取得時の raw をそのまま持っておく
    raw: Vec<String>,
    /// raw 上の hunk header (@@ 行) の index。classify は "@@" 始まりの行だけを Kind::Hunk に
    /// するので、この一覧は inline.hunks / side.hunks と同じ順序・同じ個数になる。
    /// 表示行 → 生 diff の対応付けを「何番目の hunk か」だけに絞れるのはこの 1:1 が根拠
    raw_hunks: Vec<usize>,
    /// inline 表示の行 index → 生 diff の行 index。classify がファイルヘッダを落とすぶんだけ
    /// ずれるだけで単調増加なので、表示行の閉区間はそのまま生 diff 上の閉区間に写せる
    /// (行単位ステージ (`S`) が選択範囲を生 diff 側へ持ち込む唯一の経路)
    body_raw: Vec<usize>,
    /// hunk 単位で index に適用できるか。untracked の `--no-index` フォールバックで作った
    /// diff はヘッダのパスが repo 相対でない (呼び出し側が絶対パスを渡すため) ので、
    /// そのまま `git apply --cached` に通すと repo 外へファイルを作ろうとして失敗する。
    /// ヘッダのパスが期待する相対パスと一致するときだけ true にし、判別できない形
    /// (git がクォートしたパス等) は false = 拒否側へ倒す
    stageable: bool,
}

/// `Space` (GIT レーン右ペイン) の対象。組み立てられなかった理由まで型で返し、
/// notice の文言は App 側 (app/git_ops.rs) が決める
pub enum HunkPatch {
    Ready {
        /// git apply にそのまま渡せる 1 ファイル・1 hunk のパッチ
        patch: String,
        /// 表示用の 1-origin 序数
        ordinal: usize,
        total: usize,
    },
    /// `A` のまとめ表示中。1 つのファイルヘッダに決められないので単一ファイル表示に戻してもらう
    ShowingAll,
    /// untracked (`--no-index`) 由来の diff。ツリー側の Space でファイル単位に stage する
    NotApplicable,
    /// diff が空 / hunk header が 1 つも無い (binary 等)
    Empty,
}

/// `S` (GIT レーン右ペイン) の対象。HunkPatch と同じく、組み立てられなかった理由まで
/// 型で返して notice の文言は App 側 (app/git_ops.rs) に決めさせる
pub enum LinePatch {
    Ready {
        /// git apply にそのまま渡せる 1 ファイル分のパッチ (選んだ行だけを反映する)
        patch: String,
        /// 実際に反映される変更行 (+/-) の数。notice に出す
        lines: usize,
    },
    /// `A` のまとめ表示中。hunk 単位と同じ理由で単一ファイル表示に戻してもらう
    ShowingAll,
    /// side-by-side 表示中。左右で 1 つの行 index が 2 本の行を指すので生 diff に写せない
    SideBySide,
    /// untracked (`--no-index`) 由来の diff。ツリー側の Space でファイル単位に stage する
    NotApplicable,
    /// 選択範囲に + / - の行が 1 つも無い (文脈行・hunk header だけを選んでいる)
    NoChangedLines,
    /// diff が空 / hunk header が 1 つも無い (binary 等)
    Empty,
}

/// `A` の全ファイルまとめ diff。単一ファイルの `GitDiff` とは独立に持ち、トグルで
/// 表示を切り替えるだけで OFF→ON の取り直しはしない (基準切替・rescan の取り直しだけ
/// `load_all` を再度呼ぶ)。side-by-side 相当の構造を持たないのは inline 専用のため
struct AllDiff {
    /// ペインタイトルにそのまま出す (ファイル数・打ち切りの有無を含む)
    title: String,
    lines: Vec<Line<'static>>,
    hunks: Vec<usize>,
    gutter_width: usize,
    max_width: usize,
    /// ファイル境界: #40 と同じ sticky header 用 (見出し行の index → ラベル)
    boundaries: Vec<(usize, String)>,
}

pub struct GitState {
    /// diff は閲覧中のファイルとは別ドキュメントなので、Viewer と EditState が共有する
    /// Viewport とは別に自前で持つ (GIT に入っても VIEW 側の読み位置を壊さないため)
    pub viewport: Viewport,
    /// 現在の diff 基準 (HEAD/staged/unstaged)。`w` (折返し) と同じく GIT レーン内だけの
    /// 一時状態で config には保存しない
    base: DiffBase,
    current: Option<GitDiff>,
    /// v: inline / side-by-side 切替。`w` と同じく config には保存しない
    side_by_side: bool,
    /// side-by-side + wrap 時だけ使う、直前フレームで実際に描いた行数・hunk 位置。
    /// wrap 幅は実測でしか出せないため、viewport.height/width と同じく ui 側が毎フレーム
    /// 書き戻す (side_by_side_wrapped の結果をそのまま持たせる)
    side_wrap_cache: Option<(usize, Vec<usize>)>,
    /// diff 内検索 (#31)。単一ファイル/まとめ diff のどちらでも同じ 1 つの状態を使い回す
    /// (対象は常に「今表示している inline 行」で、切替のたびに recompute_search で追従する)
    search: Option<SearchState>,
    /// `A`: 全ファイルまとめ diff の取得結果。`showing_all` が true の間だけ表示に使う。
    /// Box するのは Lane enum (Git/Edit/Log の判別共用体) のサイズを抑えるため
    /// (clippy::large_enum_variant。単一ファイルの GitDiff と違い、まとめ diff は
    /// 複数ファイル分の Line 列を持つので素の埋め込みだと variant がかなり膨らむ)
    all: Option<Box<AllDiff>>,
    /// `A` のトグル状態。ツリーでファイルを選び直すと `exit_all` で false に戻す
    showing_all: bool,
    /// 行カーソル (今表示している行リスト上の論理行 index)。diff ペインには EDIT のような
    /// 文字カーソルが無いので、以前は Space の対象を「上端に見えている行が属する hunk」と
    /// していたが、画面のどこを指しているのかが一切見えず対象が当てられなかった。
    /// j/k はこのカーソルを動かし viewport が追従する形にして、描画でも帯として見せる
    cursor: usize,
    /// `V` で始めた行選択の起点。None の間は対象がカーソル行 1 行だけになる。
    /// 表示中の行リストが変わる操作 (ファイル切替・基準切替・v/A のトグル) では捨てる
    selection_anchor: Option<usize>,
}

impl GitState {
    pub fn new(wrap: bool) -> Self {
        Self {
            viewport: Viewport::new(wrap),
            base: DiffBase::Head,
            current: None,
            side_by_side: false,
            side_wrap_cache: None,
            search: None,
            all: None,
            showing_all: false,
            cursor: 0,
            selection_anchor: None,
        }
    }

    /// 指定ファイルの diff を読み込んで表示対象にする。差分が無い・取得失敗の場合も
    /// title 付きの空 diff として保持し、ペイン側で "no changes" を出す。
    /// `showing_all` はここでは触らない (rescan 経由の refresh からも呼ばれるため。
    /// ツリーでの選び直しに伴う解除は呼び出し側の `exit_all` が担う)
    pub fn open(&mut self, root: &Path, path: &Path) {
        let title = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string();
        let raw = git::file_diff(root, path, self.base).unwrap_or_default();
        let mut body: Vec<(Kind, &str)> = Vec::with_capacity(raw.len());
        let mut body_raw: Vec<usize> = Vec::with_capacity(raw.len());
        for (i, line) in raw.iter().enumerate() {
            if let Some(item) = classify(line) {
                body.push(item);
                body_raw.push(i);
            }
        }
        let raw_hunks = raw
            .iter()
            .enumerate()
            .filter(|(_, line)| line.starts_with("@@"))
            .map(|(i, _)| i)
            .collect();
        let stageable = header_path_matches(&raw, &title);
        self.current = Some(GitDiff {
            title,
            inline: render_inline(&body),
            side: render_side_by_side(&body),
            path: path.to_path_buf(),
            raw,
            raw_hunks,
            body_raw,
            stageable,
        });
        self.viewport.scroll = 0;
        self.viewport.hscroll = 0;
        self.cursor = 0;
        self.selection_anchor = None;
        self.side_wrap_cache = None;
        self.recompute_search();
    }

    /// ツリーでファイルを選び直した (#31: まとめ表示は解除して単一ファイル表示に戻る)
    pub fn exit_all(&mut self) {
        self.showing_all = false;
    }

    /// A: 全ファイルまとめ diff とファイル単位表示をトグルする。ON にする瞬間だけ取得し直し、
    /// OFF に戻す時は取り直さない (既に読み込み済みの単一ファイル側にそのまま戻るだけ)。
    /// 戻り値は今回の取得で打ち切りが発生したか (OFF に戻すだけの場合は常に false)
    pub fn toggle_all(&mut self, root: &Path, untracked: &[PathBuf]) -> bool {
        self.showing_all = !self.showing_all;
        let truncated = if self.showing_all {
            self.load_all(root, untracked)
        } else {
            false
        };
        self.viewport.scroll = 0;
        self.viewport.hscroll = 0;
        self.cursor = 0;
        self.selection_anchor = None;
        self.side_wrap_cache = None;
        self.recompute_search();
        truncated
    }

    // render_commit をそのまま再利用する (LOG レーンの複数ファイル diff とレンダラを共有する
    // という #31 の要求そのもの)。ヘッダ (コミットメッセージ相当) が無いだけで組み立ては同じ
    fn load_all(&mut self, root: &Path, untracked: &[PathBuf]) -> bool {
        let (raw, truncated) = git::diff_all(root, self.base, untracked);
        let (lines, hunks, gutter_width, max_width, boundaries) = render_commit(&raw);
        let mut title = format!("all changes ({} files)", boundaries.len());
        if truncated {
            title.push_str("  (打ち切り)");
        }
        self.all = Some(Box::new(AllDiff {
            title,
            lines,
            hunks,
            gutter_width,
            max_width,
            boundaries,
        }));
        truncated
    }

    /// t: diff 基準を HEAD → staged → unstaged → HEAD と循環し、表示中の内容を取り直す。
    /// まとめ表示中はまとめ diff を、そうでなければ単一ファイルを refresh に委ねる
    pub fn cycle_base(&mut self, root: &Path, untracked: &[PathBuf]) -> bool {
        self.base = self.base.next();
        self.refresh(root, untracked)
    }

    pub fn base_label(&self) -> &'static str {
        self.base.label()
    }

    /// 表示中の内容を取り直す (rescan / 外部変更の取り込み後、基準切替後)。
    /// スクロール位置は新しい行数にクランプして維持する。戻り値は打ち切りが発生したか
    /// (まとめ表示でない間は常に false)
    pub fn refresh(&mut self, root: &Path, untracked: &[PathBuf]) -> bool {
        if self.showing_all {
            let truncated = self.load_all(root, untracked);
            let last = self.line_count().saturating_sub(1);
            self.viewport.scroll = self.viewport.scroll.min(last);
            self.cursor = self.cursor.min(last);
            self.recompute_search();
            return truncated;
        }
        let Some(path) = self.current.as_ref().map(|d| d.path.clone()) else {
            return false;
        };
        // stage/unstage の直後もここを通る。読んでいた位置とカーソルはどちらも維持したい
        // (適用した hunk が diff から消えて行数が縮んでも、近い行に留めるだけでよい)
        let scroll = self.viewport.scroll;
        let cursor = self.cursor;
        self.open(root, &path);
        let last = self.line_count().saturating_sub(1);
        self.viewport.scroll = scroll.min(last);
        self.cursor = cursor.min(last);
        false
    }

    pub fn title(&self) -> Option<&str> {
        if self.showing_all {
            return self.all.as_ref().map(|d| d.title.as_str());
        }
        self.current.as_ref().map(|d| d.title.as_str())
    }

    /// inline 表示用の行 (単一ファイルの side-by-side が無効/幅不足フォールバック時、
    /// および全ファイルまとめ表示の両方で使う)
    pub fn lines(&self) -> &[Line<'static>] {
        if self.showing_all {
            return self.all.as_ref().map_or(&[], |d| d.lines.as_slice());
        }
        match &self.current {
            Some(diff) => &diff.inline.lines,
            None => &[],
        }
    }

    pub fn gutter_width(&self) -> usize {
        if self.showing_all {
            return self.all.as_ref().map_or(0, |d| d.gutter_width);
        }
        self.current.as_ref().map_or(0, |d| d.inline.gutter_width)
    }

    pub fn line_count(&self) -> usize {
        if self.showing_all {
            return self.lines().len();
        }
        if self.side_by_side_active() {
            if self.viewport.wrap
                && let Some((len, _)) = &self.side_wrap_cache
            {
                return *len;
            }
            return self.current.as_ref().map_or(0, |d| d.side.left.len());
        }
        self.lines().len()
    }

    /// マウスホイール専用: 画面だけを送り、カーソルは可視範囲の端に寄せて連れて行く。
    /// カーソルが画面外に置き去りになると Space/S の対象が見えないまま押せてしまう
    pub fn scroll_by(&mut self, delta: isize) {
        let last = self.line_count().saturating_sub(1);
        self.viewport.scroll_by(delta, last);
        if self.viewport.height == 0 {
            return;
        }
        let top = self.viewport.scroll;
        let bottom = (top + self.viewport.height - 1).min(last);
        self.cursor = self.cursor.clamp(top, bottom);
    }

    /// j/k/Ctrl+d/Ctrl+u: カーソルを動かし viewport を追従させる (EDIT と同じ向きの操作)
    pub fn move_cursor(&mut self, delta: isize) {
        let last = self.line_count().saturating_sub(1);
        let next = (self.cursor_line() as isize + delta).clamp(0, last as isize) as usize;
        self.cursor_to(next);
    }

    /// カーソルを指定行へ置き、その行が見えるところまでスクロールする
    pub fn cursor_to(&mut self, line: usize) {
        let last = self.line_count().saturating_sub(1);
        self.cursor = line.min(last);
        self.ensure_cursor_visible();
    }

    /// 今どの行に居るか (行数が縮んでも常に有効な index に丸めて返す)
    pub fn cursor_line(&self) -> usize {
        self.cursor.min(self.line_count().saturating_sub(1))
    }

    /// 帯を敷く閉区間。V の選択中は起点〜カーソル、そうでなければカーソル行 1 行だけ
    pub fn cursor_band(&self) -> (usize, usize) {
        let cursor = self.cursor_line();
        match self.selection_anchor {
            Some(anchor) => {
                let anchor = anchor.min(self.line_count().saturating_sub(1));
                (anchor.min(cursor), anchor.max(cursor))
            }
            None => (cursor, cursor),
        }
    }

    /// V: 行選択の開始 / 解除。開始位置は今のカーソル行
    pub fn toggle_selection(&mut self) {
        self.selection_anchor = match self.selection_anchor {
            Some(_) => None,
            None => Some(self.cursor_line()),
        };
    }

    pub fn clear_selection(&mut self) {
        self.selection_anchor = None;
    }

    /// 選択中の行数 (選択していなければ None)。ステータスバーのヒントに出す
    pub fn selected_line_count(&self) -> Option<usize> {
        self.selection_anchor?;
        let (start, end) = self.cursor_band();
        Some(end - start + 1)
    }

    // カーソル行が縦範囲に収まるまでスクロールする。wrap 中は視覚行数で判定する
    // (EditState::ensure_visible と同じ数え方。side-by-side は wrap でも事前分割済みの
    // 行を 1 視覚行として渡しているので、常に論理行 = 視覚行の扱いでよい)
    fn ensure_cursor_visible(&mut self) {
        // height はペインを描いた時に ui 側が書き戻す。まだ 0 = このレーンを 1 度も描いて
        // いない間は「何行見えているか」が分からないので動かさない (1 行として扱うと、
        // 描く前に届いたキーでカーソル行が画面上端に貼り付いてしまう)
        if self.viewport.height == 0 {
            return;
        }
        let line = self.cursor_line();
        self.viewport.ensure_row_visible(line);
        if !self.viewport.wrap || self.side_by_side_active() {
            return;
        }
        // ensure_row_visible で scroll..line の距離は高々 height に収まっているので、
        // ここからの数え直しは画面の大きさに比例する範囲で終わる
        let width = self
            .viewport
            .width
            .saturating_sub(self.gutter_width())
            .max(1);
        let height = self.viewport.height.max(1);
        let mut scroll = self.viewport.scroll;
        let lines = self.lines();
        let mut rows = 1usize;
        for row in &lines[scroll..line] {
            rows += text::wrap_rows(&line_plain_text(row), width);
        }
        while rows > height && scroll < line {
            rows -= text::wrap_rows(&line_plain_text(&lines[scroll]), width);
            scroll += 1;
        }
        self.viewport.scroll = scroll;
    }

    /// diff ペインのクリック: 画面内座標の行 → カーソル行。折返し中は描画と同じ規則で
    /// 視覚行を辿る (Viewport::locate と同じ数え方だが、桁は要らないので行だけを見る)
    pub fn click_row(&mut self, row: usize) {
        let last = self.line_count().saturating_sub(1);
        if !self.viewport.wrap || self.side_by_side_active() {
            self.cursor = (self.viewport.scroll + row).min(last);
            return;
        }
        let width = self
            .viewport
            .width
            .saturating_sub(self.gutter_width())
            .max(1);
        let lines = self.lines();
        let mut line = self.viewport.scroll.min(last);
        let mut remaining = row;
        while line < last {
            let rows = text::wrap_rows(&line_plain_text(&lines[line]), width);
            if remaining < rows {
                break;
            }
            remaining -= rows;
            line += 1;
        }
        self.cursor = line;
    }

    /// sticky header が 1 行目を占めているか。クリック座標を行 index に直す側が引く
    pub fn sticky_rows(&self) -> usize {
        usize::from(self.sticky_label().is_some())
    }

    pub fn jump_to_top(&mut self) {
        self.viewport.scroll = 0;
        self.cursor = 0;
    }

    pub fn jump_to_bottom(&mut self) {
        let total = self.line_count();
        let last = total.saturating_sub(1);
        self.viewport.scroll = total.saturating_sub(self.viewport.height).min(last);
        self.cursor = last;
    }

    pub fn hscroll_by(&mut self, delta: isize) {
        let (width_budget, max_width) = if self.showing_all {
            (
                self.viewport.width,
                self.all.as_ref().map_or(0, |d| d.max_width),
            )
        } else if self.side_by_side_active() {
            let max_width = self
                .current
                .as_ref()
                .map_or(0, |d| d.side.left_max_width.max(d.side.right_max_width));
            (self.column_width(), max_width)
        } else {
            let max_width = self.current.as_ref().map_or(0, |d| d.inline.max_width);
            (self.viewport.width, max_width)
        };
        let max = max_width.saturating_sub(width_budget / 2);
        self.viewport.hscroll_by(delta, max);
    }

    pub fn hscroll_reset(&mut self) {
        self.viewport.hscroll = 0;
    }

    /// ]: 現在位置より後ろの最初の hunk header へ。無ければ位置を変えない。
    /// 基準はスクロール位置ではなくカーソル行 (Space の対象と一致させる)
    pub fn next_hunk(&mut self) {
        if let Some(&target) = self.hunks().iter().find(|&&i| i > self.cursor_line()) {
            self.viewport.scroll = target;
            self.cursor = target;
        }
    }

    /// [: 現在位置より前の最後の hunk header へ
    pub fn prev_hunk(&mut self) {
        if let Some(&target) = self.hunks().iter().rev().find(|&&i| i < self.cursor_line()) {
            self.viewport.scroll = target;
            self.cursor = target;
        }
    }

    fn hunks(&self) -> &[usize] {
        if self.showing_all {
            return self
                .all
                .as_ref()
                .map_or(&[] as &[usize], |d| d.hunks.as_slice());
        }
        if self.side_by_side_active() {
            if self.viewport.wrap
                && let Some((_, hunks)) = &self.side_wrap_cache
            {
                return hunks.as_slice();
            }
            return self
                .current
                .as_ref()
                .map_or(&[] as &[usize], |d| d.side.hunks.as_slice());
        }
        self.current
            .as_ref()
            .map_or(&[] as &[usize], |d| d.inline.hunks.as_slice())
    }

    /// カーソル行が居る hunk の序数 (0-origin)。以前は「上端に見えている行が属する hunk」
    /// だったが、その行が画面のどこかを示すものが無く Space の対象が当てられなかったので、
    /// 帯として見えている行カーソルを基準にする (`]`/`[` のジャンプ先とも一致したまま)。
    /// hunks() を使うので inline / side-by-side / wrap のどの表示でも同じ序数が出る
    /// (どの表示でも hunk の並び順は生 diff と同じで、変わるのは行 index だけ)
    fn current_hunk_ordinal(&self) -> Option<usize> {
        let idx = self
            .hunks()
            .partition_point(|&line| line <= self.cursor_line());
        (idx > 0).then(|| idx - 1)
    }

    /// ペインタイトルに出す「今どの hunk を見ているか」(1-origin の序数, 総数)。
    /// Space の対象が暗黙にならないようにするための表示なので、Space を受け付けない
    /// まとめ表示中 (current_hunk_patch が ShowingAll を返す) は出さない — 序数だけ出ると
    /// 「押せば効く」と読めてしまい、ステータスバーが Space のヒントを消しているのと食い違う
    pub fn hunk_position(&self) -> Option<(usize, usize)> {
        if self.showing_all {
            return None;
        }
        let total = self.hunks().len();
        self.current_hunk_ordinal().map(|o| (o + 1, total))
    }

    /// diff 基準が staged (index vs HEAD) のとき、Space は「index から取り消す」向きになる。
    /// Head / Unstaged はどちらも worktree 側の変更を index へ移す向き
    pub fn unstaging(&self) -> bool {
        self.base == DiffBase::Staged
    }

    /// Space: 今見ている hunk だけを含む 1 ファイル分のパッチを組み立てる。
    /// ファイルヘッダ (最初の @@ より前) + その hunk の生行をそのまま連結するだけで、
    /// hunk header の行番号は書き換えない — git apply は文脈行を照合して適用位置を決めるため、
    /// 先行する hunk が未適用でもオフセットを吸収してくれる (`git add -p` と同じ作法)
    pub fn current_hunk_patch(&self) -> HunkPatch {
        if self.showing_all {
            return HunkPatch::ShowingAll;
        }
        let Some(diff) = &self.current else {
            return HunkPatch::Empty;
        };
        if !diff.stageable {
            return HunkPatch::NotApplicable;
        }
        let (Some(ordinal), Some(&header_end)) =
            (self.current_hunk_ordinal(), diff.raw_hunks.first())
        else {
            return HunkPatch::Empty;
        };
        let Some(&start) = diff.raw_hunks.get(ordinal) else {
            return HunkPatch::Empty;
        };
        let end = diff
            .raw_hunks
            .get(ordinal + 1)
            .copied()
            .unwrap_or(diff.raw.len());

        let mut patch = String::new();
        for line in diff.raw[..header_end].iter().chain(&diff.raw[start..end]) {
            patch.push_str(line);
            patch.push('\n');
        }
        HunkPatch::Ready {
            patch,
            ordinal: ordinal + 1,
            total: diff.raw_hunks.len(),
        }
    }

    /// S: カーソル行 (V の選択中は選択範囲) の変更行だけを反映するパッチを組み立てる。
    /// 表示行 → 生 diff の写像は `body_raw` が持つ。選ばなかった変更行の扱い (落とす /
    /// 文脈行にする) は `patch::build_line_patch` が `git add -p` と同じ作法で決める
    pub fn current_line_patch(&self) -> LinePatch {
        if self.showing_all {
            return LinePatch::ShowingAll;
        }
        if self.side_by_side_active() {
            return LinePatch::SideBySide;
        }
        let Some(diff) = &self.current else {
            return LinePatch::Empty;
        };
        if !diff.stageable {
            return LinePatch::NotApplicable;
        }
        if diff.raw_hunks.is_empty() {
            return LinePatch::Empty;
        }
        let (start, end) = self.cursor_band();
        let (Some(&lo), Some(&hi)) = (diff.body_raw.get(start), diff.body_raw.get(end)) else {
            return LinePatch::Empty;
        };
        // 反映される行数は「選択範囲に入っている + / - の数」。ファイルヘッダより手前は
        // body_raw に入らないので、ここで数えるのは常に hunk の中の行だけになる
        let lines = diff.raw[lo..=hi]
            .iter()
            .filter(|l| matches!(l.as_bytes().first(), Some(b'+') | Some(b'-')))
            .count();
        if lines == 0 {
            return LinePatch::NoChangedLines;
        }
        match build_line_patch(&diff.raw, &diff.raw_hunks, lo, hi, self.unstaging()) {
            Some(patch) => LinePatch::Ready { patch, lines },
            None => LinePatch::NoChangedLines,
        }
    }

    /// }: まとめ diff 中の次のファイル境界へ。単一ファイル表示中は boundaries が空なので no-op
    pub fn next_file(&mut self) {
        if let Some(&(line, _)) = self
            .boundaries()
            .iter()
            .find(|(l, _)| *l > self.cursor_line())
        {
            self.viewport.scroll = line;
            self.cursor = line;
        }
    }

    /// {: まとめ diff 中の前のファイル境界へ
    pub fn prev_file(&mut self) {
        if let Some(&(line, _)) = self
            .boundaries()
            .iter()
            .rev()
            .find(|(l, _)| *l < self.cursor_line())
        {
            self.viewport.scroll = line;
            self.cursor = line;
        }
    }

    /// #40 と同じ sticky header 用のファイル境界一覧。単一ファイル表示中は常に空
    pub fn boundaries(&self) -> &[(usize, String)] {
        if self.showing_all {
            self.all.as_ref().map_or(&[], |d| &d.boundaries)
        } else {
            &[]
        }
    }

    pub fn has_file_boundary(&self) -> bool {
        !self.boundaries().is_empty()
    }

    /// LogState::sticky_label と同じロジックを共有する (gitlane::sticky_label、下記)。
    /// 複数ファイル diff のレンダラ (render_commit) を共有しているぶん、sticky 表示のロジックも
    /// 揃えておく
    pub fn sticky_label(&self) -> Option<&str> {
        sticky_label(self.boundaries(), self.viewport.scroll)
    }

    /// A でトグル中かどうか。ステータスバーのヒント出し分けに使う
    pub fn showing_all(&self) -> bool {
        self.showing_all
    }

    pub fn search(&self) -> Option<&SearchState> {
        self.search.as_ref()
    }

    /// Search 入力中のライブプレビュー。viewer::Viewer::update_search と同じ 3 点セット
    /// (update/confirm/cancel) を GIT レーンにも持たせる (#31)。マッチ探索は
    /// viewer::search_matches をそのまま再利用し、大文字小文字の畳み込み (ASCII 限定) も揃う
    pub fn update_search(&mut self, query: &str) {
        if query.is_empty() {
            self.search = None;
            return;
        }
        let matches = self.compute_matches(query);
        self.search = Some(SearchState {
            query: query.to_string(),
            matches,
            current: None,
        });
    }

    /// Enter で確定。現在のスクロール位置以降の最初のマッチへジャンプ (なければ先頭へ wrap)
    pub fn confirm_search(&mut self) {
        let Some(search) = &self.search else {
            return;
        };
        if search.matches.is_empty() {
            return;
        }
        let scroll = self.viewport.scroll;
        let idx = search
            .matches
            .iter()
            .position(|m| m.line >= scroll)
            .unwrap_or(0);
        let line = search.matches[idx].line;
        if let Some(search) = &mut self.search {
            search.current = Some(idx);
        }
        self.center_on_line(line);
    }

    pub fn cancel_search(&mut self) {
        self.search = None;
    }

    pub fn next_match(&mut self) {
        self.step_match(1);
    }

    pub fn prev_match(&mut self) {
        self.step_match(-1);
    }

    fn step_match(&mut self, delta: isize) {
        let Some(search) = &self.search else {
            return;
        };
        if search.matches.is_empty() {
            return;
        }
        let Some(current) = search.current else {
            return;
        };
        let len = search.matches.len() as isize;
        let next = (current as isize + delta).rem_euclid(len) as usize;
        let line = search.matches[next].line;
        if let Some(search) = &mut self.search {
            search.current = Some(next);
        }
        self.center_on_line(line);
    }

    fn center_on_line(&mut self, line: usize) {
        let last = self.line_count().saturating_sub(1);
        self.viewport.center_on(line, last);
        self.cursor = line.min(last);
    }

    // 検索対象は常に「今表示している inline 行」(単一ファイル/まとめ diff の両方に対応)。
    // side-by-side は左右が独立ドキュメントで一意な行位置を持たないため対象にしない
    // (呼び出し側 on_git_key が side_by_side_active 中は `/` 自体を出さない)
    fn compute_matches(&self, query: &str) -> Vec<Match> {
        let plain: Vec<String> = self.lines().iter().map(line_plain_text).collect();
        search_matches(&plain, query)
    }

    // ファイル切替・基準切替・まとめ表示トグルの後に、同じクエリでマッチを再計算する
    // (viewer::Viewer::recompute_search と同じ意図)
    fn recompute_search(&mut self) {
        let Some(query) = self.search.as_ref().map(|s| s.query.clone()) else {
            return;
        };
        let matches = self.compute_matches(&query);
        if let Some(search) = &mut self.search {
            let current = search
                .current
                .map(|idx| idx.min(matches.len().saturating_sub(1)));
            search.current = if matches.is_empty() { None } else { current };
            search.matches = matches;
        }
    }

    /// v: inline ⇔ side-by-side 切替 (ユーザーの意図のトグル。実際に側で描けるかは
    /// side_by_side_active が幅を見て決める)
    pub fn toggle_side_by_side(&mut self) {
        self.side_by_side = !self.side_by_side;
        self.side_wrap_cache = None;
        // inline と side-by-side では同じ行 index が別の行を指すので、選択は持ち越さない
        self.selection_anchor = None;
    }

    /// side-by-side をユーザーが要求しているか (幅不足で実際は inline に落ちていても true)。
    /// ui 側がタイトルに「幅が足りず inline」ヒントを出すかどうかの判定に使う
    pub fn side_by_side_requested(&self) -> bool {
        self.side_by_side
    }

    /// 実際に side-by-side で描けるか。トグルが on でも各カラムが gutter 込み 40 桁を
    /// 切るほど狭ければ inline に自動で戻す (ペイン幅のドラッグリサイズで壊れないため)。
    /// まとめ diff 表示中 (showing_all) は常に false — 複数ファイルの左右対応を取る意味がない
    /// (LOG レーンの render_commit を inline 専用にしているのと同じ理由)
    pub fn side_by_side_active(&self) -> bool {
        !self.showing_all
            && self.side_by_side
            && self.current.is_some()
            && self.column_width() >= MIN_SIDE_BY_SIDE_COLUMN
    }

    /// side-by-side 1 カラムの char 幅 (gutter 込み)。セパレータ 1 桁を引いて半分に割る
    pub fn column_width(&self) -> usize {
        self.viewport.width.saturating_sub(1) / 2
    }

    pub fn side_lines(&self) -> (&[Line<'static>], &[Line<'static>]) {
        match &self.current {
            Some(diff) => (&diff.side.left, &diff.side.right),
            None => (&[], &[]),
        }
    }

    pub fn side_gutter_widths(&self) -> (usize, usize) {
        self.current.as_ref().map_or((0, 0), |d| {
            (d.side.left_gutter_width, d.side.right_gutter_width)
        })
    }

    pub fn side_hunks(&self) -> &[usize] {
        self.current
            .as_ref()
            .map_or(&[], |d| d.side.hunks.as_slice())
    }

    /// side-by-side + wrap の実測 (行数・hunk 位置) を ui 側が毎フレーム書き戻す。
    /// scroll のクランプ・n/N の hunk ジャンプが常に「直前フレームで実際に描いた行」を
    /// 基準にできるようにする (viewport.height/width と同じ ui→app のパターン)
    pub fn set_side_wrap_cache(&mut self, len: usize, hunks: Vec<usize>) {
        self.side_wrap_cache = Some((len, hunks));
    }
}

/// render_commit が返す境界一覧から、viewport.scroll 以下で最大の index を二分探索で引き、
/// そのファイルのラベルを返す。LOG レーン (LogState::sticky_label) と GIT レーンのまとめ diff
/// (GitState::sticky_label) の両方が同じ境界一覧の形を使うので、探索ロジックもここに 1 つだけ置く
pub(crate) fn sticky_label(boundaries: &[(usize, String)], scroll: usize) -> Option<&str> {
    let idx = boundaries.partition_point(|&(line, _)| line <= scroll);
    (idx > 0).then(|| boundaries[idx - 1].1.as_str())
}

// 生 diff のファイルヘッダが指すパスが、期待する repo 相対パスと一致するか。
// `git::file_diff` は untracked ファイルで `git diff --no-index -- /dev/null <絶対パス>` に
// フォールバックし、その出力のヘッダには絶対パスがそのまま載る。これを `git apply --cached` に
// 通すと repo 外のパスを作ろうとして失敗するので、hunk 単位の対象から外すための判定に使う。
// 新規ファイルは `+++ b/<path>`、削除ファイルは `--- a/<path>` 側にしかパスが出ない。
// git が特殊文字を含むパスをクォートした場合は strip_prefix が外れて false になる = 拒否側
// (誤って別のパスへ apply するより、ツリー側の Space でファイル単位に stage してもらう方が安全)
fn header_path_matches(raw: &[String], rel: &str) -> bool {
    let header = raw
        .iter()
        .find_map(|l| l.strip_prefix("+++ b/"))
        .or_else(|| raw.iter().find_map(|l| l.strip_prefix("--- a/")));
    // Path::display() は Windows で `\` 区切りになるが diff のヘッダは常に `/` なので揃える
    header.is_some_and(|path| path == rel.replace('\\', "/"))
}

// TextPane の highlight_matches が前提とする「span[1..] を連結すると本文」を使って、
// 検索対象の plain テキストを組み立てる。word-level ハイライト (#29) で複数 span に
// 分割された行でも、連結すれば normalize 済みの content と同じ文字列に戻るため桁がズレない
fn line_plain_text(line: &Line<'static>) -> String {
    line.spans
        .iter()
        .skip(1)
        .map(|s| s.content.as_ref())
        .collect()
}
