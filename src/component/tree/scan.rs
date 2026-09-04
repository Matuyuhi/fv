use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use super::node::{Node, NodeKind, Row};

/// 走査でどこまで見せるかの設定。ツリー (このファイル)・Finder の候補
/// (component/finder/index.rs)・FS 監視 (watch.rs) の 3 者が同じ条件で揃っていないと
/// 「ツリーには出るのに Finder には出ない」「表示しているのに自動リロードされない」が起きる。
/// bool を個別に配って回らないよう、無視設定はこの型 1 つにまとめて渡す
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScanOptions {
    pub(crate) show_hidden: bool,
    /// .gitignore / .ignore / .git/info/exclude で無視されるファイルも表示する。
    /// 既定 (false) では走査の時点で落ちるので、ツリーにも Finder にも現れない
    pub(crate) show_ignored: bool,
}

impl ScanOptions {
    /// 無視設定を反映した WalkBuilder。require_git(false) は git repo 外でも .gitignore を
    /// 効かせるため、parents は既定の true のまま (サブディレクトリ起点の 1 階層走査でも
    /// 祖先の .gitignore がそのまま効くのが遅延走査の前提)。
    /// show_ignored のときは無視ファイルの読み元を全て切る — 一部だけ残すと
    /// 「.gitignore の分だけ見える」といった中途半端な集合になり、説明できない
    pub(crate) fn walker(&self, dir: &Path) -> WalkBuilder {
        let mut builder = WalkBuilder::new(dir);
        builder
            .require_git(false)
            .hidden(!self.show_hidden)
            .git_ignore(!self.show_ignored)
            .git_global(!self.show_ignored)
            .git_exclude(!self.show_ignored)
            .ignore(!self.show_ignored);
        builder
    }
}

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
                ignored: node.ignored,
            }),
            NodeKind::Dir { .. } => {
                // 子がディレクトリ 1 つだけの階層は `api/v1` のように 1 行へ畳む
                // (VSCode の compact folders)。行の index_path・path・展開状態は
                // 連鎖の末端のノードのものになるので、開閉も選択の復元もそこへ効く
                let mut leaf = node;
                let mut name = node.name.clone();
                let mut ignored = node.ignored;
                let pushed = prefix.len();
                while let NodeKind::Dir { children, .. } = &leaf.kind {
                    let [only] = children.as_slice() else { break };
                    if !matches!(only.kind, NodeKind::Dir { .. })
                        || filter.is_some_and(|f| !f.contains(&only.path))
                    {
                        break;
                    }
                    name.push('/');
                    name.push_str(&only.name);
                    ignored |= only.ignored;
                    prefix.push(0);
                    leaf = only;
                }
                let NodeKind::Dir {
                    expanded, children, ..
                } = &leaf.kind
                else {
                    unreachable!("chain walks Dir nodes only");
                };
                rows.push(Row {
                    index_path: prefix.clone(),
                    name,
                    path: leaf.path.clone(),
                    depth,
                    is_dir: true,
                    expanded: *expanded,
                    ignored,
                });
                if *expanded {
                    flatten(children, depth + 1, prefix, rows, filter);
                }
                prefix.truncate(pushed);
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
pub(super) fn expand_all(nodes: &mut [Node], expanded: &HashSet<PathBuf>, opts: ScanOptions) {
    for node in nodes {
        if !matches!(node.kind, NodeKind::Dir { .. }) {
            continue;
        }
        if expanded.contains(&node.path) {
            load(node, opts);
            if let NodeKind::Dir {
                expanded: is_expanded,
                ..
            } = &mut node.kind
            {
                *is_expanded = true;
            }
        }
        if let NodeKind::Dir { children, .. } = &mut node.kind {
            expand_all(children, expanded, opts);
        }
    }
}

/// 展開状態を集合そのものに揃える (集合に無いディレクトリは閉じる)。
/// 絞り込み解除時に「絞り込み前の状態」へ厳密に戻すために使う
pub(super) fn set_expanded(nodes: &mut [Node], expanded: &HashSet<PathBuf>, opts: ScanOptions) {
    for node in nodes {
        if !matches!(node.kind, NodeKind::Dir { .. }) {
            continue;
        }
        let open = expanded.contains(&node.path);
        if open {
            load(node, opts);
        }
        if let NodeKind::Dir {
            expanded: is_expanded,
            children,
            ..
        } = &mut node.kind
        {
            *is_expanded = open;
            set_expanded(children, expanded, opts);
        }
    }
}

// 実パスでの検索 (index_path ではなく path で探す)。合成ノードが既に追加済みかどうかの
// 重複チェックに使う
fn node_by_path<'a>(nodes: &'a [Node], path: &Path) -> Option<&'a Node> {
    for node in nodes {
        if node.path == path {
            return Some(node);
        }
        if let NodeKind::Dir { children, .. } = &node.kind
            && let Some(found) = node_by_path(children, path)
        {
            return Some(found);
        }
    }
    None
}

