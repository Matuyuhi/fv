//! ワークスペース横断検索 (`Ctrl+f`) の状態。Finder と同じ「クエリ + 一覧 + 選択位置」の骨格だが、
//! 候補が同期的に手元にあるのではなく背景の走査から流れ込んでくる点が違う。
//! 転置インデックス (trigram 等) は持たない。代わりに search.rs が「読んだ内容の cache」と
//! 「完走した走査のファイル一覧 (corpus)」を持ち、ここは **どちらの経路で走査するか** だけを
//! 決める (`Snapshot::trusted`): FS 監視が生きていて変更が無ければ corpus をメモリ上で照合し、そうでなければ
//! root を歩き直す (stat で変わっていないファイルは cache から読む)。オーバーレイ側は
//! 「クエリを渡すと (path, line, col) が流れてくる」以上のことを知らない形に閉じてある。
//!
//! 設計メモは docs/design/workspace-grep.md、恒久的な要約は docs/design/grep.md。

pub mod search;
pub mod view;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use ratatui::widgets::ListState;

use crate::component::tree::ScanOptions;

pub use search::FileHits;
use search::{Corpus, Message, SharedCache};

/// キー入力が止まってから走査を起こすまでの間。1 打鍵ごとに repo 全体を歩き直さないため
const DEBOUNCE: Duration = Duration::from_millis(150);
/// これより短いクエリでは走査しない。1 文字は repo のほぼ全行に当たり、上限で打ち切られた
/// 「先頭 5000 件」を見せるだけになるので、歩くコストに見合う結果が出ない
const MIN_QUERY_CHARS: usize = 2;

/// 一覧の 1 行 = 1 ヒット。files 内の位置で指す (ヒット本体は複製しない)
#[derive(Clone, Copy)]
pub struct Row {
    pub file: usize,
    pub hit: usize,
}

struct Job {
    rx: Receiver<Message>,
    cancel: Arc<AtomicBool>,
    /// Done を受け取った (ヒットはもう来ない)。walk 経路では corpus の組み立てがこの後も続く
    done: bool,
    /// 走査を起こした時点で FS 監視が生きていたか。完走した corpus を次回そのまま信用できるか
    /// はこれで決まる (監視が無い間に起きた変更は誰にも分からない)
    watched: bool,
    /// 走査を起こした時点の global gitignore の指紋 (完走した一覧に添える)
    global_ignore: GlobalIgnoreStamp,
    /// 走査中に変更の通知が来た。読み直した項目 (Refreshed) をそのまま一覧へ書き戻すと、
    /// 読んだ後に来た変更の dirty を消してしまうので、その走査の Refreshed は捨てる
    changed_during: bool,
}

/// 前回完走した走査のファイル一覧と、それを歩き直さずに使ってよいかの根拠
struct Snapshot {
    corpus: Corpus,
    /// corpus 内の位置 (変更通知のあったパスを dirty にするため)
    index: HashMap<PathBuf, usize>,
    /// 内容だけが変わったと通知されたファイル。次の照合で stat と cache を通し直す
    dirty: Vec<bool>,
    /// 一覧が今の root の中身と一致していると言えるか。完走時に監視が生きていれば true、
    /// 構造が変わる (作成・削除・リネーム) 通知や監視の途切れで false になる
    trusted: bool,
    /// 走査を起こした時点の global gitignore (core.excludesFile) の指紋。root の外にあって
    /// FS 監視が届かないので、一覧を使い回す前にこれを照合する
    global_ignore: GlobalIgnoreStamp,
}

/// global gitignore の (path, mtime, size)。無ければ None 同士で一致する
type GlobalIgnoreStamp = Option<(PathBuf, Option<std::time::SystemTime>, u64)>;

fn global_ignore_stamp() -> GlobalIgnoreStamp {
    let path = ignore::gitignore::gitconfig_excludes_path()?;
    let meta = std::fs::metadata(&path).ok();
    Some((
        path,
        meta.as_ref().and_then(|m| m.modified().ok()),
        meta.map_or(0, |m| m.len()),
    ))
}

