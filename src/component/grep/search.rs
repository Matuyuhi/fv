//! ワークスペース横断検索の走査本体 (バックグラウンドスレッド側)。
//! 「大きい repo で遅い」の主因は照合ではなく走査と読み込みなので、逐次ではなく `ignore` の
//! 並列 walker (ripgrep と同じもの) で歩き、見つかったファイルから順に channel へ流す。結果を
//! 待たずに最初のヒットから見せるため、1 ファイルぶんのヒットを 1 メッセージとして送る。
//!
//! 走査は 2 種類ある。実測 (合成 2 万ファイル・120MB、4 コア) では walk が 19ms・read が 19ms・
//! 照合は 2ms で、**コストのほぼ全てが syscall** だった。そこで読んだ内容を `Cache` に残し、
//! (a) `spawn_walk`: root を歩き直すが、stat で (mtime, size) が変わっていないファイルは読まない
//! (b) `spawn_corpus`: 前回完走した走査のファイル一覧 (`Corpus`) をそのままメモリ上で照合する
//!     (walk も stat も read もしない。FS 監視が生きていて変更が無い間だけ許される)
//! の 2 経路に分けた。どちらを使うかは GrepState (mod.rs) が決める。
//!
//! 設計メモは docs/design/workspace-grep.md、恒久的な要約は CLAUDE.md「ワークスペース横断検索」節。

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::SystemTime;

use ignore::WalkState;

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
/// content cache に残す本文の総量の上限。超えたぶんは従来通り走査のたびに読む
/// (どのファイルが残るかは到着順なので決定的ではないが、残った分だけ read が減る)
const MAX_CACHE_BYTES: usize = 256 * 1024 * 1024;

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

pub(super) enum Message {
    File(FileHits),
    /// ヒットの送出が終わった。scanned は実際に中身を照合したファイル数、truncated は MAX_HITS で
    /// 打ち切ったか。打ち切った時はこの後も走査 (corpus の組み立て) だけ静かに続く
    Done {
        scanned: usize,
        truncated: bool,
    },
    /// root 以下を最後まで歩き切った時の、その時点のファイル一覧。キャンセルされた走査は送らない
    /// (途中までの一覧では「無い」と「まだ歩いていない」が区別できないため)
    Corpus(Corpus),
    /// corpus 経路で dirty だった項目を読み直した結果。呼び出し側が一覧の該当項目を差し替えて
    /// dirty を消す (消さないと変更されたファイルが単調に増え、毎回それら全件を読み直す)
    Refreshed(Vec<Arc<Entry>>),
}

/// 完走した走査 1 回ぶんのファイル一覧。`spawn_corpus` はこれをそのまま照合対象にするので、
/// root を歩き直さずに済む。並びは到着順 (ワーカー任せ) で意味を持たない
pub(super) type Corpus = Vec<Arc<Entry>>;

/// 1 ファイルぶんの cache 項目。(mtime, size) が一致する間は中身を読み直さない
pub(super) struct Entry {
    /// root からの相対パス。cache の鍵と共有する (ファイルごとの確保を 1 回にする)
    pub rel: Arc<Path>,
    mtime: Option<SystemTime>,
    size: u64,
    content: Content,
}

enum Content {
    /// 読んで残してあるテキスト。読んだ Vec をそのまま持つ (写しを作らない)
    Text(Vec<u8>),
    /// テキストだが cache の上限に収まらなかった。走査のたびに読む
    Uncached,
    /// バイナリ・大きすぎる。読まない (照合対象にも scanned にも数えない)
    Skip,
}

impl Entry {
    /// 本文を持たない写し。dirty になった (次の照合で読み直す) 項目の古い本文を snapshot 側で
    /// 持ち続けないため。stat の値は残すが、dirty の経路では cache を引き直すので使われない
    pub(super) fn without_content(&self) -> Arc<Entry> {
        Arc::new(Entry {
            rel: Arc::clone(&self.rel),
            mtime: self.mtime,
            size: self.size,
            content: Content::Skip,
        })
    }
}

type Map = HashMap<Arc<Path>, Arc<Entry>>;

