mod node;
mod scan;
pub mod view;

pub use node::Row;
pub(crate) use scan::ScanOptions;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ratatui::widgets::ListState;

use node::{Node, NodeKind};

pub struct Tree {
    // 展開時の遅延走査で走査起点を組み立てるため Tree 自身が root を持つ
    // (呼び出し側が毎回渡す形だと「読み込みに root が要る」操作が増えるたびに引数が伝播する)
    root: PathBuf,
    nodes: Vec<Node>,
    opts: ScanOptions,
    // 表示を絞り込むパス集合 (対象ファイル + その祖先ディレクトリ)。None なら全表示。
    // GIT レーンの出入りで App が付け外しする
    filter: Option<HashSet<PathBuf>>,
    // 絞り込み開始時の展開状態。絞り込み中は対象を開いたり畳んだりできるので、
    // 解除時にここへ厳密に戻して VIEW 側の見え方を元通りにする
    saved_expanded: Option<HashSet<PathBuf>>,
    // git 側で削除された未コミットファイル (sync_deleted で App から受け取った最新の集合)。
    // 1 回挿し込んで終わりにできないのは、遅延ロード (scan::load) が実走査の結果で children を
    // 丸ごと置き換えるため — 合成ノードは実ファイルとして走査に出てこないので、その階層を
    // 展開した瞬間に消えてしまう。集合を持ち続けて rebuild_visible のたびに入れ直す
    deleted: HashSet<PathBuf>,
    pub visible: Vec<Row>,
    pub selected: usize,
    pub list_state: ListState,
}

impl Tree {
    /// 起動時に読むのは root 直下の 1 階層だけ。以下の階層はディレクトリを
    /// 開いた時に読む (巨大なディレクトリでも起動が待たされないようにするため)
    pub fn new(root: &Path, opts: ScanOptions) -> Self {
        // root 自身は走査起点なので無視対象にはなりえない (親を持たない)
        let nodes = scan::read_dir(root, opts, false);
        let mut tree = Self {
            root: root.to_path_buf(),
            nodes,
            opts,
            filter: None,
            saved_expanded: None,
            deleted: HashSet::new(),
            visible: Vec::new(),
            selected: 0,
            list_state: ListState::default(),
        };
        tree.rebuild_visible();
        tree
    }

    /// 表示を絞り込むパス集合を差し替える (None で解除)。
    /// 選択は絞り込み前後で同じファイルに留まるよう path で引き継ぐ
    pub fn set_filter(&mut self, filter: Option<HashSet<PathBuf>>) {
        match (&self.filter, &filter) {
            // 絞り込み開始: 元の展開状態を退避してから対象を全部開く
            (None, Some(paths)) => {
                self.saved_expanded = Some(scan::collect_expanded(&self.nodes));
                let paths = paths.clone();
                scan::expand_all(&mut self.nodes, &paths, self.opts);
            }
            // 絞り込み中の張り替え (再走査): 新しく対象になったディレクトリだけ開く。
            // 既存のものに触らないので、ユーザーが畳んだ状態が保存のたびに開き直されない
            (Some(previous), Some(paths)) => {
                let added: HashSet<PathBuf> = paths.difference(previous).cloned().collect();
                scan::expand_all(&mut self.nodes, &added, self.opts);
            }
            // 絞り込み解除: 退避しておいた状態へ厳密に戻す (絞り込み中の開閉は持ち越さない)
            (Some(_), None) => {
                if let Some(saved) = self.saved_expanded.take() {
                    scan::set_expanded(&mut self.nodes, &saved, self.opts);
                }
            }
            (None, None) => {}
        }
        self.filter = filter;
        let selected = self.selected_path();
        self.rebuild_visible();
        self.restore_selection(selected);
    }

    pub fn is_filtered(&self) -> bool {
        self.filter.is_some()
    }