/// git worktree/index 側で削除された (実ファイルが既に無い) パスを合成ノードとして追加する。
/// WalkBuilder は実ファイルしか見ないため、削除だけはこの経路で橋渡ししないと GIT レーンで
/// 選択も stage/unstage もできない。1件でも追加したら true (呼び出し側の rebuild_visible 要否判定用)
pub(super) fn sync_deleted(nodes: &mut Vec<Node>, root: &Path, deleted: &HashSet<PathBuf>) -> bool {
    let mut changed = false;
    for path in deleted {
        if node_by_path(nodes, path).is_none() {
            insert_missing(nodes, root, path);
            changed = true;
        }
    }
    if changed {
        sort_nodes(nodes);
    }
    changed
}

// insert() と違い、削除ファイルの合成先で親ディレクトリ自体が (ひきずられて) 存在しない
// 場合もその場で作る。通常の insert() が「親は必ず既存」を前提にできるのは深さ優先走査の
// 順序があるからで、合成挿入にはその前提が無い
fn insert_missing(top: &mut Vec<Node>, root: &Path, path: &Path) {
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
    let mut children = top;
    let mut acc = root.to_path_buf();
    for comp in &components {
        acc.push(comp);
        let pos = match children.iter().position(|n| n.name == *comp) {
            Some(pos) => pos,
            None => {
                children.push(Node {
                    name: comp.clone(),
                    path: acc.clone(),
                    // 削除ファイル (git が追跡している) の親なので無視対象ではありえない
                    ignored: false,
                    // loaded=false にするのは、この合成ディレクトリが実在する可能性があるため
                    // (遅延走査では「まだ読んでいないだけ」の実ディレクトリもツリーに現れない)。
                    // true にすると実在する場合に本物の子が二度と読まれなくなる。false なら
                    // 展開時に読み直され、実体が無ければ空のまま残るだけで済む
                    kind: NodeKind::Dir {
                        expanded: false,
                        loaded: false,
                        children: Vec::new(),
                    },
                });
                children.len() - 1
            }
        };
        match &mut children[pos].kind {
            NodeKind::Dir { children: c, .. } => children = c,
            // 経路上に同名ファイルがある異常系。合成は諦める
            NodeKind::File => return,
        }
    }
    if children.iter().any(|n| n.name == name) {
        return;
    }
    children.push(Node {
        name,
        path: path.to_path_buf(),
        ignored: false,
        kind: NodeKind::File,
    });
}

