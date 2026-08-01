// Finder (Ctrl+p) の候補一覧。ツリーは展開時まで走査しないため、候補は
// ツリーとは別に root 全体を歩いて作る必要がある。巨大なディレクトリでも
// UI を止めないよう走査は別スレッドに出し、完了までは呼び出し側が
// 「今ツリーに読み込まれている分」で代用する。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;

use ignore::WalkBuilder;

pub struct FileIndex {
    root: PathBuf,
    show_hidden: bool,
    files: Option<Vec<PathBuf>>,
    // 走査中のスレッドからの受け口。None なら走査していない
    pending: Option<Receiver<Vec<PathBuf>>>,
    // files が古くなったことを示す。古い一覧も「無いよりまし」なので捨てず、
    // 次に Finder を開いた時に走査し直す。走査中に立った stale は完了時に消さない
    // (その走査には載っていない変更なので、もう一度歩き直す必要がある)
    stale: bool,
}

impl FileIndex {
    pub fn new(root: PathBuf, show_hidden: bool) -> Self {
        Self {
            root,
            show_hidden,
            files: None,
            pending: None,
            stale: false,
        }
    }

    /// Finder を開くときに呼ぶ。必要なら走査を起こしたうえで、今使える一覧を返す。
    /// None は「まだ一度も走査が終わっていない」= ツリー側で代用してほしいの意
    pub fn request(&mut self) -> Option<&[PathBuf]> {
        self.poll();
        if (self.files.is_none() || self.stale) && self.pending.is_none() {
            self.spawn();
        }
        self.files.as_deref()
    }

    /// 走査完了を取り込む。取り込めた (= 候補が入れ替わった) ときだけ true。
    /// Finder を開いたまま完了した場合に候補を差し替えるため毎 tick 呼ばれる
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.pending else {
            return false;
        };
        match rx.try_recv() {
            Ok(files) => {
                self.files = Some(files);
                self.pending = None;
                // ここで stale を落とさない。走査開始後に来た invalidate は「この結果には
                // 反映されていない変更」を意味するので、次に Finder を開いた時に歩き直させる
                true
            }
            // スレッドが送信前に落ちた場合 (走査失敗) は pending を畳んだうえで、
            // spawn 時に消した stale を戻して再走査できるようにする
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                self.stale = true;
                false
            }
            Err(TryRecvError::Empty) => false,
        }
    }

    /// 走査済みの一覧 (古いかもしれない)。走査を起こさず今あるものだけを見る
    pub fn files(&self) -> Option<&[PathBuf]> {
        self.files.as_deref()
    }

    /// 走査中かどうか (Finder のタイトルに出す)
    pub fn scanning(&self) -> bool {
        self.pending.is_some()
    }

    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    pub fn set_show_hidden(&mut self, show_hidden: bool) {
        self.show_hidden = show_hidden;
        self.stale = true;
    }

    fn spawn(&mut self) {
        let (tx, rx) = channel();
        let root = self.root.clone();
        let show_hidden = self.show_hidden;
        // 走査中に終了された場合は send が失敗するだけ (受け口が drop されている)
        if thread::Builder::new()
            .spawn(move || {
                let _ = tx.send(walk_files(&root, show_hidden));
            })
            .is_ok()
        {
            self.pending = Some(rx);
            // 今の stale はこの走査が引き取る。以後に立つ stale だけが
            // 「この走査に載っていない変更」として残る (走査開始に失敗したら消さない)
            self.stale = false;
        }
    }
}

// ツリーの走査 (tree/scan.rs) と同じ無視設定で root 以下の全ファイルを相対パスで集める
fn walk_files(root: &Path, show_hidden: bool) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let walker = WalkBuilder::new(root)
        .require_git(false)
        .hidden(!show_hidden)
        .build();
    for entry in walker.flatten() {
        if entry.file_type().is_some_and(|t| t.is_dir()) {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(root) {
            files.push(rel.to_path_buf());
        }
    }
    files
}
