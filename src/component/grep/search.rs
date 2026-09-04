//! ワークスペース横断検索の走査本体 (バックグラウンドスレッド側)。
//! インデックスは持たず、呼ばれるたびに root 以下を歩いて読む。「大きい repo で遅い」の主因は
//! 照合ではなく走査と読み込みなので、逐次ではなく `ignore` の並列 walker (ripgrep と同じもの)
//! で歩き、見つかったファイルから順に channel へ流す。結果を待たずに最初のヒットから見せる
//! ため、1 ファイルぶんのヒットを 1 メッセージとして送る (完了時に 1 回だけ送るのではない)。
//!
//! 設計メモは docs/design/workspace-grep.md、恒久的な要約は CLAUDE.md「ワークスペース横断検索」節。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use ignore::{DirEntry, WalkState};

use crate::component::tree::ScanOptions;
use crate::component::viewer::line_matches;
use crate::text;

/// これ以上のヒットは集めない (走査ごと打ち切る)。1 文字のクエリを巨大 repo に投げた時に
/// 結果がメモリと画面を埋め尽くさないための上限で、まとめ diff (`A`) の 20000 行上限と同じ発想
pub(crate) const MAX_HITS: usize = 5000;
/// 1 ファイルあたりのヒット上限。minified な 1 行ファイル等で 1 ファイルが上限を独占しないため
const MAX_HITS_PER_FILE: usize = 200;
/// これより大きいファイルは読まない (1 スレッドが一時的に持つメモリの上限でもある)
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// バイナリ判定に見る先頭バイト数 (ripgrep/grep と同じ「NUL があればバイナリ」)
const BINARY_SNIFF_BYTES: usize = 8 * 1024;
/// 一覧に載せる 1 行の最大 char 数。これを超える行はマッチの周辺だけを切り出す
/// (長い行を丸ごと持つと、ヒット 5000 件でも数 MB を抱えうるため)
const MAX_LINE_CHARS: usize = 400;
/// 切り出す時にマッチの左に残す char 数
const LINE_CONTEXT_BEFORE: usize = 40;

/// 1 ヒット。列は plain (タブ展開済み・normalize 済み) の char インデックスで、
/// VIEW の検索マッチ (`viewer::Match`) と同じ座標系 — 開いた先で同じ位置を光らせるため
pub struct Hit {
    /// 0-origin
    pub line: usize,
    /// 行の中での一致位置 (plain の char 桁)。開いた先で同じ一致を現在位置にするために使う
    pub col: usize,
    /// text 内での強調範囲。切り出していなければ start_col == col
    pub start_col: usize,
    pub end_col: usize,
    /// 表示用の行本文 (plain)。MAX_LINE_CHARS を超える行は切り出し済みで、その場合
    /// start_col/end_col はこの text 内の座標に直してある (`clipped` が true)
    pub text: String,
    pub clipped: bool,
}

pub struct FileHits {
    /// root からの相対パス
    pub path: PathBuf,
    pub hits: Vec<Hit>,
}

pub enum Message {
    File(FileHits),
    /// 走査完了。scanned は実際に中身を読んだファイル数、truncated は MAX_HITS で打ち切ったか
    Done {
        scanned: usize,
        truncated: bool,
    },
}

/// 走査を起こし、結果の受け口を返す。cancel を立てると走査中のスレッドが次のファイルで止まる
/// (Done は送られないことがある。クエリを打ち直した時の古い走査は捨てるだけなので構わない)
pub(super) fn spawn(
    root: PathBuf,
    opts: ScanOptions,
    query: String,
    cancel: Arc<AtomicBool>,
) -> Receiver<Message> {
    let (tx, rx) = mpsc::channel();
    // walker.run はスレッドを内部で複数起こしたうえで**呼び出し側をブロックする**ので、
    // それ自体をさらに 1 本のスレッドへ出す (UI スレッドを止めない)
    thread::spawn(move || {
        let scanned = Arc::new(AtomicUsize::new(0));
        let hits = Arc::new(AtomicUsize::new(0));
        let truncated = Arc::new(AtomicBool::new(false));
        let needle = Needle::new(&query);
        let walker = opts.walker(&root).build_parallel();
        walker.run(|| {
            let tx = tx.clone();
            let root = root.clone();
            let needle = needle.clone();
            let cancel = Arc::clone(&cancel);
            let scanned = Arc::clone(&scanned);
            let hits = Arc::clone(&hits);
            let truncated = Arc::clone(&truncated);
            Box::new(move |entry| {
                if cancel.load(Ordering::Relaxed) || truncated.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    return WalkState::Continue;
                }
                if let Some(file) = search_entry(&entry, &root, &needle) {
                    scanned.fetch_add(1, Ordering::Relaxed);
                    if file.hits.is_empty() {
                        return WalkState::Continue;
                    }
                    let total =
                        hits.fetch_add(file.hits.len(), Ordering::Relaxed) + file.hits.len();
                    // 上限を跨いだファイルまでは送る (打ち切りの表示は Done 側で出す)
                    let _ = tx.send(Message::File(file));
                    if total >= MAX_HITS {
                        truncated.store(true, Ordering::Relaxed);
                        return WalkState::Quit;
                    }
                }
                WalkState::Continue
            })
        });
        if !cancel.load(Ordering::Relaxed) {
            let _ = tx.send(Message::Done {
                scanned: scanned.load(Ordering::Relaxed),
                truncated: truncated.load(Ordering::Relaxed),
            });
        }
    });
    rx
}

