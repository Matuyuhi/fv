//! ワークスペース横断検索の走査本体 (バックグラウンドスレッド側)。
//! インデックスは持たず、呼ばれるたびに root 以下を歩いて読む。「大きい repo で遅い」の主因は
//! 照合ではなく走査と読み込みなので、逐次ではなく `ignore` の並列 walker (ripgrep と同じもの)
//! で歩き、見つかったファイルから順に channel へ流す。結果を待たずに最初のヒットから見せる
//! ため、1 ファイルぶんのヒットを 1 メッセージとして送る (完了時に 1 回だけ送るのではない)。
//!
//! 設計メモは docs/design/workspace-grep.md、恒久的な要約は CLAUDE.md「ワークスペース横断検索」節。

use std::fs::File;
use std::io::Read;
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
            // 読み込みバッファはワーカーごとに 1 本を使い回す (ファイルごとに確保しない)
            let mut buf = Vec::new();
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
                if let Some(file) = search_entry(&entry, &root, &needle, &mut buf) {
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
/// 同じ規則で、どちらで探しても同じ行に当たることを保証する。
/// ファイルごとに写しを作るかどうか (fold / expand_tabs) はクエリだけで決まるので、
/// ここで 1 度だけ判定しておく
#[derive(Clone)]
struct Needle {
    query: String,
    /// 大小無視のときは小文字に畳んだもの。ASCII の畳み込みはバイト長を変えないので、
    /// 畳んだ側で見つけたバイト位置をそのまま元のテキストに当てられる
    folded: String,
    /// ファイル側も小文字に畳む必要があるか。smart-case で大小無視でも、クエリに ASCII の
    /// 英字が 1 つも無ければ (記号だけ・日本語だけ) 畳んでも何も変わらないので写しを作らない
    fold: bool,
    /// タブを空白に展開した写しの上で探す必要があるか。展開で増減するのは空白だけなので、
    /// クエリに空白が無ければ生のテキストの一致と plain の一致は 1:1 に対応する
    expand_tabs: bool,
}

impl Needle {
    fn new(query: &str) -> Self {
        let ignore_case = !query.chars().any(|c| c.is_uppercase());
        let fold = ignore_case && query.chars().any(|c| c.is_ascii_alphabetic());
        let folded = if fold {
            query.to_ascii_lowercase()
        } else {
            query.to_string()
        };
        Self {
            query: query.to_string(),
            folded,
            fold,
            expand_tabs: query.contains(' '),
        }
    }

    /// バイト列の照合に使う形 (fold 時は小文字)
    fn bytes(&self) -> &[u8] {
        self.folded.as_bytes()
    }
}

/// 1 ファイルを読んで照合する。読めない・バイナリ・大きすぎるものは None (scanned に数えない)。
/// ヒットが無ければ空の hits を返す (scanned には数える)。
/// サイズは stat せず「上限 + 1 バイトまで読んで溢れたら捨てる」で判定する — 10 万ファイル級では
/// ファイルごとの stat 1 回が積み上がるうえ、open + read は結局要るので stat の情報は余分
fn search_entry(
    entry: &DirEntry,
    root: &Path,
    needle: &Needle,
    buf: &mut Vec<u8>,
) -> Option<FileHits> {
    let file = File::open(entry.path()).ok()?;
    buf.clear();
    file.take(MAX_FILE_BYTES + 1).read_to_end(buf).ok()?;
    if buf.len() as u64 > MAX_FILE_BYTES {
        return None;
    }
    if buf[..buf.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return None;
    }
    let rel = entry.path().strip_prefix(root).ok()?.to_path_buf();
    Some(FileHits {
        path: rel,
        hits: search_text(buf, needle),
    })
}

// ファイル全体を 1 本のバイト列として流し、当たった行だけを文字列に起こして行単位の照合
// (line_matches) にかけ直す。ファイル丸ごとの UTF-8 検証・小文字化の写し・行ごとの
// `Vec<char>` 確保はどれもファイルの大きさに比例するので、全て「当たった行だけ」に限る
// (ヒットの無い大多数のファイルはバイト走査 1 回で抜ける)。
// 非 UTF-8 のファイルも行単位で lossy 変換するので落とさない
fn search_text(bytes: &[u8], needle: &Needle) -> Vec<Hit> {
    // プリフィルタも VIEW と同じ plain (タブ展開済み) の上で行う。生テキストのままだと
    // 「空白 4 つ + foo」のクエリが `\tfoo` の行に当たらず、`/` では見つかるのに
    // 横断検索では出ない、という食い違いになる。展開しても改行の位置は変わらないので
    // 行の切り出しはこの写しの上でそのまま行える。写しを作るのはクエリに空白がある時だけ
    // (Needle::expand_tabs) — ファイルの大きさぶんの確保をファイルごとに払わないため
    let hay: std::borrow::Cow<[u8]> = if needle.expand_tabs && bytes.contains(&b'\t') {
        expand_tabs(bytes).into()
    } else {
        bytes.into()
    };
    let hay = hay.as_ref();
    let mut hits = Vec::new();
    let mut line_no = 0usize;
    let mut line_start = 0usize;
    let mut cursor = 0usize;
    while let Some(found) = find_at(&hay[cursor..], needle.bytes(), needle.fold) {
        let at = cursor + found;
        line_no += hay[line_start..at].iter().filter(|&&b| b == b'\n').count();
        line_start = hay[..at]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i + 1);
        let line_end = hay[at..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(hay.len(), |i| at + i);
        let plain = text::normalize(&String::from_utf8_lossy(&hay[line_start..line_end]));
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
        if cursor >= hay.len() {
            break;
        }
    }
    hits
}

// タブを text::TAB_EXPANDED に置き換えた写し (`str::replace` のバイト列版)
fn expand_tabs(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 8);
    for &b in bytes {
        if b == b'\t' {
            out.extend_from_slice(text::TAB_EXPANDED.as_bytes());
        } else {
            out.push(b);
        }
    }
    out
}

