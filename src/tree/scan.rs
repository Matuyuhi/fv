use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use super::node::{Node, NodeKind, Row};

/// filter が Some のときは集合に含まれるノードだけを出す。展開状態は絞り込み中も
/// expanded フラグをそのまま尊重する (h/l の折りたたみを効かせるため)。
/// 「絞り込み開始時に対象を全部開く」のは Tree::set_filter の役割
pub(super) fn flatten(
    nodes: &[Node],
    depth: usize,
    prefix: &mut Vec<usize>,
    rows: &mut Vec<Row>,
    filter: Option<&HashSet<PathBuf>>,
) {
    for (i, node) in nodes.iter().enumerate() {
        if filter.is_some_and(|f| !f.contains(&node.path)) {
            continue;
        }
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
            NodeKind::Dir {
                expanded, children, ..
            } => {
                rows.push(Row {
                    index_path: prefix.clone(),
                    name: node.name.clone(),
                    path: node.path.clone(),
                    depth,
                    is_dir: true,
                    expanded: *expanded,
                });
                if *expanded {
                    flatten(children, depth + 1, prefix, rows, filter);
                }
            }
        }
        prefix.pop();
    }
}

pub(super) fn collect_files(nodes: &[Node], root: &Path, out: &mut Vec<PathBuf>) {
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

pub(super) fn node<'a>(nodes: &'a [Node], index_path: &[usize]) -> Option<&'a Node> {
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

pub(super) fn collect_expanded(nodes: &[Node]) -> HashSet<PathBuf> {
    let mut set = HashSet::new();
    fn walk(nodes: &[Node], set: &mut HashSet<PathBuf>) {
        for node in nodes {
            if let NodeKind::Dir {
                expanded, children, ..
            } = &node.kind
            {
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

/// 集合に含まれるディレクトリを開く (含まれないものは今の状態のまま)。
/// 絞り込み開始時の一括展開で使う。集合は祖先も含んでいる前提で、
/// 未走査のディレクトリはここで読み込んでから降りる (GIT の変更ファイルが
/// 未展開の階層にあっても絞り込みツリーに現れるようにするため)
pub(super) fn expand_all(nodes: &mut [Node], expanded: &HashSet<PathBuf>, show_hidden: bool) {
    for node in nodes {
        if !matches!(node.kind, NodeKind::Dir { .. }) {
            continue;
        }
        if expanded.contains(&node.path) {
            load(node, show_hidden);
            if let NodeKind::Dir {
                expanded: is_expanded,
                ..
            } = &mut node.kind
            {
                *is_expanded = true;
            }
        }
        if let NodeKind::Dir { children, .. } = &mut node.kind {
            expand_all(children, expanded, show_hidden);
        }
    }
}

/// 展開状態を集合そのものに揃える (集合に無いディレクトリは閉じる)。
/// 絞り込み解除時に「絞り込み前の状態」へ厳密に戻すために使う
pub(super) fn set_expanded(nodes: &mut [Node], expanded: &HashSet<PathBuf>, show_hidden: bool) {
    for node in nodes {
        if !matches!(node.kind, NodeKind::Dir { .. }) {
            continue;
        }
        let open = expanded.contains(&node.path);
        if open {
            load(node, show_hidden);
        }
        if let NodeKind::Dir {
            expanded: is_expanded,
            children,
            ..
        } = &mut node.kind
        {
            *is_expanded = open;
            set_expanded(children, expanded, show_hidden);
        }
    }
}

pub(super) fn node_mut<'a>(nodes: &'a mut [Node], index_path: &[usize]) -> Option<&'a mut Node> {
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

// ディレクトリ 1 階層だけを読む。1 階層でも WalkBuilder を通すのは、既定の
// parents(true) が祖先の .gitignore まで遡って読むため、サブディレクトリ起点の
// 走査でも root 側の無視設定がそのまま効くから (これが効かないなら一括走査に
// 戻す必要がある)。require_git(false) は git repo 外のディレクトリでも
// .gitignore を効かせるため (ignore クレートの既定では git repo 内でのみ適用される)。
pub(super) fn read_dir(dir: &Path, show_hidden: bool) -> Vec<Node> {
    let mut nodes = Vec::new();
    let walker = WalkBuilder::new(dir)
        .require_git(false)
        .hidden(!show_hidden)
        .max_depth(Some(1))
        .build();
    for entry in walker.flatten() {
        // depth 0 は走査起点のディレクトリ自身
        if entry.depth() == 0 {
            continue;
        }
        let kind = if entry.file_type().is_some_and(|t| t.is_dir()) {
            NodeKind::Dir {
                expanded: false,
                loaded: false,
                children: Vec::new(),
            }
        } else {
            NodeKind::File
        };
        nodes.push(Node {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: entry.path().to_path_buf(),
            kind,
        });
    }
    sort_nodes(&mut nodes);
    nodes
}

/// 未走査のディレクトリなら子を読み込む。展開の直前に必ず通す
/// (「開こうとした時に読む」= 起動時にツリー全体を歩かないための入口)
pub(super) fn load(node: &mut Node, show_hidden: bool) {
    let NodeKind::Dir {
        loaded, children, ..
    } = &mut node.kind
    else {
        return;
    };
    if *loaded {
        return;
    }
    *loaded = true;
    *children = read_dir(&node.path, show_hidden);
}

/// 読み込み済みの階層だけを読み直して差分を取り込む。未走査のディレクトリには
/// 触らないので、再走査のコストは「今開いている範囲」に比例する。
/// 展開状態・読み込み済みの子は名前で引き継ぐ (index_path は作り直しになる)
pub(super) fn refresh(nodes: &mut Vec<Node>, dir: &Path, show_hidden: bool) {
    let mut previous: HashMap<String, NodeKind> =
        nodes.drain(..).map(|node| (node.name, node.kind)).collect();
    let mut fresh = read_dir(dir, show_hidden);
    for node in &mut fresh {
        // 種別が変わった (ファイル ⇄ ディレクトリ) 場合は引き継がず新しい方を使う
        let Some(NodeKind::Dir {
            expanded,
            loaded,
            mut children,
        }) = previous.remove(&node.name)
        else {
            continue;
        };
        if !matches!(node.kind, NodeKind::Dir { .. }) {
            continue;
        }
        if loaded {
            refresh(&mut children, &node.path, show_hidden);
        }
        node.kind = NodeKind::Dir {
            expanded,
            loaded,
            children,
        };
    }
    *nodes = fresh;
}

fn sort_nodes(nodes: &mut [Node]) {
    nodes.sort_by(|a, b| {
        let a_dir = matches!(a.kind, NodeKind::Dir { .. });
        let b_dir = matches!(b.kind, NodeKind::Dir { .. });
        b_dir
            .cmp(&a_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}