/// 読んだ内容を走査を跨いで残す (path → Entry)。持ち主は GrepState で、走査スレッドはこれを
/// 共有して「読む前に stat で照合し、変わっていなければ前回の中身を使う」。
/// 走査中はファイルごとにロックを取らない — map は不変のスナップショットとして Arc で配り、
/// 読み直した項目はワーカーが手元に溜めて走査の終わりに 1 回で差し替える (実測で、ファイルごとの
/// ロック 3 回 + 本文の memcpy が cold の走査を 2 倍に遅くしていた)
#[derive(Default)]
pub(super) struct Cache {
    map: Mutex<Arc<Map>>,
    /// Text として残している本文の合計 (上限 MAX_CACHE_BYTES の判定用)。走査中は読む前に
    /// 予約として足すので概算で、差し替え時に正確な値へ戻す
    bytes: AtomicUsize,
}

impl Cache {
    fn snapshot(&self) -> Arc<Map> {
        Arc::clone(&self.map.lock().unwrap())
    }

    /// 読む前に呼ぶ。上限に収まるなら予約して true。読み取りだけで弾ける時は RMW を発行しない
    /// (4 ワーカーが同じカウンタを叩くので、ファイルごとの RMW は cache line の奪い合いになる)
    fn reserve(&self, len: usize) -> bool {
        if self.bytes.load(Ordering::Relaxed) + len > MAX_CACHE_BYTES {
            return false;
        }
        if self.bytes.fetch_add(len, Ordering::Relaxed) + len <= MAX_CACHE_BYTES {
            return true;
        }
        self.bytes.fetch_sub(len, Ordering::Relaxed);
        false
    }

    fn release(&self, len: usize) {
        self.bytes.fetch_sub(len, Ordering::Relaxed);
    }

    /// 走査の終わりに map を差し替える。完走していれば seen だけ (消えた・無視対象になった
    /// ファイルはここで落ちる)、途中で止まったなら前の map に**読み直したぶんだけ**重ねる
    /// (読んだぶんは無駄にしない)。打鍵のたびにキャンセルされる走査で、cache がそのまま
    /// 当たった項目まで map を複製し直さないよう、既に同じ Arc が入っているものは省く
    fn replace(&self, seen: Vec<Arc<Entry>>, complete: bool) {
        let mut map = if complete {
            Map::with_capacity(seen.len())
        } else {
            let known = self.snapshot();
            let fresh: Vec<Arc<Entry>> = seen
                .into_iter()
                .filter(|e| !known.get(&e.rel).is_some_and(|k| Arc::ptr_eq(k, e)))
                .collect();
            if fresh.is_empty() {
                return;
            }
            let mut map = Map::clone(&known);
            for entry in fresh {
                map.insert(Arc::clone(&entry.rel), entry);
            }
            self.commit(map);
            return;
        };
        for entry in seen {
            map.insert(Arc::clone(&entry.rel), entry);
        }
        self.commit(map);
    }