/// hay の中で needle が最初に現れる位置。fold なら ASCII の大小を無視する (needle は小文字済み)。
/// 先頭バイトの候補を find_byte2 で拾い、そこだけ全長を突き合わせる。std の `str::find` は
/// &str を要求するので (= ファイル丸ごとの UTF-8 検証が要る)、バイト列のまま探せるよう自前で持つ。
/// needle の先頭は ASCII か多バイト文字の先頭バイトなので、候補位置は常に char 境界になる
fn find_at(hay: &[u8], needle: &[u8], fold: bool) -> Option<usize> {
    let first = needle[0];
    let (a, b) = if fold && first.is_ascii_alphabetic() {
        (first, first.to_ascii_uppercase())
    } else {
        (first, first)
    };
    let mut from = 0;
    while from + needle.len() <= hay.len() {
        // 残りが needle より短い位置は候補にしない
        let limit = hay.len() - needle.len() + 1;
        let at = from + find_byte2(&hay[from..limit], a, b)?;
        let window = &hay[at..at + needle.len()];
        let matched = if fold {
            window.eq_ignore_ascii_case(needle)
        } else {
            window == needle
        };
        if matched {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

/// a か b のどちらかのバイトを探す (memchr2 相当)。8 バイトずつ「ゼロバイトを含むか」の
/// SWAR 判定で飛ばし、含む語だけバイト単位で見る。memchr クレートを足さずに済ませるための
/// 最小限の実装で、ripgrep の SIMD 版ほど速くはないが、1 バイトずつ比べるループよりは数倍速い
fn find_byte2(hay: &[u8], a: u8, b: u8) -> Option<usize> {
    const LO: u64 = 0x0101_0101_0101_0101;
    const HI: u64 = 0x8080_8080_8080_8080;
    let ra = u64::from_ne_bytes([a; 8]);
    let rb = u64::from_ne_bytes([b; 8]);
    let has_zero = |w: u64| w.wrapping_sub(LO) & !w & HI != 0;
    let mut i = 0;
    while i + 8 <= hay.len() {
        let w = u64::from_ne_bytes(hay[i..i + 8].try_into().expect("8 bytes"));
        if has_zero(w ^ ra) || has_zero(w ^ rb) {
            return hay[i..i + 8]
                .iter()
                .position(|&c| c == a || c == b)
                .map(|p| i + p);
        }
        i += 8;
    }
    hay[i..]
        .iter()
        .position(|&c| c == a || c == b)
        .map(|p| i + p)
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
        search_text(text.as_bytes(), &Needle::new(query))
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
    fn copies_are_made_only_when_the_query_needs_them() {
        assert!(Needle::new("foo").fold);
        assert!(!Needle::new("Foo").fold);
        // 英字を含まないクエリは畳んでも変わらない (日本語・記号)
        assert!(!Needle::new("->").fold);
        assert!(!Needle::new("日本語").fold);
        assert!(!Needle::new("foo").expand_tabs);
        assert!(Needle::new("fn main").expand_tabs);
        // 写しを省いても一致は同じ
        assert_eq!(hits("\t日本語\n", "日本語"), vec![(0, 4, 7)]);
        assert_eq!(hits("a -> b\n", "->"), vec![(0, 2, 4)]);
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
        let found = search_text(text.as_bytes(), &Needle::new("a"));
        assert_eq!(found.len(), MAX_HITS_PER_FILE);
        // 2 行目は 1 行目で枠を使い切っているので数えない
        let text = format!("{}\nfoo\n", "foo ".repeat(MAX_HITS_PER_FILE));
        let found = search_text(text.as_bytes(), &Needle::new("foo"));
        assert_eq!(found.len(), MAX_HITS_PER_FILE);
        assert!(found.iter().all(|h| h.line == 0));
    }

    #[test]
    fn clipped_hit_keeps_the_original_column() {
        let text = format!("{}foo\n", "x".repeat(1000));
        let hit = &search_text(text.as_bytes(), &Needle::new("foo"))[0];
        assert_eq!(hit.col, 1000);
        assert_ne!(hit.start_col, hit.col);
    }

    #[test]
    fn byte_search_agrees_with_str_find_and_ignores_case_when_folding() {
        let hay = b"xxFoo foo FOO fo";
        assert_eq!(find_at(hay, b"foo", false), Some(6));
        assert_eq!(find_at(hay, b"foo", true), Some(2));
        assert_eq!(find_at(hay, b"fox", true), None);
        // 末尾で needle が収まらない位置は候補にしない
        assert_eq!(find_at(b"fo", b"foo", true), None);
        // 8 バイト境界を跨ぐ・語の途中にある
        let long = format!("{}Needle", "-".repeat(13));
        assert_eq!(find_at(long.as_bytes(), b"needle", true), Some(13));
        assert_eq!(find_byte2(b"abcdefghijklmnop", b'p', b'z'), Some(15));
        assert_eq!(find_byte2(b"abcdefghijklmnop", b'Z', b'c'), Some(2));
        assert_eq!(find_byte2(b"abc", b'x', b'y'), None);
    }

    #[test]
    fn invalid_utf8_files_are_still_searched_line_by_line() {
        let mut bytes = b"ok foo\n\xff\xfe foo\n".to_vec();
        bytes.extend_from_slice(b"\xc3 last foo");
        let found = search_text(&bytes, &Needle::new("foo"));
        let lines: Vec<usize> = found.iter().map(|h| h.line).collect();
        assert_eq!(lines, vec![0, 1, 2]);
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
        let hit = &search_text(text.as_bytes(), &Needle::new("foo"))[0];
        assert!(hit.clipped);
        assert_eq!(hit.text.chars().count(), 43);
        assert_eq!(&hit.text[hit.start_col..hit.end_col], "foo");
    }
}