    /// git 側で削除された未コミットファイルの集合を差し替える。Tree は本来 git を知らない
    /// 設計だが、削除ファイルだけは WalkBuilder の実ファイル走査で拾えず、この橋渡しが無いと
    /// GIT レーンで選択も stage/unstage もできない。rescan (App::rescan / App::new /
    /// toggle_hidden) で nodes を作り直す都度、呼び出し側が最新の削除集合で呼び直す想定。
    /// 実際の挿し込みは rebuild_visible が毎回行う (deleted フィールドの説明を参照)
    pub fn sync_deleted(&mut self, deleted: &HashSet<PathBuf>) {
        if self.deleted == *deleted {
            return;
        }
        self.deleted = deleted.clone();
        // selected_path は index_path 経由で self.nodes を辿るため、挿入・ソートで
        // インデックスが崩れる前に捕まえておく必要がある (rescan/set_filter と同じ順序)
        let selected = self.selected_path();
        self.rebuild_visible();
        self.restore_selection(selected);
    }

    /// 現在の visible 行数。フィルタ中は「変更ファイル + ディレクトリ」の件数になる
    pub fn visible_files(&self) -> usize {
        self.visible.iter().filter(|row| !row.is_dir).count()
    }

    /// 選択行がファイルならそのパス。ディレクトリ行なら先頭のファイルにフォールバックする
    /// (GIT レーンに入った直後、何かしらの diff を出すため)
    pub fn selected_or_first_file(&self) -> Option<PathBuf> {
        if let Some(row) = self.visible.get(self.selected)
            && !row.is_dir
        {
            return Some(row.path.clone());
        }
        self.visible
            .iter()
            .find(|row| !row.is_dir)
            .map(|row| row.path.clone())
    }

    pub fn show_hidden(&self) -> bool {
        self.opts.show_hidden
    }

    pub fn show_ignored(&self) -> bool {
        self.opts.show_ignored
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let last = self.visible.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
    }

    /// 選択中がディレクトリなら展開/折りたたみして None、
    /// ファイルならそのパスを返す。
    pub fn toggle_or_open(&mut self) -> Option<PathBuf> {
        let index_path = self.visible.get(self.selected)?.index_path.clone();
        let opts = self.opts;
        let node = scan::node_mut(&mut self.nodes, &index_path)?;
        let opened = match &mut node.kind {
            NodeKind::Dir { expanded, .. } => {
                *expanded = !*expanded;
                *expanded
            }
            NodeKind::File => return Some(node.path.clone()),
        };
        // 開く時だけ走査する。畳む時に読む必要はないし、閉じたまま残った子は
        // 次に開く時のキャッシュとしてそのまま使える。子がディレクトリ 1 つ
        // だけの階層はそのまま連鎖して開く (`com/example/app` を 3 回開かせない)
        if opened {
            scan::expand_single_child_chain(node, opts);
            scan::expand_ancestors(&mut self.nodes, &index_path);
        }
        self.rebuild_visible();
        None
    }

    /// 選択がディレクトリで未展開なら展開のみ行い選択は動かさない (l を連打して
    /// 一段ずつ潜れるようにするため)。展開済みなら最初の子へ移動。ファイルなら
    /// toggle_or_open と同じくパスを返して呼び出し側で開かせる。
    pub fn expand_or_enter(&mut self) -> Option<PathBuf> {
        let row = self.visible.get(self.selected)?;
        let is_dir = row.is_dir;
        let expanded = row.expanded;
        let depth = row.depth;
        if !is_dir {
            return self.toggle_or_open();
        }
        if expanded {
            if let Some(next) = self.visible.get(self.selected + 1)
                && next.depth == depth + 1
            {
                self.selected += 1;
            }
            None
        } else {
            self.toggle_or_open()
        }
    }

    /// 選択がディレクトリで展開済みなら折りたたむ。それ以外 (ファイル・未展開
    /// ディレクトリ) なら親ディレクトリの行へ選択を移動する。
    pub fn collapse_or_parent(&mut self) {
        let Some(row) = self.visible.get(self.selected) else {
            return;
        };
        if row.is_dir && row.expanded {
            self.toggle_or_open();
        } else {
            self.select_parent();
        }
    }

