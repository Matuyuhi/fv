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

use patch::{PatchError, build_line_patch};
use render::{classify, render_inline};
use side::render_side_by_side;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ratatui::style::Color;
use ratatui::text::Line;

use crate::component::viewer::{Match, SearchState, Viewport, rowcursor, search_matches};
use crate::git::{self, DiffBase};
use crate::text;
use crate::widget::text_pane::line_body;

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
    /// inline 表示行 → raw の index。`body` は raw を classify で絞った列で、
    /// `render_inline` はその 1 要素につき 1 行を組み立てるので、この対応は 1:1 で取れる。
    /// 行単位 stage (`Enter`) がカーソル行から生 diff の行へ戻るのに使う
    raw_index: Vec<usize>,
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

/// `Enter` (GIT レーン右ペイン) の対象。HunkPatch と分けてあるのは、行単位でしか
/// 起きない断り (side-by-side・変更行以外にカーソルがある) を型で表すため
pub enum LinePatch {
    Ready {
        patch: String,
        /// 実際に反映される変更行数 (notice 用)
        lines: usize,
    },
    ShowingAll,
    /// side-by-side は左右が独立した行列で、表示行 → 生 diff の対応を一意に取れない
    /// (`/` 検索を側で出さないのと同じ理由)
    SideBySide,
    NotApplicable,
    /// カーソル (または選択範囲) に `+`/`-` の行が 1 つも無い
    NoChangedLine,
    /// rename。旧側と新側でパスが違うので行だけを切り出せない
    Rename,
    /// 新規/削除ファイルを部分的に反映しようとした結果、`/dev/null` 側に行が残る組み合わせ
    /// (例: staged の新規ファイルから 1 行だけ unstage する)
    WholeFileOnly,
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
    /// フォーカス行 (今表示している行列での論理行 index)。閲覧レーンと違い diff ペインには
    /// 「今どこを見ているか」を示すものが枠の色しか無く、Space/Enter の対象も暗黙になって
    /// いたので、スクロール位置とは別にカーソルを持たせて対象を 1 行に確定させる
    cursor: usize,
    /// V で始めた行単位選択の錨。None なら選択なし (= 対象はカーソル行 1 行だけ)
    select_anchor: Option<usize>,
}

