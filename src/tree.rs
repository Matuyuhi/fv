use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use ratatui::widgets::ListState;

pub struct Tree {
    nodes: Vec<Node>,
    pub visible: Vec<Row>,
    pub selected: usize,
    pub list_state: ListState,
}

struct Node {
    name: String,
    path: PathBuf,
    kind: NodeKind,
}

enum NodeKind {
    File,
    Dir { expanded: bool, children: Vec<Node> },
}

/// 展開状態を反映した表示用の1行。index_path で実ノードを引く。
/// path は git 状態 (HashMap<PathBuf, _>) のキーと突き合わせるための絶対パス。
pub struct Row {
    index_path: Vec<usize>,
    pub name: String,
    pub path: PathBuf,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
}

impl Tree {
    pub fn new(root: &Path) -> Self {
        let nodes = build_nodes(root);
        let mut tree = Self {
            nodes,
            visible: Vec::new(),
            selected: 0,
            list_state: ListState::default(),
        };
        tree.rebuild_visible();
        tree
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
        let node = node_mut(&mut self.nodes, &index_path)?;
        match &mut node.kind {
            NodeKind::Dir { expanded, .. } => {
                *expanded = !*expanded;
                self.rebuild_visible();
                None
            }
            NodeKind::File => Some(node.path.clone()),
        }
    }

    /// ファイルシステム変更を検知した際に再走査する。展開中ディレクトリと
    /// 選択位置は path で覚えておき、再構築後に付け直す
    /// (走査順が変わりうるため index_path はそのまま使い回せない)。
    pub fn rescan(&mut self, root: &Path) {
        let expanded = collect_expanded(&self.nodes);
        let selected_path = self
            .visible
            .get(self.selected)
            .and_then(|row| node(&self.nodes, &row.index_path))
            .map(|n| n.path.clone());

        self.nodes = build_nodes(root);
        apply_expanded(&mut self.nodes, &expanded);
        self.rebuild_visible();

        if let Some(path) = selected_path {
            if let Some(pos) = self
                .visible
                .iter()
                .position(|row| node(&self.nodes, &row.index_path).is_some_and(|n| n.path == path))
            {
                self.selected = pos;
            }
            // 消えていた場合は rebuild_visible が既に selected を範囲内にクランプ済み
        }
    }

    /// root 以下の全ファイルを相対パスで列挙する。Finder 起動時に一度だけ呼ばれ、
    /// 折りたたまれているディレクトリの中身も対象にする (新たな走査はせず既存 nodes を使う)
    pub fn collect_file_paths(&self, root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        collect_files(&self.nodes, root, &mut out);
        out
    }

    fn rebuild_visible(&mut self) {
        let mut rows = Vec::new();
        flatten(&self.nodes, 0, &mut Vec::new(), &mut rows);
        self.visible = rows;
        self.selected = self
            .selected
            .min(self.visible.len().saturating_sub(1));
    }
}

fn flatten(nodes: &[Node], depth: usize, prefix: &mut Vec<usize>, rows: &mut Vec<Row>) {
    for (i, node) in nodes.iter().enumerate() {
        prefix.push(i);
        match &node.kind {
            NodeKind::File => rows.push(Row {
                index_path: prefix.clone(),
                name: node.name.clone(),
                path: node.path.clone(),
                depth,
                is_dir: false,
                expanded: false,
            }),
            NodeKind::Dir { expanded, children } => {
                rows.push(Row {
                    index_path: prefix.clone(),
                    name: node.name.clone(),
                    path: node.path.clone(),
                    depth,
                    is_dir: true,
                    expanded: *expanded,
                });
                if *expanded {
                    flatten(children, depth + 1, prefix, rows);
                }
            }
        }
        prefix.pop();
    }
}