pub struct GrepState {
    root: PathBuf,
    opts: ScanOptions,
    pub query: String,
    /// 今の files/rows を作った走査のクエリ。query はデバウンス待ちの間に先へ進むので、
    /// 表示中のヒットを開く時はこちらを使う (query A の行に query B の `/` を立てない)
    result_query: String,
    /// クエリが変わってからまだ走査を起こしていない間の、最後の打鍵時刻
    pending_since: Option<Instant>,
    job: Option<Job>,
    /// ヒットのあるファイル。パス昇順に保つ (到着順はスレッドの都合で毎回違うため)
    files: Vec<FileHits>,
    /// files を平らにした一覧。poll のたびに 1 回だけ作り直す (ファイルの到着ごとには作らない)
    rows: Vec<Row>,
    /// 直近の走査が読んだファイル数 (完了時に確定)
    scanned: usize,
    truncated: bool,
    /// 走査完了後にファイル変更を検知した。結果が古いかもしれないことをタイトルに出し、
    /// 次に開いた時に同じクエリで歩き直す
    stale: bool,
    /// 読んだ内容の cache (search.rs)。走査を跨いで持ち、走査スレッドと共有する
    cache: SharedCache,
    snapshot: Option<Snapshot>,
    /// 今 FS 監視が生きているか。App が毎 tick 書き込む (走査を起こす瞬間と完走時点の両方で見る)
    watched: bool,
    pub selected: usize,
    pub list_state: ListState,
}

impl Snapshot {
    fn new(corpus: Corpus, trusted: bool, global_ignore: GlobalIgnoreStamp) -> Self {
        let index = corpus
            .iter()
            .enumerate()
            .map(|(i, e)| (e.rel.to_path_buf(), i))
            .collect();
        let dirty = vec![false; corpus.len()];
        Self {
            corpus,
            index,
            dirty,
            trusted,
            global_ignore,
        }
    }

    /// 一覧にあるパスなら dirty にして true。古い本文は手放す (読み直すまで cache と二重に
    /// 持たない。AI が同じファイルを書き換え続ける間、完走するまで古い版が溜まらないように)
    fn mark_dirty(&mut self, rel: &Path) -> bool {
        match self.index.get(rel) {
            Some(&i) => {
                if !self.dirty[i] {
                    self.dirty[i] = true;
                    self.corpus[i] = self.corpus[i].without_content();
                }
                true
            }
            None => false,
        }
    }

    /// 読み直した項目を一覧へ書き戻して dirty を消す
    fn apply_refreshed(&mut self, entries: Vec<Arc<search::Entry>>) {
        for entry in entries {
            if let Some(&i) = self.index.get(entry.rel.as_ref()) {
                self.corpus[i] = entry;
                self.dirty[i] = false;
            }
        }
    }

    #[cfg(test)]
    fn dirty_count(&self) -> usize {
        self.dirty.iter().filter(|&&d| d).count()
    }

    fn entries_with_dirty(&self) -> Vec<(Arc<search::Entry>, bool)> {
        self.corpus
            .iter()
            .zip(&self.dirty)
            .map(|(e, &d)| (Arc::clone(e), d))
            .collect()
    }
}

impl GrepState {
    pub fn new(root: PathBuf, opts: ScanOptions) -> Self {
        Self {
            root,
            opts,
            query: String::new(),
            result_query: String::new(),
            pending_since: None,
            job: None,
            files: Vec::new(),
            rows: Vec::new(),
            scanned: 0,
            truncated: false,
            stale: false,
            cache: SharedCache::default(),
            snapshot: None,
            watched: false,
            selected: 0,
            list_state: ListState::default(),
        }
    }

    /// FS 監視が生きているかを App が毎 tick 伝える。監視が途切れた瞬間に corpus の信用も切る
    /// (途切れている間の変更は届かないので、次は歩き直す)
    pub fn set_watched(&mut self, watched: bool) {
        if self.watched && !watched {
            self.distrust();
        }
        self.watched = watched;
    }

    fn distrust(&mut self) {
        if let Some(snapshot) = &mut self.snapshot {
            snapshot.trusted = false;
        }
    }

    /// 次の走査が root を歩き直さずに済むか
    #[cfg(test)]
    fn trusted(&self) -> bool {
        self.watched && self.snapshot.as_ref().is_some_and(|s| s.trusted)
    }

    /// 走査に値するクエリか (MIN_QUERY_CHARS 以上)。短い間は結果も走査も持たない
    pub fn searchable(&self) -> bool {
        self.query.chars().count() >= MIN_QUERY_CHARS
    }