    /// 親ディレクトリの行へ選択を移動したうえで折りたたむ。ranger 等の H 相当。
    pub fn select_parent_and_collapse(&mut self) {
        if !self.select_parent() {
            return;
        }
        if let Some(row) = self.visible.get(self.selected)
            && row.is_dir
            && row.expanded
        {
            self.toggle_or_open();
        }
    }

    /// 選択を先頭行へ移動する (gg)。
    pub fn select_top(&mut self) {
        self.selected = 0;
    }

    /// 選択を末尾行へ移動する (G)。
    pub fn select_bottom(&mut self) {
        self.selected = self.visible.len().saturating_sub(1);
    }

    /// visible 上で現在行より上方向にある、depth が1小さい直近の行へ選択を移す。
    /// 見つかれば true (トップレベル行では親がないので false)。
    fn select_parent(&mut self) -> bool {
        let Some(depth) = self.visible.get(self.selected).map(|r| r.depth) else {
            return false;
        };
        if depth == 0 {
            return false;
        }
        let Some(idx) = self.visible[..self.selected]
            .iter()
            .rposition(|r| r.depth == depth - 1)
        else {
            return false;
        };
        self.selected = idx;
        true
    }

    /// ファイルシステム変更を検知した際に再走査する。読み込み済みの階層だけを
    /// 読み直すので、走査量は起動時と同じく「開いている範囲」に比例する。
    /// 選択位置は path で覚えておき、再構築後に付け直す
    /// (走査順が変わりうるため index_path はそのまま使い回せない)。
    pub fn rescan(&mut self) {
        let selected = self.selected_path();
        scan::refresh(&mut self.nodes, &self.root, self.opts, false);
        self.rebuild_visible();
        self.restore_selection(selected);
    }

    /// 選択中の行が指すノードの絶対パス。rebuild を挟んで選択を引き継ぐために使う
    /// (index_path は走査・絞り込みのたびに無効になる)
    fn selected_path(&self) -> Option<PathBuf> {
        self.visible
            .get(self.selected)
            .and_then(|row| scan::node(&self.nodes, &row.index_path))
            .map(|n| n.path.clone())
    }

    // 消えていた場合は rebuild_visible が既に selected を範囲内にクランプ済み
    fn restore_selection(&mut self, path: Option<PathBuf>) {
        if let Some(path) = path
            && let Some(pos) = self.visible.iter().position(|row| row.path == path)
        {
            self.selected = pos;
        }
    }

    /// 隠し項目の表示設定を切り替え、展開状態と選択位置を保ったまま再走査する。
    pub fn toggle_hidden(&mut self) -> ScanOptions {
        self.opts.show_hidden = !self.opts.show_hidden;
        self.rescan();
        self.opts
    }

    /// .gitignore 等で無視されるファイルの表示を切り替える。読み込み済みの階層を
    /// 読み直すだけなので、コストは toggle_hidden と同じく「今開いている範囲」に比例する
    pub fn toggle_ignored(&mut self) -> ScanOptions {
        self.opts.show_ignored = !self.opts.show_ignored;
        self.rescan();
        self.opts
    }

    /// 読み込み済みのファイルを相対パスで列挙する。Finder の候補が
    /// (root 全体を歩く FileIndex より先に) 必要になった時の暫定値で、
    /// 新たな走査はせず既存 nodes をそのまま使う
    pub fn collect_file_paths(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        scan::collect_files(&self.nodes, &self.root, &mut out);
        out
    }

    fn rebuild_visible(&mut self) {
        // 削除ファイルの合成ノードは children を作り直す全ての経路 (遅延ロード・展開・再走査) で
        // 失われるため、行を組み直す直前に必ず入れ直す。deleted が空なら何もしない
        scan::sync_deleted(&mut self.nodes, &self.root, &self.deleted);
        let mut rows = Vec::new();
        scan::flatten(
            &self.nodes,
            0,
            &mut Vec::new(),
            &mut rows,
            self.filter.as_ref(),
        );
        self.visible = rows;
        self.selected = self.selected.min(self.visible.len().saturating_sub(1));
    }
}