    fn commit(&self, map: Map) {
        let bytes = map
            .values()
            .map(|e| match &e.content {
                Content::Text(b) => b.len(),
                _ => 0,
            })
            .sum();
        *self.map.lock().unwrap() = Arc::new(map);
        self.bytes.store(bytes, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.snapshot().len()
    }
}

pub(super) type SharedCache = Arc<Cache>;

/// ワーカー間で共有する集計。hits が MAX_HITS を跨いだら Done を 1 回だけ送り、以後は照合しない
struct Progress {
    tx: Sender<Message>,
    cancel: Arc<AtomicBool>,
    scanned: AtomicUsize,
    hits: AtomicUsize,
    truncated: AtomicBool,
}

impl Progress {
    fn new(tx: Sender<Message>, cancel: Arc<AtomicBool>) -> Self {
        Self {
            tx,
            cancel,
            scanned: AtomicUsize::new(0),
            hits: AtomicUsize::new(0),
            truncated: AtomicBool::new(false),
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn truncated(&self) -> bool {
        self.truncated.load(Ordering::Relaxed)
    }

    /// 1 ファイルぶんの結果を流す。上限を跨いだファイルまでは送ってから Done を出す
    /// (打ち切りの表示は Done 側で出す)
    fn report(&self, rel: &Arc<Path>, hits: Vec<Hit>) {
        self.scanned.fetch_add(1, Ordering::Relaxed);
        if hits.is_empty() {
            return;
        }
        let total = self.hits.fetch_add(hits.len(), Ordering::Relaxed) + hits.len();
        let _ = self.tx.send(Message::File(FileHits {
            path: rel.to_path_buf(),
            hits,
        }));
        if total >= MAX_HITS && !self.truncated.swap(true, Ordering::Relaxed) {
            self.send_done();
        }
    }

    fn send_done(&self) {
        let _ = self.tx.send(Message::Done {
            scanned: self.scanned.load(Ordering::Relaxed),
            truncated: self.truncated.load(Ordering::Relaxed),
        });
    }

    /// 走査の終わり。打ち切りで既に Done を送っていればもう送らない
    fn finish(&self) {
        if !self.cancelled() && !self.truncated() {
            self.send_done();
        }
    }
}

/// ワーカーが見たファイルの手元の溜め。ワーカー (`Box<dyn FnMut>`) が捨てられる時に
/// まとめて共有側へ流すので、ファイルごとにロックを取らずに済む
struct Seen<'a> {
    local: Vec<Arc<Entry>>,
    sink: &'a Mutex<Vec<Arc<Entry>>>,
}

impl Drop for Seen<'_> {
    fn drop(&mut self) {
        self.sink.lock().unwrap().append(&mut self.local);
    }
}

/// root を歩いて照合する。cancel を立てると走査中のスレッドが次のファイルで止まる (Done は
/// 送られないことがある。クエリを打ち直した時の古い走査は捨てるだけなので構わない)。
/// 打ち切り (MAX_HITS) 後もキャンセルされない限り歩き続け、完走したら Corpus を送る —
/// 次の走査で walk を省くには一覧が要り、「fn」のような広いクエリから打ち始める使い方では
/// 打ち切りで止めていると一覧がいつまでも揃わないため
pub(super) fn spawn_walk(
    root: PathBuf,
    opts: ScanOptions,
    query: String,
    cancel: Arc<AtomicBool>,
    cache: SharedCache,
) -> Receiver<Message> {
    let (tx, rx) = mpsc::channel();
    // walker.run はスレッドを内部で複数起こしたうえで**呼び出し側をブロックする**ので、
    // それ自体をさらに 1 本のスレッドへ出す (UI スレッドを止めない)
    thread::spawn(move || {
        let progress = Progress::new(tx, cancel);
        let needle = Needle::new(&query);
        let known = cache.snapshot();
        let sink: Mutex<Vec<Arc<Entry>>> = Mutex::new(Vec::new());
        let walker = opts.walker(&root).build_parallel();
        walker.run(|| {
            let root = &root;
            let needle = &needle;
            let progress = &progress;
            let cache = &cache;
            let known = &known;
            let mut seen = Seen {
                local: Vec::new(),
                sink: &sink,
            };
            // 読み込みバッファはワーカーごとに 1 本を使い回す (ファイルごとに確保しない)
            let mut buf = Vec::new();
            Box::new(move |entry| {
                if progress.cancelled() {
                    return WalkState::Quit;
                }
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    return WalkState::Continue;
                }
                let Ok(rel) = entry.path().strip_prefix(root) else {
                    return WalkState::Continue;
                };
                let Some(loaded) = load(entry.path(), rel, Some(known), cache, &mut buf) else {
                    return WalkState::Continue;
                };
                if !progress.truncated()
                    && let Some(hits) = loaded.search(needle, &buf)
                {
                    progress.report(&loaded.rel, hits);
                }
                seen.local.push(loaded);
                WalkState::Continue
            })
        });
        progress.finish();
        let complete = !progress.cancelled();
        let seen = sink.into_inner().unwrap();
        if complete {
            let _ = progress.tx.send(Message::Corpus(seen.clone()));
        }
        cache.replace(seen, complete);
    });
    rx
}

