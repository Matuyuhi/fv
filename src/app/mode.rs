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

// 設定画面の行ラベル。行の並び・件数はこの配列が唯一の情報源で、
// keys.rs (選択移動・selected の意味) と ui/settings_panel.rs (表示) の両方がここを参照する
pub const SETTINGS_ROWS: [&str; 4] = ["hidden files", "icons", "wrap (default)", "theme"];

#[derive(Default)]
pub struct SettingsState {
    pub selected: usize,
}

pub enum Mode {
    Normal,
    Input { kind: InputKind, buffer: String },
    // Ctrl+p ファジーファインダー。Input に押し込むと Search/Goto と挙動が絡み合うため独立させる
    Finder(Finder),
    // キーバインド一覧のオーバーレイ。状態を持たないので unit variant で十分
    Help,
    // 設定画面のオーバーレイ (s キー)
    Settings(SettingsState),
}