/// クエリの照合に要る形。smart-case (全部小文字なら大小無視) は VIEW の `/` (search_matches) と
/// 同じ規則で、どちらで探しても同じ行に当たることを保証する
#[derive(Clone)]
struct Needle {
    query: String,
    /// 大小無視のときは小文字に畳んだもの。ASCII の畳み込みはバイト長を変えないので、
    /// 畳んだ側で見つけたバイト位置をそのまま元のテキストに当てられる
    folded: String,
    ignore_case: bool,
}

impl Needle {
    fn new(query: &str) -> Self {
        let ignore_case = !query.chars().any(|c| c.is_uppercase());
        let folded = if ignore_case {
            query.to_ascii_lowercase()
        } else {
            query.to_string()
        };
        Self {
            query: query.to_string(),
            folded,
            ignore_case,
        }
    }
}

/// 1 ファイルを読んで照合する。読めない・バイナリ・大きすぎるものは None (scanned に数えない)。
/// ヒットが無ければ空の hits を返す (scanned には数える)
fn search_entry(entry: &DirEntry, root: &Path, needle: &Needle) -> Option<FileHits> {
    let size = entry.metadata().ok()?.len();
    if size > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(entry.path()).ok()?;
    if bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return None;
    }
    let rel = entry.path().strip_prefix(root).ok()?.to_path_buf();
    let text = String::from_utf8_lossy(&bytes);
    Some(FileHits {
        path: rel,
        hits: search_text(&text, needle),
    })
}

// ファイル全体を 1 本の文字列として `str::find` (two-way 法) で流し、当たった行だけを
// 行単位の照合 (line_matches) にかけ直す。行ごとに小文字化・char 化する照合を
// ファイル全体に使うと、ヒットの無い大多数の行にも確保が付いて回るため
fn search_text(text: &str, needle: &Needle) -> Vec<Hit> {
    // プリフィルタも VIEW と同じ plain (タブ展開済み) の上で行う。生テキストのままだと
    // 「空白 4 つ + foo」のクエリが `\tfoo` の行に当たらず、`/` では見つかるのに
    // 横断検索では出ない、という食い違いになる。展開しても改行の位置は変わらないので
    // 行の切り出しはこの写しの上でそのまま行える
    let text: std::borrow::Cow<str> = if text.contains('\t') {
        text.replace('\t', text::TAB_EXPANDED).into()
    } else {
        text.into()
    };
    let text = text.as_ref();
    let folded: std::borrow::Cow<str> = if needle.ignore_case {
        text.to_ascii_lowercase().into()
    } else {
        text.into()
    };
    let mut hits = Vec::new();
    // 直前に処理した行の末尾 (バイト)。同じ行の 2 つ目以降の一致は行単位の照合が拾うので飛ばす
    let mut line_no = 0usize;
    let mut line_start = 0usize;
    let mut cursor = 0usize;
    while let Some(found) = folded[cursor..].find(&needle.folded) {
        let at = cursor + found;
        line_no += text.as_bytes()[line_start..at]
            .iter()
            .filter(|&&b| b == b'\n')
            .count();
        line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
        let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);
        let plain = text::normalize(&text[line_start..line_end]);
        // 上限までしか一致を数えない (minified な 1 行に 1 文字のクエリを投げても、
        // 残り枠ぶんで走査を止める)
        let remaining = MAX_HITS_PER_FILE - hits.len();
        for (start_col, end_col) in line_matches(&plain, &needle.query).take(remaining) {
            hits.push(clip(&plain, line_no, start_col, end_col));
        }
        if hits.len() >= MAX_HITS_PER_FILE {
            return hits;
        }
        // 次の行頭から続ける (この行の残りの一致は上で数え終えている)
        cursor = line_end;
        line_start = line_end;
        if cursor >= text.len() {
            break;
        }
    }
    hits
}