impl GitState {
    /// `wrap` に加えて右ペインの実測サイズを引き継ぐ。diff ペインは VIEW/EDIT とまったく
    /// 同じ Rect を使うので、次の描画で書き戻されるまでの間もカーソル追従 (height 依存) が
    /// 正しく効く — 0 のままだと GIT に入った直後の 1 打鍵でスクロールが飛ぶ
    pub fn new(wrap: bool, height: usize, width: usize) -> Self {
        let mut viewport = Viewport::new(wrap);
        viewport.height = height;
        viewport.width = width;
        Self {
            viewport,
            base: DiffBase::Head,
            current: None,
            side_by_side: false,
            side_wrap_cache: None,
            search: None,
            all: None,
            showing_all: false,
            cursor: 0,
            select_anchor: None,
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
        let body: Vec<(Kind, &str)> = raw.iter().filter_map(|line| classify(line)).collect();
        let raw_hunks = raw
            .iter()
            .enumerate()
            .filter(|(_, line)| line.starts_with("@@"))
            .map(|(i, _)| i)
            .collect();
        let raw_index = raw
            .iter()
            .enumerate()
            .filter(|(_, line)| classify(line).is_some())
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
            raw_index,
            stageable,
        });
        self.viewport.scroll = 0;
        self.viewport.hscroll = 0;
        self.cursor = 0;
        self.select_anchor = None;
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
        self.select_anchor = None;
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
            self.clamp_cursor();
            self.recompute_search();
            return truncated;
        }
        let Some(path) = self.current.as_ref().map(|d| d.path.clone()) else {
            return false;
        };
        // open がカーソル・選択を捨てるので、背景更新で読んでいた位置が飛ばないよう退避する
        let (scroll, cursor, anchor) = (self.viewport.scroll, self.cursor, self.select_anchor);
        self.open(root, &path);
        self.viewport.scroll = scroll.min(self.line_count().saturating_sub(1));
        self.cursor = cursor;
        self.select_anchor = anchor;
        self.clamp_cursor();
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

    /// ホイール等の「画面を動かす」操作。カーソルは連れて動かす — 置き去りにすると
    /// Space/Enter の対象が画面外へ消え、何が起きるか分からないまま押すことになる
    pub fn scroll_by(&mut self, delta: isize) {
        let last = self.line_count().saturating_sub(1);
        self.viewport.scroll_by(delta, last);
        self.clamp_cursor_into_view();
    }

    /// j/k・Ctrl+d/u: カーソルを動かし、画面はそれに追従させる
    pub fn move_cursor(&mut self, delta: isize) {
        let last = self.line_count().saturating_sub(1);
        self.cursor = (self.cursor as isize + delta).clamp(0, last as isize) as usize;
        self.ensure_cursor_visible();
    }

    pub fn jump_to_top(&mut self) {
        self.cursor = 0;
        self.viewport.scroll = 0;
    }

    pub fn jump_to_bottom(&mut self) {
        self.cursor = self.line_count().saturating_sub(1);
        self.ensure_cursor_visible();
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// V: 行単位選択の開始/解除 (vim の visual line 相当)。錨だけを持ち、範囲は
    /// カーソルとの組で毎回引く — 伸縮は j/k のカーソル移動がそのまま担う。
    /// 行単位ステージが効かない表示 (まとめ diff・side-by-side) では選択を始めさせない —
    /// 掴めても Enter が必ず断るので、ステータスバーのヒントと実際の可否が食い違う
    pub fn line_selection_available(&self) -> bool {
        !self.showing_all() && !self.side_by_side_active()
    }

    pub fn toggle_line_selection(&mut self) {
        if !self.line_selection_available() {
            return;
        }
        self.select_anchor = match self.select_anchor {
            Some(_) => None,
            None => Some(self.cursor),
        };
    }

    pub fn clear_line_selection(&mut self) {
        self.select_anchor = None;
    }

    /// 選択範囲 (両端含む)。描画側の帯と Enter の対象がこの 1 つの定義を共有する
    pub fn line_selection(&self) -> Option<(usize, usize)> {
        self.select_anchor
            .map(|anchor| (anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    pub fn selected_row_count(&self) -> Option<usize> {
        self.line_selection().map(|(from, to)| to - from + 1)
    }

    /// クリックしたペイン内 row → カーソル。sticky header の 1 行はここへ来る前に
    /// 呼び出し側 (app/mouse.rs) が差し引く
    pub fn click_row(&mut self, row: usize) {
        let (count, wrapped, width) = self.cursor_metrics();
        let line = rowcursor::line_at_row(&self.viewport, row, count, wrapped, |i| {
            self.rows_at(i, width)
        });
        self.cursor = line;
        self.ensure_cursor_visible();
    }

    // 行カーソルの追従に要る 3 つ組。side-by-side は描画側が事前に行数を揃えるので、
    // wrap 中でも視覚行 = 論理行として扱う
    fn cursor_metrics(&self) -> (usize, bool, usize) {
        let wrapped = self.viewport.wrap && !self.side_by_side_active();
        let width = self
            .viewport
            .width
            .saturating_sub(self.gutter_width())
            .max(1);
        (self.line_count(), wrapped, width)
    }

    // 論理行 i が占める視覚行数。折返し規則は描画 (text_pane) と同じ text::wrap_rows
    fn rows_at(&self, i: usize, width: usize) -> usize {
        match self.lines().get(i) {
            Some(line) => text::wrap_rows(&line_body(line), width),
            None => 1,
        }
    }

    fn ensure_cursor_visible(&mut self) {
        let (count, wrapped, width) = self.cursor_metrics();
        let scroll = rowcursor::scroll_for(&self.viewport, self.cursor, count, wrapped, |i| {
            self.rows_at(i, width)
        });
        self.viewport.scroll = scroll;
    }

    // scroll を動かした側 (ホイール) から呼ぶ逆向きの追従
    fn clamp_cursor_into_view(&mut self) {
        let (count, wrapped, width) = self.cursor_metrics();
        let cursor = rowcursor::clamp_cursor(&self.viewport, self.cursor, count, wrapped, |i| {
            self.rows_at(i, width)
        });
        self.cursor = cursor;
    }

    // 行数が変わる取り直しの後始末。選択の錨も一緒に詰める
    fn clamp_cursor(&mut self) {
        let last = self.line_count().saturating_sub(1);
        self.cursor = self.cursor.min(last);
        if let Some(anchor) = &mut self.select_anchor {
            *anchor = (*anchor).min(last);
        }
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

    /// ]: カーソルより後ろの最初の hunk header へ。無ければ位置を変えない。
    /// hunk header を画面上端に置くのは、その hunk 全体を読み始められるようにするため
    pub fn next_hunk(&mut self) {
        if let Some(&target) = self.hunks().iter().find(|&&i| i > self.cursor) {
            self.cursor = target;
            self.viewport.scroll = target;
        }
    }

    /// [: カーソルより前の最後の hunk header へ
    pub fn prev_hunk(&mut self) {
        if let Some(&target) = self.hunks().iter().rev().find(|&&i| i < self.cursor) {
            self.cursor = target;
            self.viewport.scroll = target;
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

    /// カーソル行が属する hunk の序数 (0-origin)。以前は「上端に見えている行」を基準に
    /// していたが、それだと画面を送るだけで Space の対象が黙って変わる (どの hunk を
    /// 掴んでいるのか画面から読み取れない) ため、カーソルへ寄せてある。
    /// hunks() を使うので inline / side-by-side / wrap のどの表示でも同じ序数が出る
    /// (どの表示でも hunk の並び順は生 diff と同じで、変わるのは行 index だけ)
    fn current_hunk_ordinal(&self) -> Option<usize> {
        let idx = self.hunks().partition_point(|&line| line <= self.cursor);
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

    /// Enter: カーソル行 (V で選択中ならその範囲) の変更行だけを index へ適用/取り消す
    /// パッチを組み立てる。表示行 → 生 diff は `raw_index` で戻し、未選択の変更行の
    /// 書き換えと hunk header の数え直しは patch.rs が持つ
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
        let (from, to) = self.line_selection().unwrap_or((self.cursor, self.cursor));
        let selected: BTreeSet<usize> = (from..=to)
            .filter_map(|row| diff.raw_index.get(row).copied())
            .filter(|&i| diff.raw[i].starts_with('+') || diff.raw[i].starts_with('-'))
            .collect();
        if selected.is_empty() {
            return LinePatch::NoChangedLine;
        }
        match build_line_patch(&diff.raw, &diff.raw_hunks, &selected, self.unstaging()) {
            Ok(built) => LinePatch::Ready {
                patch: built.patch,
                lines: built.lines,
            },
            Err(PatchError::Empty) => LinePatch::NoChangedLine,
            Err(PatchError::Rename) => LinePatch::Rename,
            Err(PatchError::DevNullSideNotEmpty) => LinePatch::WholeFileOnly,
        }
    }

    /// }: まとめ diff 中の次のファイル境界へ。単一ファイル表示中は boundaries が空なので no-op
    pub fn next_file(&mut self) {
        if let Some(&(line, _)) = self.boundaries().iter().find(|(l, _)| *l > self.cursor) {
            self.cursor = line;
            self.viewport.scroll = line;
        }
    }

    /// {: まとめ diff 中の前のファイル境界へ
    pub fn prev_file(&mut self) {
        if let Some(&(line, _)) = self
            .boundaries()
            .iter()
            .rev()
            .find(|(l, _)| *l < self.cursor)
        {
            self.cursor = line;
            self.viewport.scroll = line;
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
        self.cursor = line.min(last);
        self.viewport.center_on(line, last);
        self.ensure_cursor_visible();
    }

    // 検索対象は常に「今表示している inline 行」(単一ファイル/まとめ diff の両方に対応)。
    // side-by-side は左右が独立ドキュメントで一意な行位置を持たないため対象にしない
    // (呼び出し側 on_git_key が side_by_side_active 中は `/` 自体を出さない)
    fn compute_matches(&self, query: &str) -> Vec<Match> {
        let plain: Vec<String> = self.lines().iter().map(line_body).collect();
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
        let ordinal = self.current_hunk_ordinal();
        self.side_by_side = !self.side_by_side;
        self.side_wrap_cache = None;
        self.realign_cursor(ordinal);
    }

    /// w: 折返し切替。side-by-side は wrap の有無で行列そのものが変わる
    /// (side_by_side_wrapped が視覚行を展開して行数を揃え直す) ため、inline のときと違って
    /// カーソルの行 index の意味が保たれない。realign_cursor を通すのはそのため
    pub fn toggle_wrap(&mut self) {
        let realign = self.side_by_side_active();
        let ordinal = self.current_hunk_ordinal();
        self.viewport.toggle_wrap();
        self.side_wrap_cache = None;
        if realign || self.side_by_side_active() {
            self.realign_cursor(ordinal);
        }
    }

    // 表示形式が変わると行 index の意味が変わる (inline / side-by-side / side+wrap は
    // それぞれ別の行列で、同じ index が別の内容を指す)。hunk の**並び順**だけはどの表示でも
    // 生 diff と同じなので、居た hunk の先頭へ寄せ直すことで「切替前後で Space の対象が
    // 変わらない」を保つ。行単位選択は範囲の両端が同じようには写せないので畳む
    fn realign_cursor(&mut self, ordinal: Option<usize>) {
        self.select_anchor = None;
        self.cursor = ordinal
            .and_then(|o| self.hunks().get(o).copied())
            .unwrap_or(0);
        // hunk header を上端に置く (]/[ のジャンプと同じ着地の仕方)
        self.viewport.scroll = self.cursor;
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
