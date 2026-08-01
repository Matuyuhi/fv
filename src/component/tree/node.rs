use std::path::PathBuf;

pub(super) struct Node {
    pub(super) name: String,
    pub(super) path: PathBuf,
    pub(super) kind: NodeKind,
}

pub(super) enum NodeKind {
    File,
    // 子は展開されるまで読まない。loaded=false は「未走査」であって「子が無い」ではないため、
    // children の空と区別できるようフラグで持つ
    Dir {
        expanded: bool,
        loaded: bool,
        children: Vec<Node>,
    },
}

/// 展開状態を反映した表示用の1行。index_path で実ノードを引く。
/// path は git 状態 (HashMap<PathBuf, _>) のキーと突き合わせるための絶対パス。
pub struct Row {
    pub(super) index_path: Vec<usize>,
    pub name: String,
    pub path: PathBuf,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
}
