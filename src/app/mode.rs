use crate::finder::Finder;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Viewer,
}

// Search と Goto (:N 行ジャンプ) の入力を kind で分ける
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Search,
    Goto,
}

pub enum Mode {
    Normal,
    Input { kind: InputKind, buffer: String },
    // Ctrl+p ファジーファインダー。Input に押し込むと Search/Goto と挙動が絡み合うため独立させる
    Finder(Finder),
    // キーバインド一覧のオーバーレイ。状態を持たないので unit variant で十分
    Help,
}