/// 前回完走した走査の一覧をメモリ上で照合する。walk も stat もしない (呼び出し側が「変更が無い」
/// を保証する)。`dirty` が true の項目だけは変更が通知されたものなので**必ず読み直す**
/// (cache の (mtime, size) は見ない — 同じ大きさで mtime の粒度内に書き換えられた場合、stat
/// では変わっていないように見える。通知で確定した変更を stat の推測に戻さない)。
/// 打ち切りで止まっても一覧は変わらないので Corpus は送らず、読み直した項目を Refreshed で返す
pub(super) fn spawn_corpus(
    root: PathBuf,
    corpus: Vec<(Arc<Entry>, bool)>,
    query: String,
    cancel: Arc<AtomicBool>,
    cache: SharedCache,
) -> Receiver<Message> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let progress = Progress::new(tx, cancel);
        let needle = Needle::new(&query);
        let next = AtomicUsize::new(0);
        let workers = thread::available_parallelism().map_or(1, |n| n.get());
        let sink: Mutex<Vec<Arc<Entry>>> = Mutex::new(Vec::new());
        thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    let mut buf = Vec::new();
                    let mut reread = Seen {
                        local: Vec::new(),
                        sink: &sink,
                    };
                    loop {
                        if progress.cancelled() || progress.truncated() {
                            return;
                        }
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some((entry, dirty)) = corpus.get(i) else {
                            return;
                        };
                        let loaded = if *dirty {
                            let abs = root.join(&entry.rel);
                            let fresh = load(&abs, &entry.rel, None, &cache, &mut buf);
                            if let Some(fresh) = &fresh {
                                reread.local.push(Arc::clone(fresh));
                            }
                            fresh
                        } else if matches!(entry.content, Content::Uncached) {
                            read_into(&root.join(&entry.rel), &mut buf).map(|_| Arc::clone(entry))
                        } else {
                            Some(Arc::clone(entry))
                        };
                        if let Some(hits) = loaded.as_ref().and_then(|l| l.search(&needle, &buf)) {
                            progress.report(&loaded.unwrap().rel, hits);
                        }
                    }
                });
            }
        });
        progress.finish();
        // 読み直した項目は cache と呼び出し側の一覧の両方に反映する。キャンセルされた走査の
        // ぶんは一覧へ返さない (途中で来た変更と前後が混ざるため)
        let reread = sink.into_inner().unwrap();
        if !reread.is_empty() {
            if !progress.cancelled() {
                let _ = progress.tx.send(Message::Refreshed(reread.clone()));
            }
            cache.replace(reread, false);
        }
    });
    rx
}

impl Entry {
    /// 照合する。Skip は None、それ以外はヒットが無くても Some (scanned に数える)。
    /// Uncached の本文は呼び出し側の buf に入っている (Entry には無い)
    fn search(&self, needle: &Needle, buf: &[u8]) -> Option<Vec<Hit>> {
        let bytes: &[u8] = match &self.content {
            Content::Text(b) => b,
            Content::Uncached => buf,
            Content::Skip => return None,
        };
        Some(search_text(bytes, needle))
    }
}

/// 1 ファイルを stat し、known に同じ (mtime, size) の項目があればそれを、無ければ読んだものを
/// 返す (cache への反映は呼び出し側が走査の終わりにまとめて行う)。stat も open もできないものは
/// None (一覧にも入れない)。以前は stat せず「上限 + 1 バイトまで読んで溢れたら捨てる」だったが、
/// cache を照合する鍵として stat が要るようになった。実測では stat は read の 1/5 ほどで、
/// cache が当たる限り read を丸ごと省けるので差し引きで速い。
/// known が None なら必ず読む (変更が通知で確定している dirty 項目)
fn load(
    abs: &Path,
    rel: &Path,
    known: Option<&Map>,
    cache: &Cache,
    buf: &mut Vec<u8>,
) -> Option<Arc<Entry>> {
    let meta = std::fs::metadata(abs).ok()?;
    let (mtime, size) = (meta.modified().ok(), meta.len());
    if let Some(entry) = known.and_then(|k| k.get(rel))
        && entry.mtime == mtime
        && entry.size == size
    {
        if matches!(entry.content, Content::Uncached) {
            read_into(abs, buf)?;
        }
        return Some(Arc::clone(entry));
    }
    // 収まるなら専用の Vec に読んでそのまま持つ (worker の buf から写さない)。
    // 大きさは stat の値を当てにした容量の初期値にしか使わず、読んだ長さで判定する
    let want = size.min(MAX_FILE_BYTES) as usize;
    let content = if cache.reserve(want) {
        let mut own = Vec::with_capacity(want + 1);
        match read_into(abs, &mut own) {
            Some(true) => {
                // 予約は stat の値なので読めた長さに合わせる
                if own.len() < want {
                    cache.release(want - own.len());
                }
                Content::Text(own)
            }
            other => {
                cache.release(want);
                match other {
                    Some(false) => Content::Skip,
                    _ => return None,
                }
            }
        }
    } else {
        match read_into(abs, buf) {
            Some(true) => Content::Uncached,
            Some(false) => Content::Skip,
            None => return None,
        }
    };
    Some(Arc::new(Entry {
        rel: Arc::from(rel),
        mtime,
        size,
        content,
    }))
}