fn collect_files(nodes: &[Node], root: &Path, out: &mut Vec<PathBuf>) {
    for node in nodes {
        match &node.kind {
            NodeKind::File => {
                if let Ok(rel) = node.path.strip_prefix(root) {
                    out.push(rel.to_path_buf());
                }
            }
            NodeKind::Dir { children, .. } => collect_files(children, root, out),
        }
    }
}

fn node<'a>(nodes: &'a [Node], index_path: &[usize]) -> Option<&'a Node> {
    let (&first, rest) = index_path.split_first()?;
    let mut node = nodes.get(first)?;
    for &i in rest {
        match &node.kind {
            NodeKind::Dir { children, .. } => node = children.get(i)?,
            NodeKind::File => return None,
        }
    }
    Some(node)
}

fn collect_expanded(nodes: &[Node]) -> HashSet<PathBuf> {
    let mut set = HashSet::new();
    fn walk(nodes: &[Node], set: &mut HashSet<PathBuf>) {
        for node in nodes {
            if let NodeKind::Dir { expanded, children } = &node.kind {
                if *expanded {
                    set.insert(node.path.clone());
                }
                walk(children, set);
            }
        }
    }
    walk(nodes, &mut set);
    set
}

fn apply_expanded(nodes: &mut [Node], expanded: &HashSet<PathBuf>) {
    for node in nodes {
        if let NodeKind::Dir { expanded: is_expanded, children } = &mut node.kind {
            if expanded.contains(&node.path) {
                *is_expanded = true;
            }
            apply_expanded(children, expanded);
        }
    }
}

fn node_mut<'a>(nodes: &'a mut [Node], index_path: &[usize]) -> Option<&'a mut Node> {
    let (&first, rest) = index_path.split_first()?;
    let mut node = nodes.get_mut(first)?;
    for &i in rest {
        match &mut node.kind {
            NodeKind::Dir { children, .. } => node = children.get_mut(i)?,
            NodeKind::File => return None,
        }
    }
    Some(node)
}

// ルートから一括走査してツリーを組む。サブディレクトリ起点の遅延走査だと
// 親階層の .gitignore が適用されないため、走査は WalkBuilder 1回に寄せる。
// require_git(false) は git repo 外のディレクトリでも .gitignore を効かせるため
// (ignore クレートの既定では git repo 内でのみ適用される)。
fn build_nodes(root: &Path) -> Vec<Node> {
    let mut top = Vec::new();
    let walker = WalkBuilder::new(root).require_git(false).build();
    for entry in walker.flatten() {
        if entry.depth() == 0 {
            continue;
        }
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        insert(&mut top, root, entry.path(), is_dir);
    }
    sort_nodes(&mut top);
    top
}

fn insert(top: &mut Vec<Node>, root: &Path, path: &Path, is_dir: bool) {
    let Ok(rel) = path.strip_prefix(root) else {
        return;
    };
    let mut components: Vec<String> = rel
        .iter()
        .map(|c| c.to_string_lossy().into_owned())
        .collect();
    let Some(name) = components.pop() else {
        return;
    };
    // 走査は深さ優先で親が先に来るため、途中の親ノードは必ず既存
    let mut children = top;
    for comp in &components {
        let Some(pos) = children.iter().position(|n| n.name == *comp) else {
            return;
        };
        match &mut children[pos].kind {
            NodeKind::Dir { children: c, .. } => children = c,
            NodeKind::File => return,
        }
    }
    let kind = if is_dir {
        NodeKind::Dir {
            expanded: false,
            children: Vec::new(),
        }
    } else {
        NodeKind::File
    };
    children.push(Node {
        name,
        path: path.to_path_buf(),
        kind,
    });
}

fn sort_nodes(nodes: &mut [Node]) {
    nodes.sort_by(|a, b| {
        let a_dir = matches!(a.kind, NodeKind::Dir { .. });
        let b_dir = matches!(b.kind, NodeKind::Dir { .. });
        b_dir
            .cmp(&a_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    for node in nodes {
        if let NodeKind::Dir { children, .. } = &mut node.kind {
            sort_nodes(children);
        }
    }
}
