//! ワークスペース横断検索 (`Ctrl+f`) の状態。Finder と同じ「クエリ + 一覧 + 選択位置」の骨格だが、
//! 候補が同期的に手元にあるのではなく背景の走査から流れ込んでくる点が違う。
//! インデックスは持たない — 「走査 + 読み込み」が本当に足りないと分かった時に、この型の裏側
//! (search.rs) だけを content cache / trigram に差し替えられるよう、オーバーレイ側は
//! 「クエリを渡すと (path, line, col) が流れてくる」以上のことを知らない形に閉じてある。
//!
//! 設計メモは docs/design/workspace-grep.md、恒久的な要約は CLAUDE.md「ワークスペース横断検索」節。

pub mod search;
pub mod view;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use ratatui::widgets::ListState;

use crate::component::tree::ScanOptions;

pub use search::FileHits;
use search::Message;

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
    pub selected: usize,
    pub list_state: ListState,
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
            selected: 0,
            list_state: ListState::default(),
        }
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
            let cancel = Arc::new(AtomicBool::new(false));
            let rx = search::spawn(
                self.root.clone(),
                self.opts,
                self.query.clone(),
                Arc::clone(&cancel),
            );
            self.job = Some(Job { rx, cancel });
            changed = true;
        }
        let Some(job) = &self.job else {
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
                    self.job = None;
                    changed = true;
                    break;
                }
                // スレッドが Done を送らず終わった (cancel 後・パニック) 時も走査中扱いを解く
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

    fn rebuild_rows(&mut self) {
        self.rows = self
            .files
            .iter()
            .enumerate()
            .flat_map(|(file, f)| (0..f.hits.len()).map(move |hit| Row { file, hit }))
            .collect();
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    /// 走査中 (デバウンス待ちを含む)。プレビューの settle とタイトル表示が見る
    pub fn busy(&self) -> bool {
        self.pending_since.is_some() || self.job.is_some()
    }

    /// FS 監視が変更を拾った時に呼ぶ。走査中なら止めて起こし直す (その走査は変更前後が混ざる)。
    /// 完了済みなら印だけ付け、次に開いた時に歩き直す (閉じている間に何度も歩かない)
    pub fn invalidate(&mut self) {
        if !self.searchable() {
            return;
        }
        if self.busy() {
            self.schedule();
        } else {
            self.stale = true;
        }
    }

    /// 表示条件 (隠し項目・無視ファイル) が変わったら走査条件も揃える (FileIndex と同じ)
    pub fn set_options(&mut self, opts: ScanOptions) {
        if self.opts != opts {
            self.opts = opts;
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
            let mut best = Duration::MAX;
            let mut hits = 0;
            for _ in 0..3 {
                let mut state = GrepState::new(root.clone(), opts);
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
            }
            println!("{query:>14}  {best:?}  hits={hits}");
        }
    }
}