/// index_path 上の祖先ディレクトリを全て展開済みにする。畳んだ行 (`api/v1`) の
/// 開閉は末端ノードの expanded しか触らないので、途中のノードが閉じたまま
/// (絞り込みの出入りで復元された状態など) でも末端だけ開けてしまう。その後で
/// 途中の階層に兄弟が増えて連鎖が割れると、閉じた途中ノードが行に採用されて
/// 開いていた配下が突然消える。開く時に経路ごと揃えておけば割れても見え方が保たれる
pub(super) fn expand_ancestors(nodes: &mut [Node], index_path: &[usize]) {
    for len in 1..index_path.len() {
        if let Some(Node {
            kind: NodeKind::Dir { expanded, .. },
            ..
        }) = node_mut(nodes, &index_path[..len])
        {
            *expanded = true;
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
// 戻す必要がある)。
// parent_ignored は親ディレクトリ自体が無視対象かどうか。無視されたディレクトリの
// 配下は git 的にも全て無視対象なので、その場合は判定用の再走査を省いて全件 true にする
pub(super) fn read_dir(dir: &Path, opts: ScanOptions, parent_ignored: bool) -> Vec<Node> {
    let mut nodes = entries(dir, opts, parent_ignored);
    // 無視ファイルを出している間は「どれが無視対象か」を色で示したいが、ignore クレートの
    // 走査結果からはそれが分からない。同じ 1 階層を「無視を効かせた設定」でもう一度歩き、
    // そちらに出てこなかったものを無視対象と見なす — パターンの解釈 (否定・アンカー・
    // 祖先の .gitignore) を自前で持たずに、表示・非表示と完全に同じ判定を使うため
    if opts.show_ignored && !parent_ignored {
        let shown = shown_paths(
            dir,
            ScanOptions {
                show_ignored: false,
                ..opts
            },
        );
        for node in &mut nodes {
            node.ignored = !shown.contains(&node.path);
        }
    }
    nodes
}

// 無視を効かせた設定で見えるパスだけを集める (read_dir の判定用なので Node は組み立てない)
fn shown_paths(dir: &Path, opts: ScanOptions) -> HashSet<PathBuf> {
    opts.walker(dir)
        .max_depth(Some(1))
        .build()
        .flatten()
        .filter(|entry| entry.depth() > 0)
        .map(|entry| entry.path().to_path_buf())
        .collect()
}

fn entries(dir: &Path, opts: ScanOptions, ignored: bool) -> Vec<Node> {
    let mut nodes = Vec::new();
    for entry in opts.walker(dir).max_depth(Some(1)).build().flatten() {
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
            ignored,
            kind,
        });
    }
    sort_nodes(&mut nodes);
    nodes
}

/// 未走査のディレクトリなら子を読み込む。展開の直前に必ず通す
/// (「開こうとした時に読む」= 起動時にツリー全体を歩かないための入口)
pub(super) fn load(node: &mut Node, opts: ScanOptions) {
    let ignored = node.ignored;
    let path = node.path.clone();
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
    *children = read_dir(&path, opts, ignored);
}

/// 子がディレクトリ 1 つだけの階層を連鎖して読み込む。Java/Kotlin の
/// `com/example/app` のような「中身の無い中継ディレクトリ」を 1 段ずつ
/// 開かせないため。読み込んだ連鎖は `flatten` が 1 行 (`com/example/app`) に
/// 畳んで見せる。走査は連鎖の分だけ増えるが、どれも「開いた時に読む」範囲に収まる
pub(super) fn expand_single_child_chain(node: &mut Node, opts: ScanOptions) {
    let mut node = node;
    loop {
        load(node, opts);
        let NodeKind::Dir { children, .. } = &mut node.kind else {
            return;
        };
        let [only] = children.as_mut_slice() else {
            return;
        };
        let NodeKind::Dir { expanded, .. } = &mut only.kind else {
            return;
        };
        *expanded = true;
        node = only;
    }
}

/// 読み込み済みの階層だけを読み直して差分を取り込む。未走査のディレクトリには
/// 触らないので、再走査のコストは「今開いている範囲」に比例する。
/// 展開状態・読み込み済みの子は名前で引き継ぐ (index_path は作り直しになる)
pub(super) fn refresh(nodes: &mut Vec<Node>, dir: &Path, opts: ScanOptions, parent_ignored: bool) {
    let mut previous: HashMap<String, NodeKind> =
        nodes.drain(..).map(|node| (node.name, node.kind)).collect();
    let mut fresh = read_dir(dir, opts, parent_ignored);
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
            refresh(&mut children, &node.path, opts, node.ignored);
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
