mod node;
mod scan;

pub use node::Row;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ratatui::widgets::ListState;

use node::{Node, NodeKind};

pub struct Tree {
    nodes: Vec<Node>,
    show_hidden: bool,
    // 表示を絞り込むパス集合 (対象ファイル + その祖先ディレクトリ)。None なら全表示。
    // GIT レーンの出入りで App が付け外しする
    filter: Option<HashSet<PathBuf>>,
    // 絞り込み開始時の展開状態。絞り込み中は対象を開いたり畳んだりできるので、
    // 解除時にここへ厳密に戻して VIEW 側の見え方を元通りにする
    saved_expanded: Option<HashSet<PathBuf>>,
    pub visible: Vec<Row>,
    pub selected: usize,
    pub list_state: ListState,
}

impl Tree {
    pub fn new(root: &Path, show_hidden: bool) -> Self {
        let nodes = scan::build_nodes(root, show_hidden);
        let mut tree = Self {
            nodes,
            show_hidden,
            filter: None,
            saved_expanded: None,
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
                scan::expand_all(&mut self.nodes, &paths);
            }
            // 絞り込み中の張り替え (再走査): 新しく対象になったディレクトリだけ開く。
            // 既存のものに触らないので、ユーザーが畳んだ状態が保存のたびに開き直されない
            (Some(previous), Some(paths)) => {
                let added: HashSet<PathBuf> = paths.difference(previous).cloned().collect();
                scan::expand_all(&mut self.nodes, &added);
            }
            // 絞り込み解除: 退避しておいた状態へ厳密に戻す (絞り込み中の開閉は持ち越さない)
            (Some(_), None) => {
                if let Some(saved) = self.saved_expanded.take() {
                    scan::set_expanded(&mut self.nodes, &saved);
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

    /// git 側で削除された未コミットファイルを合成ノードとして反映する。Tree は本来
    /// git を知らない設計だが、削除ファイルだけは WalkBuilder の実ファイル走査で拾えず、
    /// この橋渡しが無いと GIT レーンで選択も stage/unstage もできない。rescan (App::rescan /
    /// App::new / toggle_hidden) で nodes を作り直す都度、呼び出し側が最新の削除集合で呼び直す想定
    pub fn sync_deleted(&mut self, root: &Path, deleted: &HashSet<PathBuf>) {
        // selected_path は index_path 経由で self.nodes を辿るため、挿入・ソートで
        // インデックスが崩れる scan::sync_deleted の "前" に捕まえておく必要がある
        // (rescan/set_filter と同じ順序。後ろで呼ぶと別ノードを指してしまう)
        let selected = self.selected_path();
        if scan::sync_deleted(&mut self.nodes, root, deleted) {
            self.rebuild_visible();
            self.restore_selection(selected);
        }
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
        self.show_hidden
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
        let node = scan::node_mut(&mut self.nodes, &index_path)?;
        match &mut node.kind {
            NodeKind::Dir { expanded, .. } => {
                *expanded = !*expanded;
                self.rebuild_visible();
                None
            }
            NodeKind::File => Some(node.path.clone()),
        }
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

    /// ファイルシステム変更を検知した際に再走査する。展開中ディレクトリと
    /// 選択位置は path で覚えておき、再構築後に付け直す
    /// (走査順が変わりうるため index_path はそのまま使い回せない)。
    pub fn rescan(&mut self, root: &Path) {
        let expanded = scan::collect_expanded(&self.nodes);
        let selected = self.selected_path();

        self.nodes = scan::build_nodes(root, self.show_hidden);
        scan::expand_all(&mut self.nodes, &expanded);
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
    pub fn toggle_hidden(&mut self, root: &Path) -> bool {
        self.show_hidden = !self.show_hidden;
        self.rescan(root);
        self.show_hidden
    }

    /// root 以下の全ファイルを相対パスで列挙する。Finder 起動時に一度だけ呼ばれ、
    /// 折りたたまれているディレクトリの中身も対象にする (新たな走査はせず既存 nodes を使う)
    pub fn collect_file_paths(&self, root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        scan::collect_files(&self.nodes, root, &mut out);
        out
    }

    fn rebuild_visible(&mut self) {
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