    /// Ctrl+f で開いた時に呼ぶ。結果が古ければ同じクエリで歩き直す
    pub fn on_open(&mut self) {
        if self.stale && self.searchable() {
            self.stale = false;
            self.schedule();
        }
    }

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.schedule();
    }

    pub fn backspace(&mut self) {
        if self.query.pop().is_some() {
            self.schedule();
        }
    }

    pub fn clear_query(&mut self) {
        if !self.query.is_empty() {
            self.query.clear();
            self.schedule();
        }
    }

    // クエリが変わった。走っている走査は古いので止め、デバウンス後に起こし直す。
    // 結果はまだ捨てない — 打っている間も前の結果が見えている方が落ち着いて読める
    fn schedule(&mut self) {
        self.cancel_job();
        self.stale = false;
        if !self.searchable() {
            self.pending_since = None;
            self.clear_results();
        } else {
            self.pending_since = Some(Instant::now());
        }
    }

    fn cancel_job(&mut self) {
        if let Some(job) = self.job.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    fn cache_len(&self) -> usize {
        self.cache.len()
    }

    #[cfg(test)]
    fn dirty_count(&self) -> usize {
        self.snapshot.as_ref().map_or(0, Snapshot::dirty_count)
    }

    fn clear_results(&mut self) {
        self.files.clear();
        self.rows.clear();
        self.scanned = 0;
        self.truncated = false;
        self.selected = 0;
    }

    /// 毎 tick 呼ぶ。デバウンスの発火と結果の drain を兼ね、一覧が変わったら true
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if self
            .pending_since
            .is_some_and(|since| since.elapsed() >= DEBOUNCE)
        {
            self.pending_since = None;
            self.clear_results();
            self.result_query = self.query.clone();
            self.start_job();
            changed = true;
        }
        let Some(job) = &mut self.job else {
            return changed;
        };
        let mut received = false;
        loop {
            match job.rx.try_recv() {
                Ok(Message::File(file)) => {
                    let at = self.files.partition_point(|f| f.path < file.path);
                    self.files.insert(at, file);
                    received = true;
                }
                Ok(Message::Done { scanned, truncated }) => {
                    self.scanned = scanned;
                    self.truncated = truncated;
                    job.done = true;
                    changed = true;
                }
                Ok(Message::Corpus(corpus)) => {
                    // 完走した時点でも監視が生きていてこそ「以後の変更は必ず届く」と言える
                    let trusted = job.watched && self.watched;
                    self.snapshot = Some(Snapshot::new(corpus, trusted, job.global_ignore.clone()));
                }
                Ok(Message::Refreshed(entries)) => {
                    if !job.changed_during
                        && let Some(snapshot) = &mut self.snapshot
                    {
                        snapshot.apply_refreshed(entries);
                    }
                }
                // スレッドが終わった (完走・cancel 後・パニック)。Done 無しでも走査中扱いを解く
                Err(TryRecvError::Disconnected) => {
                    self.job = None;
                    changed = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        if received {
            self.rebuild_rows();
            changed = true;
        }
        changed
    }

    // 走査を起こす。信用できる一覧があればメモリ上の照合だけ、無ければ root を歩き直す
    fn start_job(&mut self) {
        let cancel = Arc::new(AtomicBool::new(false));
        // root の外の無視設定 (global gitignore) は監視が届かないので、使い回す前に指紋で照合する
        let global_ignore = global_ignore_stamp();
        if let Some(snapshot) = &mut self.snapshot
            && snapshot.global_ignore != global_ignore
        {
            snapshot.trusted = false;
        }
        let rx = match &self.snapshot {
            Some(snapshot) if self.watched && snapshot.trusted => search::spawn_corpus(
                self.root.clone(),
                snapshot.entries_with_dirty(),
                self.query.clone(),
                Arc::clone(&cancel),
                Arc::clone(&self.cache),
            ),
            _ => search::spawn_walk(
                self.root.clone(),
                self.opts,
                self.query.clone(),
                Arc::clone(&cancel),
                Arc::clone(&self.cache),
            ),
        };
        self.job = Some(Job {
            rx,
            cancel,
            done: false,
            watched: self.watched,
            global_ignore,
            changed_during: false,
        });
    }

    fn rebuild_rows(&mut self) {
        self.rows = self
            .files
            .iter()
            .enumerate()
            .flat_map(|(file, f)| (0..f.hits.len()).map(move |hit| Row { file, hit }))
            .collect();
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    /// ヒットがまだ流れてくる (デバウンス待ちを含む)。プレビューの settle とタイトル表示が見る。
    /// 打ち切り後に corpus の組み立てだけが続いている間は false
    pub fn busy(&self) -> bool {
        self.pending_since.is_some() || self.job.as_ref().is_some_and(|j| !j.done)
    }

    /// ファイルの中身だけが変わった (作成・削除・リネームではない) 通知。一覧はそのままで、
    /// そのファイルだけ次の照合で読み直す。一覧に無いパスなら構造が変わったと見なす
    pub fn touch(&mut self, path: &Path) {
        let rel = path.strip_prefix(&self.root).unwrap_or(path);
        let known = self
            .snapshot
            .as_mut()
            .is_some_and(|snapshot| snapshot.mark_dirty(rel));
        self.on_change(known);
    }

    /// FS 監視が変更を拾った時に呼ぶ (どのファイルが変わったか分からない、または構造が変わった)。
    /// 走査中なら止めて起こし直す (その走査は変更前後が混ざる)。完了済みなら印だけ付け、
    /// 次に開いた時に歩き直す (閉じている間に何度も歩かない)
    pub fn invalidate(&mut self) {
        self.on_change(false);
    }

    // 変更が来た。`list_intact` は一覧 (どのパスがあるか) が変わっていないと分かっている時 true
    fn on_change(&mut self, list_intact: bool) {
        // 走っている走査は変更の前後どちらを読んだか分からないので、完走しても一覧を信用しない
        // (打ち切り後に一覧の組み立てだけ続いている間も同じ)
        if let Some(job) = &mut self.job {
            job.watched = false;
            job.changed_during = true;
        }
        if !list_intact {
            self.distrust();
        }
        if !self.searchable() {
            return;
        }
        if self.busy() {
            self.schedule();
        } else {
            self.stale = true;
        }
    }

    /// 表示条件 (隠し項目・無視ファイル) が変わったら走査条件も揃える (FileIndex と同じ)。
    /// 一覧は条件ごと違うので捨てる (cache は stat で照合するので残してよい)
    pub fn set_options(&mut self, opts: ScanOptions) {
        if self.opts != opts {
            self.opts = opts;
            self.snapshot = None;
            self.cancel_job();
            self.invalidate();
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn files(&self) -> &[FileHits] {
        &self.files
    }

    pub fn hit_count(&self) -> usize {
        self.rows.len()
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn scanned(&self) -> usize {
        self.scanned
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }

    pub fn stale(&self) -> bool {
        self.stale
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
    }

    /// 選択中のヒット (root からの相対パス, 0-origin 行, plain の char 桁)
    pub fn selected_hit(&self) -> Option<(&std::path::Path, usize, usize)> {
        let row = self.rows.get(self.selected)?;
        let file = &self.files[row.file];
        let hit = &file.hits[row.hit];
        Some((&file.path, hit.line, hit.col))
    }

    /// 表示中のヒットを作ったクエリ (入力中の query とは別)
    pub fn result_query(&self) -> &str {
        &self.result_query
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("fv-grep-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.rs"), "fn alpha() {}\nlet needle = 1;\n").unwrap();
        fs::write(root.join("src/b.rs"), "needle\nNEEDLE\n").unwrap();
        fs::write(
            root.join("bin.dat"),
            [0u8, b'n', b'e', b'e', b'd', b'l', b'e'],
        )
        .unwrap();
        fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(root.join("ignored.txt"), "needle\n").unwrap();
        root
    }

    fn wait(state: &mut GrepState) {
        for _ in 0..2000 {
            state.poll();
            if !state.busy() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("search did not finish");
    }

    #[test]
    fn streams_hits_sorted_by_path_and_respects_ignore_rules() {
        let root = fixture("basic");
        let opts = ScanOptions {
            show_hidden: false,
            show_ignored: false,
        };
        let mut state = GrepState::new(root.clone(), opts);
        for c in "needle".chars() {
            state.push_char(c);
        }
        assert!(state.busy());
        wait(&mut state);
        let paths: Vec<String> = state
            .files()
            .iter()
            .map(|f| f.path.to_string_lossy().into_owned())
            .collect();
        // バイナリ (NUL) と .gitignore 対象は出ない。smart-case なので NEEDLE も当たる
        assert_eq!(paths, vec!["src/a.rs", "src/b.rs"]);
        assert_eq!(state.hit_count(), 3);
        assert_eq!(state.scanned(), 2);
        assert!(!state.truncated());
        state.move_selection(2);
        let (path, line, col) = state.selected_hit().unwrap();
        assert_eq!(path.to_string_lossy(), "src/b.rs");
        assert_eq!((line, col), (1, 0));
        // 打ち直してデバウンス待ちの間、表示中の行はまだ前のクエリのもの
        state.push_char('x');
        assert_eq!(state.result_query(), "needle");
        assert_eq!(state.hit_count(), 3);
        let _ = fs::remove_dir_all(root);
    }

    fn search(state: &mut GrepState, query: &str) {
        state.clear_query();
        for c in query.chars() {
            state.push_char(c);
        }
        wait(state);
        // 打ち切り後の一覧の組み立ても待つ (Done の後に Corpus が来る)
        for _ in 0..2000 {
            state.poll();
            if state.job.is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("walk did not finish");
    }

    fn lines(state: &GrepState) -> Vec<(String, usize)> {
        state
            .rows()
            .iter()
            .map(|r| {
                let f = &state.files()[r.file];
                (f.path.to_string_lossy().into_owned(), f.hits[r.hit].line)
            })
            .collect()
    }

    // 2 回目以降の走査は前回の一覧をメモリ上で照合する (監視が生きている間だけ)。
    // 内容だけの変更はそのファイルを読み直し、構造の変更は歩き直す — どちらも結果は
    // 毎回歩き直した時と同じでなければならない
    #[test]
    fn reuses_the_corpus_while_watched_and_refreshes_touched_files() {
        let root = fixture("corpus");
        let opts = ScanOptions {
            show_hidden: false,
            show_ignored: false,
        };
        let mut state = GrepState::new(root.clone(), opts);
        // 監視が無い間は完走しても一覧を信用しない
        search(&mut state, "needle");
        assert!(!state.trusted());
        // バイナリも「読まない」印として残す (次回 stat だけで飛ばせる)
        assert_eq!(state.cache_len(), 3);
        state.set_watched(true);
        search(&mut state, "needle");
        assert!(state.trusted());
        let before = lines(&state);
        assert_eq!(before.len(), 3);

        // 中身だけの変更: 一覧はそのまま (trusted のまま)、そのファイルだけ読み直す
        fs::write(
            root.join("src/a.rs"),
            "needle
needle
needle
",
        )
        .unwrap();
        state.touch(&root.join("src/a.rs"));
        assert!(state.trusted());
        assert!(state.stale());
        assert_eq!(state.dirty_count(), 1);
        state.on_open();
        search(&mut state, "needle");
        assert_eq!(lines(&state).len(), 5);
        assert!(state.trusted());
        // 読み直した項目は一覧へ書き戻され、次から dirty ではない (毎回読み直さない)
        assert_eq!(state.dirty_count(), 0);

        // 同じ大きさで書き換えても (stat では見抜けない) 通知があれば読み直す
        fs::write(root.join("src/a.rs"), "needle\nneedle\nnothin\n").unwrap();
        state.touch(&root.join("src/a.rs"));
        state.on_open();
        search(&mut state, "needle");
        assert_eq!(lines(&state).len(), 4);
        assert_eq!(state.dirty_count(), 0);

        // 新しいファイル: 構造の変更なので歩き直し、新しいファイルも当たる
        fs::write(
            root.join("src/c.rs"),
            "needle
",
        )
        .unwrap();
        state.invalidate();
        assert!(!state.trusted());
        state.on_open();
        search(&mut state, "needle");
        assert_eq!(lines(&state).len(), 5);
        assert!(state.trusted());

        // 一覧に無いパスの内容変更は (取りこぼしの可能性があるので) 歩き直す側に倒す
        state.touch(&root.join("src/unknown.rs"));
        assert!(!state.trusted());

        // 監視が途切れたら信用しない
        search(&mut state, "needle");
        assert!(state.trusted());
        state.set_watched(false);
        assert!(!state.trusted());
        let _ = fs::remove_dir_all(root);
    }

    // 内容が変わったのに mtime も size も同じ、は cache では見抜けない。逆に size か mtime が
    // 変わっていれば歩き直しの経路 (監視なし) でも必ず読み直す
    #[test]
    fn walk_rereads_files_whose_stat_changed() {
        let root = fixture("stat");
        let opts = ScanOptions {
            show_hidden: false,
            show_ignored: false,
        };
        let mut state = GrepState::new(root.clone(), opts);
        search(&mut state, "needle");
        assert_eq!(lines(&state).len(), 3);
        fs::write(
            root.join("src/b.rs"),
            "nothing here
",
        )
        .unwrap();
        search(&mut state, "needle");
        assert_eq!(
            lines(&state),
            vec![("src/a.rs".to_string(), 1)],
            "b.rs は size が変わったので読み直される"
        );
        // 消えたファイルは完走時に cache からも落ちる
        fs::remove_file(root.join("src/b.rs")).unwrap();
        search(&mut state, "needle");
        assert_eq!(state.cache_len(), 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalidate_marks_finished_results_stale_and_reopen_reruns() {
        let root = fixture("stale");
        let opts = ScanOptions {
            show_hidden: false,
            show_ignored: false,
        };
        let mut state = GrepState::new(root.clone(), opts);
        state.push_char('n');
        // 1 文字では走査しない (結果も持たない)
        assert!(!state.busy());
        assert!(!state.searchable());
        state.push_char('e');
        wait(&mut state);
        assert!(!state.stale());
        state.invalidate();
        assert!(state.stale());
        state.on_open();
        assert!(state.busy());
        wait(&mut state);
        assert!(!state.stale());
        // 空クエリは走査を起こさず結果も捨てる
        state.clear_query();
        assert!(!state.busy());
        assert_eq!(state.hit_count(), 0);
        let _ = fs::remove_dir_all(root);
    }
}

// 走査コストの物差し。合成ツリー (2 万ファイル・タブ入り混在) を歩いて完了までの時間を出す。
// 通常の cargo test では走らせない (数秒かかる):
//   cargo test --release -- --ignored grep_bench --nocapture
#[cfg(test)]
mod bench {
    use super::*;
    use std::fs;
    use std::io::Write;

    #[test]
    #[ignore]
    fn grep_bench() {
        let root = std::env::temp_dir().join("fv-grep-bench");
        if !root.join(".done").exists() {
            let _ = fs::remove_dir_all(&root);
            for d in 0..200 {
                let dir = root.join(format!("pkg{d:03}/src"));
                fs::create_dir_all(&dir).unwrap();
                for f in 0..100 {
                    let mut out = fs::File::create(dir.join(format!("mod{f:03}.rs"))).unwrap();
                    let indent = if f % 2 == 0 { "\t" } else { "    " };
                    for i in 0..60 {
                        writeln!(
                            out,
                            "{indent}fn item_{d}_{f}_{i}(x: usize) -> usize {{ x + {i} }}"
                        )
                        .unwrap();
                        writeln!(out, "{indent}// Some Comment about Needle{}", i % 7).unwrap();
                    }
                }
            }
            fs::write(root.join(".done"), "").unwrap();
        }
        let opts = ScanOptions {
            show_hidden: false,
            show_ignored: false,
        };
        // 上の 3 つは上限で打ち切られる経路、"item_150_50_" は全ファイルを歩いて 1 ファイルだけ
        // 当たる経路、"zzzz" は全ファイルを歩いて何も当たらない (= 純粋な走査 + 読み込み) 経路、"Zzzz" は同じく大小区別 (小文字化の写し無し)
        for query in [
            "needle3",
            "Needle3",
            "fn item",
            "item_150_50_",
            "zzzz",
            "Zzzz",
        ] {
            // cold: cache 無し (初回) / walk: 歩き直すが読まない (監視なし) / corpus: 一覧をメモリ上で照合
            let mut state = GrepState::new(root.clone(), opts);
            let mut row = String::new();
            for (label, watched) in [("cold", false), ("walk", false), ("corpus", true)] {
                let mut best = Duration::MAX;
                let mut hits = 0;
                for _ in 0..3 {
                    state.set_watched(watched);
                    state.clear_query();
                    for c in query.chars() {
                        state.push_char(c);
                    }
                    // デバウンス待ちは測らない
                    std::thread::sleep(DEBOUNCE);
                    let start = Instant::now();
                    loop {
                        state.poll();
                        if !state.busy() {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    best = best.min(start.elapsed());
                    hits = state.hit_count();
                    // 打ち切り後の一覧の組み立てが終わるまで待ってから次を測る
                    while state.job.is_some() {
                        state.poll();
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    if label == "cold" {
                        break;
                    }
                }
                row.push_str(&format!("  {label} {best:>10.1?}"));
                if label == "corpus" {
                    row.push_str(&format!("  hits={hits} trusted={}", state.trusted()));
                }
            }
            println!("{query:>14}{row}");
        }
    }
}