/// ファイルを buf に読む。Some(true) = テキスト、Some(false) = 読まない (バイナリ・大きすぎる)、
/// None = 開けない。サイズは stat の値ではなく「上限 + 1 バイトまで読んで溢れたか」で見る
/// (読んでいる途中で伸びたファイルでもメモリの上限を守るため)
fn read_into(abs: &Path, buf: &mut Vec<u8>) -> Option<bool> {
    let file = File::open(abs).ok()?;
    buf.clear();
    file.take(MAX_FILE_BYTES + 1).read_to_end(buf).ok()?;
    if buf.len() as u64 > MAX_FILE_BYTES {
        return Some(false);
    }
    if buf[..buf.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return Some(false);
    }
    Some(true)
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
/// needle の中で**最も稀なバイト**の候補を `Candidates` で拾い、そこから逆算した位置だけ全長を
/// 突き合わせる (ripgrep の memmem と同じ「rare byte」の発想)。先頭バイトで探すと `usize` の
/// `u` のような頻出文字で数十バイトごとに候補が立ち、走査の大半が突き合わせと再開で消える。
/// std の `str::find` は &str を要求するので (= ファイル丸ごとの UTF-8 検証が要る)、バイト列の
/// まま探せるよう自前で持つ。多バイト文字の途中のバイトで候補が立っても全長の一致で弾かれる
fn find_at(hay: &[u8], needle: &[u8], fold: bool) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let off = rarest_offset(needle);
    let rare = needle[off];
    let (a, b) = if fold && rare.is_ascii_alphabetic() {
        (rare, rare.to_ascii_uppercase())
    } else {
        (rare, rare)
    };
    // 稀なバイトが立ちうる範囲: 手前に off バイト、後ろに残りが収まる位置だけ
    let limit = hay.len() - (needle.len() - off - 1);
    if off >= limit {
        return None;
    }
    for at in Candidates::new(&hay[off..limit], a, b) {
        let start = at;
        let window = &hay[start..start + needle.len()];
        let matched = if fold {
            window.eq_ignore_ascii_case(needle)
        } else {
            window == needle
        };
        if matched {
            return Some(start);
        }
    }
    None
}

/// needle の中で一番出現頻度が低いバイトの位置。大小無視で探す時は大文字と小文字を同じ
/// 頻度として見る (両方を候補に拾うので)
fn rarest_offset(needle: &[u8]) -> usize {
    needle
        .iter()
        .enumerate()
        .min_by_key(|(_, b)| byte_frequency(**b))
        .map_or(0, |(i, _)| i)
}

/// バイトのおおよその出現頻度 (大きいほど頻出)。ソースコードと英日混在テキストを想定した
/// 手書きの目安で、厳密である必要は無い — 外れても候補が増えて遅くなるだけで結果は変わらない
fn byte_frequency(b: u8) -> u8 {
    const COMMON_LETTERS: &[u8] = b"etaoinsrhldcumfpgwybvkxjqz";
    match b {
        b' ' | b'\n' => 255,
        b'\t' => 200,
        b'a'..=b'z' | b'A'..=b'Z' => {
            let l = b.to_ascii_lowercase();
            let rank = COMMON_LETTERS.iter().position(|&c| c == l).unwrap_or(25);
            (190 - rank * 6) as u8
        }
        b'_' | b'.' | b',' | b'(' | b')' | b';' | b'=' | b'/' | b'"' | b'\'' | b':' => 120,
        b'0'..=b'9' => 80,
        b'{' | b'}' | b'[' | b']' | b'<' | b'>' | b'-' | b'*' | b'&' | b'+' => 70,
        // UTF-8 の続きバイトと CJK の先頭バイトは日本語のテキストでは頻出
        0x80..=0xBF => 150,
        0xE0..=0xEF => 140,
        0xC0..=0xDF => 60,
        _ => 30,
    }
}

/// a か b のどちらかのバイトの位置を前から順に返す (memchr2 相当のイテレータ)。x86_64 では
/// SSE2 (基準命令セットなので実行時判定は要らない) で 32 バイトずつ比べて一致ビットのマスクを
/// 作り、候補が密な所でもマスクのビットを順に消費するだけで再開する (呼び出しごとに SIMD の
/// ループを立て直さない)。それ以外は 8 バイトずつの SWAR 判定。memchr クレートを足さずに
/// 済ませるための最小限の実装
struct Candidates<'a> {
    hay: &'a [u8],
    a: u8,
    b: u8,
    /// 次に読むチャンクの先頭
    i: usize,
    /// 今のチャンクの未消費の一致ビット (x86_64 のみ使う)
    mask: u32,
    /// mask のビット 0 が指す位置
    base: usize,
}

