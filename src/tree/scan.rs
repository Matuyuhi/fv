use std::collections::HashSet;
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

/// 集合に含まれるディレクトリを開く (含まれないものは今の状態のまま)。
/// 再走査後の復元と、絞り込み開始時の一括展開の両方で使う
pub(super) fn expand_all(nodes: &mut [Node], expanded: &HashSet<PathBuf>) {
    for node in nodes {
        if let NodeKind::Dir {
            expanded: is_expanded,
            children,
        } = &mut node.kind
        {
            if expanded.contains(&node.path) {
                *is_expanded = true;
            }
            expand_all(children, expanded);
        }
    }
}

/// 展開状態を集合そのものに揃える (集合に無いディレクトリは閉じる)。
/// 絞り込み解除時に「絞り込み前の状態」へ厳密に戻すために使う
pub(super) fn set_expanded(nodes: &mut [Node], expanded: &HashSet<PathBuf>) {
    for node in nodes {
        if let NodeKind::Dir {
            expanded: is_expanded,
            children,
        } = &mut node.kind
        {
            *is_expanded = expanded.contains(&node.path);
            set_expanded(children, expanded);
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

// ルートから一括走査してツリーを組む。サブディレクトリ起点の遅延走査だと
// 親階層の .gitignore が適用されないため、走査は WalkBuilder 1回に寄せる。
// require_git(false) は git repo 外のディレクトリでも .gitignore を効かせるため
// (ignore クレートの既定では git repo 内でのみ適用される)。
pub(super) fn build_nodes(root: &Path, show_hidden: bool) -> Vec<Node> {
    let mut top = Vec::new();
    let walker = WalkBuilder::new(root)
        .require_git(false)
        .hidden(!show_hidden)
        .build();
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