// 一覧に載せる本文を組み立てる。長い行はマッチの手前 LINE_CONTEXT_BEFORE 文字から切り出し、
// 座標も切り出した text 内のものに直す
fn clip(plain: &str, line: usize, start_col: usize, end_col: usize) -> Hit {
    let len = plain.chars().count();
    if len <= MAX_LINE_CHARS {
        return Hit {
            line,
            col: start_col,
            start_col,
            end_col,
            text: plain.to_string(),
            clipped: false,
        };
    }
    let from = start_col.saturating_sub(LINE_CONTEXT_BEFORE);
    let text: String = plain.chars().skip(from).take(MAX_LINE_CHARS).collect();
    Hit {
        line,
        col: start_col,
        start_col: start_col - from,
        end_col: (end_col - from).min(MAX_LINE_CHARS),
        text,
        clipped: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(text: &str, query: &str) -> Vec<(usize, usize, usize)> {
        search_text(text, &Needle::new(query))
            .iter()
            .map(|h| (h.line, h.start_col, h.end_col))
            .collect()
    }

    #[test]
    fn finds_every_occurrence_per_line_with_line_numbers() {
        let text = "alpha\nfoo bar foo\n\nfoo\n";
        assert_eq!(hits(text, "foo"), vec![(1, 0, 3), (1, 8, 11), (3, 0, 3)]);
    }

    #[test]
    fn smart_case_matches_viewer_search() {
        let text = "Foo\nfoo\nFOO\n";
        assert_eq!(hits(text, "foo").len(), 3);
        assert_eq!(hits(text, "Foo"), vec![(0, 0, 3)]);
    }

    #[test]
    fn columns_are_in_tab_expanded_plain_coordinates() {
        let text = "\tfoo\n";
        assert_eq!(hits(text, "foo"), vec![(0, 4, 7)]);
    }

    #[test]
    fn query_with_expanded_tab_matches_a_tab_in_the_file() {
        // `/` は plain (タブ → 空白 4) の上で探すので、横断検索も同じ行に当たること
        assert_eq!(hits("\tfoo\n", "    foo"), vec![(0, 0, 7)]);
        assert_eq!(hits("a\tb\n", "a b"), Vec::new());
    }

    #[test]
    fn per_file_cap_stops_counting_inside_a_huge_line() {
        let text = "a".repeat(100_000);
        let found = search_text(&text, &Needle::new("a"));
        assert_eq!(found.len(), MAX_HITS_PER_FILE);
        // 2 行目は 1 行目で枠を使い切っているので数えない
        let text = format!("{}\nfoo\n", "foo ".repeat(MAX_HITS_PER_FILE));
        let found = search_text(&text, &Needle::new("foo"));
        assert_eq!(found.len(), MAX_HITS_PER_FILE);
        assert!(found.iter().all(|h| h.line == 0));
    }

    #[test]
    fn clipped_hit_keeps_the_original_column() {
        let text = format!("{}foo\n", "x".repeat(1000));
        let hit = &search_text(&text, &Needle::new("foo"))[0];
        assert_eq!(hit.col, 1000);
        assert_ne!(hit.start_col, hit.col);
    }

    #[test]
    fn last_line_without_newline_is_searched() {
        assert_eq!(hits("a\nfoo", "foo"), vec![(1, 0, 3)]);
    }

    #[test]
    fn non_ascii_before_match_keeps_char_columns() {
        // 小文字化はバイト長を変えず、列は char で数える
        assert_eq!(hits("日本語 foo\n", "foo"), vec![(0, 4, 7)]);
    }

    #[test]
    fn long_lines_are_clipped_around_the_match() {
        let text = format!("{}foo\n", "x".repeat(1000));
        let hit = &search_text(&text, &Needle::new("foo"))[0];
        assert!(hit.clipped);
        assert_eq!(hit.text.chars().count(), 43);
        assert_eq!(&hit.text[hit.start_col..hit.end_col], "foo");
    }
}