impl<'a> Candidates<'a> {
    fn new(hay: &'a [u8], a: u8, b: u8) -> Self {
        Self {
            hay,
            a,
            b,
            i: 0,
            mask: 0,
            base: 0,
        }
    }
}

impl Iterator for Candidates<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        loop {
            if self.mask != 0 {
                let bit = self.mask.trailing_zeros() as usize;
                self.mask &= self.mask - 1;
                return Some(self.base + bit);
            }
            #[cfg(target_arch = "x86_64")]
            if self.i + 32 <= self.hay.len() {
                self.mask = mask32_sse2(&self.hay[self.i..], self.a, self.b);
                self.base = self.i;
                self.i += 32;
                continue;
            }
            // 端数 (と x86_64 以外の全体) は SWAR で次の 1 件だけ拾う
            let rest = &self.hay[self.i..];
            let found = find_byte2_swar(rest, self.a, self.b)?;
            let at = self.i + found;
            self.i = at + 1;
            return Some(at);
        }
    }
}

/// hay の先頭 32 バイトのうち a か b に一致する位置のビットマスク (ビット n = 位置 n)
#[cfg(target_arch = "x86_64")]
fn mask32_sse2(hay: &[u8], a: u8, b: u8) -> u32 {
    use std::arch::x86_64::{
        __m128i, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_or_si128, _mm_set1_epi8,
    };
    debug_assert!(hay.len() >= 32);
    // SAFETY: SSE2 は x86_64 の基準命令セットで常に使える。loadu はアラインメントを要求せず、
    // 読む 32 バイトは呼び出し側が hay の中に収まることを保証している
    unsafe {
        let va = _mm_set1_epi8(a as i8);
        let vb = _mm_set1_epi8(b as i8);
        let p = hay.as_ptr() as *const __m128i;
        let v0 = _mm_loadu_si128(p);
        let v1 = _mm_loadu_si128(p.add(1));
        let m0 = _mm_or_si128(_mm_cmpeq_epi8(v0, va), _mm_cmpeq_epi8(v0, vb));
        let m1 = _mm_or_si128(_mm_cmpeq_epi8(v1, va), _mm_cmpeq_epi8(v1, vb));
        (_mm_movemask_epi8(m0) as u32) | ((_mm_movemask_epi8(m1) as u32) << 16)
    }
}

/// a か b のどちらかのバイトが最初に現れる位置 (memchr2 相当)
#[cfg(test)]
fn find_byte2(hay: &[u8], a: u8, b: u8) -> Option<usize> {
    Candidates::new(hay, a, b).next()
}

fn find_byte2_swar(hay: &[u8], a: u8, b: u8) -> Option<usize> {
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
        // 稀なバイトが needle の途中にあっても、返すのは needle の先頭
        assert_eq!(find_at(b"needle needle3", b"needle3", false), Some(7));
        assert_eq!(find_at(b"xx3 needle3", b"needle3", true), Some(4));
        // 稀なバイトが先頭付近に立っても手前が足りない位置は候補にしない
        assert_eq!(find_at(b"3", b"ab3", false), None);
    }

    #[test]
    fn find_byte2_variants_agree_at_every_offset() {
        // SIMD の 32 バイト境界・SWAR の 8 バイト境界・端数の全てで同じ答えになること
        let mut hay = vec![b'.'; 100];
        for at in 0..100 {
            hay[at] = if at % 2 == 0 { b'q' } else { b'Q' };
            for from in 0..=at {
                let want = Some(at - from);
                assert_eq!(
                    find_byte2(&hay[from..], b'q', b'Q'),
                    want,
                    "at={at} from={from}"
                );
                assert_eq!(find_byte2_swar(&hay[from..], b'q', b'Q'), want);
            }
            hay[at] = b'.';
        }
        assert_eq!(find_byte2(&hay, b'q', b'Q'), None);
        // 同じチャンクの中の複数の候補を順に全部返す
        let dense = b"qQ..q...........................q.......Q";
        let got: Vec<usize> = Candidates::new(dense, b'q', b'Q').collect();
        assert_eq!(got, vec![0, 1, 4, 32, 40]);
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
